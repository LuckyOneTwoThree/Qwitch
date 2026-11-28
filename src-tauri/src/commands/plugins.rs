use chrono::Utc;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

use crate::domain::WireProtocolKind;
use crate::plugins::{
    commit_staged_plugin, discard_staged_plugin, remove_external_plugin_dir, stage_dir_plugin,
    stage_json_plugin, stage_zip_plugin, sync_global_registry_snapshot, BridgeMatrix,
    CompatibilityEngine, CompatibilityReport, DeclarativeEgressPlugin, EgressTransport, PluginKind,
    PluginManifest, PluginRecord, PluginSource, PluginTransport, ProbeCapabilities,
    SubprocessPluginHost,
};
use crate::storage::plugins::{StoredCapabilityProbe, StoredPluginState};

use super::AppState;

const PROBE_TTL_DAYS: i64 = 7;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginItemDto {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: PluginKind,
    pub transport: PluginTransport,
    pub source: PluginSource,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_hash: Option<String>,
    pub overrides_builtin: bool,
    pub short_tagline: String,
    pub wire_protocols: Vec<WireProtocolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provides_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_tools: Option<bool>,
}

fn record_to_dto(rec: &PluginRecord) -> PluginItemDto {
    let mut wire_protocols = Vec::new();
    if let Some(ep) = &rec.manifest.egress_profile {
        wire_protocols.extend(ep.wire_protocols.iter().copied());
    }
    if let Some(ip) = &rec.manifest.ingress_profile {
        for p in &ip.wire_protocols {
            if !wire_protocols.contains(p) {
                wire_protocols.push(*p);
            }
        }
    }
    PluginItemDto {
        id: rec.manifest.id.clone(),
        name: rec.manifest.name.clone(),
        version: rec.manifest.version.clone(),
        kind: rec.manifest.kind,
        transport: rec.manifest.transport,
        source: rec.source,
        enabled: rec.enabled,
        pinned_hash: rec.pinned_hash.clone(),
        overrides_builtin: rec.overrides_builtin,
        short_tagline: rec.manifest.brand.short_tagline.clone(),
        wire_protocols,
        provides_tools: rec
            .manifest
            .egress_profile
            .as_ref()
            .map(|p| p.provides.tools),
        requires_tools: rec
            .manifest
            .ingress_profile
            .as_ref()
            .map(|p| p.requires.tools),
    }
}

pub(crate) fn build_external_egress_transport(
    manifest: &PluginManifest,
    dir: &Path,
    hash: &str,
) -> Option<Arc<dyn EgressTransport>> {
    if !matches!(manifest.kind, PluginKind::Egress | PluginKind::Hybrid) {
        return None;
    }
    match manifest.transport {
        PluginTransport::Declarative => DeclarativeEgressPlugin::from_manifest(manifest)
            .ok()
            .map(|p| Arc::new(p) as Arc<dyn EgressTransport>),
        PluginTransport::Subprocess => {
            SubprocessPluginHost::new(manifest.clone(), dir.to_path_buf(), hash.to_string())
                .ok()
                .map(|p| Arc::new(p) as Arc<dyn EgressTransport>)
        }
        PluginTransport::Builtin => None,
    }
}

fn kind_to_str(kind: PluginKind) -> &'static str {
    match kind {
        PluginKind::Egress => "egress",
        PluginKind::Ingress => "ingress",
        PluginKind::Hybrid => "hybrid",
    }
}

fn source_to_str(source: PluginSource) -> &'static str {
    match source {
        PluginSource::Builtin => "builtin",
        PluginSource::External => "external",
    }
}

fn list_dtos(state: &AppState) -> Result<Vec<PluginItemDto>, String> {
    let reg = state
        .plugins
        .read()
        .map_err(|_| "插件注册表锁异常".to_string())?;
    Ok(reg
        .list_all()
        .into_iter()
        .filter(|r| !crate::domain::is_api_provider_tool_id(&r.manifest.id))
        .map(record_to_dto)
        .collect())
}

#[tauri::command]
pub fn list_plugins_cmd(state: State<'_, AppState>) -> Result<Vec<PluginItemDto>, String> {
    list_dtos(&state)
}

#[tauri::command]
pub fn toggle_plugin_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    plugin_id: String,
    enabled: bool,
) -> Result<Vec<PluginItemDto>, String> {
    let (kind_str, src_str) = {
        let mut reg = state
            .plugins
            .write()
            .map_err(|_| "插件注册表锁异常".to_string())?;
        reg.set_enabled(&plugin_id, enabled)?;
        sync_global_registry_snapshot(&reg);
        let rec = reg
            .get(&plugin_id)
            .ok_or_else(|| format!("找不到插件: {plugin_id}"))?;
        (kind_to_str(rec.manifest.kind), source_to_str(rec.source))
    };

    state
        .db
        .set_plugin_enabled(&plugin_id, kind_str, src_str, enabled)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("qwitch://pool-workspace-updated", ());
    list_dtos(&state)
}

#[tauri::command]
pub async fn install_external_plugin_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    source_path: String,
) -> Result<PluginItemDto, String> {
    let src = Path::new(source_path.trim());
    if !src.exists() {
        return Err(format!("插件路径不存在: {}", src.display()));
    }
    let data_dir = state.host.qwitch_data_dir().map_err(|e| e.to_string())?;

    let builtin_ids: HashSet<String> = {
        let reg = state
            .plugins
            .read()
            .map_err(|_| "插件注册表锁异常".to_string())?;
        reg.list_all()
            .into_iter()
            .filter(|r| r.source == PluginSource::Builtin || r.overrides_builtin)
            .map(|r| r.manifest.id.clone())
            .collect()
    };

    let staged = if src.is_dir() {
        stage_dir_plugin(src, &data_dir, &builtin_ids)?
    } else if src
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
    {
        stage_zip_plugin(src, &data_dir, &builtin_ids)?
    } else if src
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        stage_json_plugin(src, &data_dir, &builtin_ids)?
    } else {
        return Err("仅支持导入插件目录、.zip 压缩包或声明式 .json 文件".into());
    };

    let hash_preview: String = staged.content_hash.chars().take(12).collect();
    let title = if staged.overrides_builtin {
        "确认安装并覆盖官方内置插件"
    } else {
        "确认安装外部插件"
    };
    let override_notice = if staged.overrides_builtin {
        "\n⚠️ 注意：该插件 ID 与 Qwitch 内置默认插件相同，安装后将覆盖官方内置实现（卸载后可恢复）。\n"
    } else {
        ""
    };
    let detail = format!(
        "插件名称：{}\n插件 ID：{}\n版本：{}\n类型：{:?} / {:?}\nSHA-256 指纹：{hash_preview}…{override_notice}\n\n仅在你确认该插件来源可信时继续安装。",
        staged.manifest.name,
        staged.manifest.id,
        staged.manifest.version,
        staged.manifest.kind,
        staged.manifest.transport,
    );

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let res = rfd::MessageDialog::new()
            .set_title(title)
            .set_description(detail)
            .set_level(if staged.overrides_builtin {
                rfd::MessageLevel::Warning
            } else {
                rfd::MessageLevel::Info
            })
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        let _ = tx.send(res == rfd::MessageDialogResult::Yes);
    })
    .map_err(|e| format!("调度安装确认框失败: {e}"))?;

    let confirmed = rx.await.map_err(|e| format!("安装确认框执行失败: {e}"))?;
    if !confirmed {
        discard_staged_plugin(&staged);
        return Err("已取消安装外部插件".into());
    }

    let manifest = staged.manifest.clone();
    let hash = staged.content_hash.clone();
    let overrides_builtin = staged.overrides_builtin;
    let final_dir = commit_staged_plugin(staged, &data_dir)?;

    let transport = build_external_egress_transport(&manifest, &final_dir, &hash);
    let now = Utc::now().to_rfc3339();
    let stored = StoredPluginState {
        plugin_id: manifest.id.clone(),
        kind: kind_to_str(manifest.kind).to_string(),
        source: "external".to_string(),
        enabled: true,
        pinned_hash: Some(hash.clone()),
        overrides_builtin,
        installed_at: now.clone(),
        updated_at: now,
    };

    let dto = {
        let mut reg = state
            .plugins
            .write()
            .map_err(|_| "插件注册表锁异常".to_string())?;
        reg.register_external(manifest.clone(), hash, final_dir, true)?;
        if let Some(tr) = transport {
            reg.register_egress_transport(tr);
        }
        sync_global_registry_snapshot(&reg);
        let rec = reg
            .get(&manifest.id)
            .ok_or_else(|| "注册插件后读取失败".to_string())?;
        record_to_dto(rec)
    };

    state
        .db
        .upsert_plugin_state(&stored)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("qwitch://pool-workspace-updated", ());
    Ok(dto)
}

#[tauri::command]
pub fn remove_external_plugin_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<Vec<PluginItemDto>, String> {
    let data_dir = state.host.qwitch_data_dir().map_err(|e| e.to_string())?;
    let restored_builtin = {
        let mut reg = state
            .plugins
            .write()
            .map_err(|_| "插件注册表锁异常".to_string())?;
        let rec = reg
            .get(&plugin_id)
            .ok_or_else(|| format!("找不到插件: {plugin_id}"))?;
        if rec.source != PluginSource::External {
            return Err("默认内置插件不可物理删除，请改用「停用」开关".into());
        }
        remove_external_plugin_dir(&data_dir, &plugin_id)?;
        reg.remove_external(&plugin_id);
        let restored = reg
            .get(&plugin_id)
            .is_some_and(|r| r.source == PluginSource::Builtin);
        sync_global_registry_snapshot(&reg);
        restored
    };

    if restored_builtin {
        let _ = state.db.delete_plugin_state(&plugin_id);
    } else {
        state
            .db
            .delete_plugin_state(&plugin_id)
            .map_err(|e| e.to_string())?;
    }
    let _ = app.emit("qwitch://pool-workspace-updated", ());
    list_dtos(&state)
}

fn is_probe_fresh(verified_at: &str) -> bool {
    let Ok(ts) = chrono::DateTime::parse_from_rfc3339(verified_at) else {
        return false;
    };
    Utc::now()
        .signed_duration_since(ts.with_timezone(&Utc))
        .num_days()
        < PROBE_TTL_DAYS
}

#[tauri::command]
pub fn check_agent_egress_compatibility_cmd(
    state: State<'_, AppState>,
    agent_id: String,
    egress_plugin_ids: Vec<String>,
) -> Result<HashMap<String, CompatibilityReport>, String> {
    let reg = state
        .plugins
        .read()
        .map_err(|_| "插件注册表锁异常".to_string())?;
    let ingress = reg
        .ingress_profile(&agent_id)
        .ok_or_else(|| format!("未找到已启用的入口插件: {agent_id}"))?
        .clone();

    let probes = state.db.list_capability_probes(None).unwrap_or_default();
    let matrix = BridgeMatrix::default_kernel();
    let mut out = HashMap::new();

    for egress_id in egress_plugin_ids {
        let Some(egress) = reg.egress_profile(&egress_id) else {
            continue;
        };
        let probe_caps = probes
            .iter()
            .find(|p| p.egress_plugin_id == egress_id && is_probe_fresh(&p.verified_at))
            .and_then(|p| serde_json::from_str::<ProbeCapabilities>(&p.probe_json).ok());
        let report = CompatibilityEngine::evaluate(&ingress, egress, probe_caps.as_ref(), &matrix);
        out.insert(egress_id, report);
    }

    Ok(out)
}

#[tauri::command]
pub fn probe_egress_capabilities_cmd(
    state: State<'_, AppState>,
    egress_plugin_id: String,
    model_id: String,
) -> Result<StoredCapabilityProbe, String> {
    let profile = {
        let reg = state
            .plugins
            .read()
            .map_err(|_| "插件注册表锁异常".to_string())?;
        reg.egress_profile(&egress_plugin_id)
            .ok_or_else(|| format!("未找到已启用的出口插件: {egress_plugin_id}"))?
            .clone()
    };

    let has_active_account = state
        .db
        .list_accounts(Some(&egress_plugin_id))
        .map(|list| {
            list.iter()
                .any(|a| a.status == crate::domain::AccountStatus::Active && a.has_api_token)
        })
        .unwrap_or(false);

    let (caps, error_summary) = if has_active_account || egress_plugin_id == "custom_api_key" {
        (
            ProbeCapabilities {
                streaming: Some(profile.provides.streaming),
                tools: Some(profile.provides.tools),
                system_prompt_ok: Some(true),
                vision: Some(profile.provides.vision),
            },
            None,
        )
    } else {
        (
            ProbeCapabilities::default(),
            Some(format!(
                "未检测到 {} 的活跃在线凭据，已保留静态能力声明",
                egress_plugin_id
            )),
        )
    };

    let probe = StoredCapabilityProbe {
        egress_plugin_id,
        model_id,
        probe_json: serde_json::to_string(&caps).map_err(|e| e.to_string())?,
        verified_at: Utc::now().to_rfc3339(),
        error_summary,
    };
    state
        .db
        .upsert_capability_probe(&probe)
        .map_err(|e| e.to_string())?;
    Ok(probe)
}
