use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::adapter::write_private_file_atomic;
use crate::domain::{
    compute_agent_exposed_models, AccountPool, AgentBinding, AgentBindingOptions,
    AgentBindingStatus, TargetAgentId, WireProtocolKind,
};
use crate::storage::pools::StoredAgentBinding;
use crate::storage::QwitchDatabase;

const CLAUDE_CONFIG_UUID: &str = "00000000-0000-4000-8000-717769746368";

pub fn agent_token(agent_id: TargetAgentId) -> &'static str {
    match agent_id {
        TargetAgentId::ClaudeDesktop => "qwitch-bind-claude-desktop",
        TargetAgentId::CodexDesktop => "qwitch-bind-codex-desktop",
        TargetAgentId::Dsh => "qwitch-bind-dsh",
        TargetAgentId::Opencode => "qwitch-bind-opencode",
    }
}

pub fn agent_id_from_token(token: &str) -> Option<TargetAgentId> {
    match token.trim() {
        "qwitch-bind-claude-desktop" => Some(TargetAgentId::ClaudeDesktop),
        "qwitch-bind-codex-desktop" => Some(TargetAgentId::CodexDesktop),
        "qwitch-bind-dsh" => Some(TargetAgentId::Dsh),
        "qwitch-bind-opencode" => Some(TargetAgentId::Opencode),
        _ => None,
    }
}

pub fn detect_bridge_runtime() -> (String, String) {
    let bun_candidates = [
        "bun".to_string(),
        std::env::var("HOME")
            .map(|h| format!("{h}/.bun/bin/bun"))
            .unwrap_or_default(),
        "/opt/homebrew/bin/bun".to_string(),
        "/usr/local/bin/bun".to_string(),
    ];
    for bin in bun_candidates {
        if bin.is_empty() {
            continue;
        }
        if let Ok(out) = Command::new(&bin).arg("--version").output() {
            if out.status.success() {
                let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return ("bun_ready".into(), format!("Bun {ver}"));
            }
        }
    }

    let node_candidates = [
        "node".to_string(),
        "/opt/homebrew/bin/node".to_string(),
        "/usr/local/bin/node".to_string(),
    ];
    for bin in node_candidates {
        if let Ok(out) = Command::new(&bin).arg("--version").output() {
            if out.status.success() {
                let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return ("node_ready".into(), format!("Node {ver} (回退)"));
            }
        }
    }

    ("missing".into(), "未检测到 Bun/Node".into())
}

pub fn detect_global_proxy_active() -> bool {
    let has_env_proxy = ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()));
    let no_proxy = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();
    let loopback_bypassed = no_proxy.contains("127.0.0.1") && no_proxy.contains("localhost");
    if has_env_proxy && !loopback_bypassed {
        return true;
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = Command::new("scutil").arg("--proxy").output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                let http_on =
                    text.contains("HTTPEnable : 1") || text.contains("HTTPSEnable : 1");
                if http_on && !text.contains("127.0.0.1") {
                    return true;
                }
            }
        }
    }
    false
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

struct AgentPaths {
    primary_display: String,
    files: Vec<PathBuf>,
    installed: bool,
    version: String,
}

fn resolve_agent_paths(
    agent_id: TargetAgentId,
    options: &AgentBindingOptions,
    base_dir_override: Option<&Path>,
) -> AgentPaths {
    let home = home_dir();
    match agent_id {
        TargetAgentId::ClaudeDesktop => {
            let isolated = options.claude_isolated_twin.unwrap_or(true);
            let dir = if let Some(base) = base_dir_override {
                base.join(if isolated { "Claude-Qwitch-3p" } else { "Claude-3p" })
            } else {
                #[cfg(target_os = "macos")]
                {
                    home.join("Library/Application Support").join(if isolated {
                        "Claude-Qwitch-3p"
                    } else {
                        "Claude-3p"
                    })
                }
                #[cfg(not(target_os = "macos"))]
                {
                    std::env::var_os("LOCALAPPDATA")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home.join("AppData/Local"))
                        .join(if isolated { "Claude-Qwitch-3p" } else { "Claude-3p" })
                }
            };
            let main_cfg = if let Some(base) = base_dir_override {
                base.join("Claude/claude_desktop_config.json")
            } else {
                #[cfg(target_os = "macos")]
                {
                    home.join("Library/Application Support/Claude/claude_desktop_config.json")
                }
                #[cfg(not(target_os = "macos"))]
                {
                    dir.join("claude_desktop_config.json")
                }
            };
            let lib_dir = dir.join("configLibrary");
            let installed = base_dir_override.is_some()
                || main_cfg.parent().is_some_and(|p| p.exists())
                || dir.exists();
            AgentPaths {
                primary_display: if isolated {
                    "~/Library/Application Support/Claude-Qwitch-3p/configLibrary/".into()
                } else {
                    "~/Library/Application Support/Claude-3p/configLibrary/".into()
                },
                files: vec![
                    main_cfg,
                    lib_dir.join(format!("{CLAUDE_CONFIG_UUID}.json")),
                    lib_dir.join("_meta.json"),
                ],
                installed,
                version: "2.x 3p".into(),
            }
        }
        TargetAgentId::Opencode => {
            let dir = if let Some(base) = base_dir_override {
                base.join("opencode")
            } else if let Ok(custom) = std::env::var("OPENCODE_CONFIG_DIR") {
                PathBuf::from(custom)
            } else {
                home.join(".config/opencode")
            };
            let cfg_file = ["opencode.jsonc", "opencode.json", "config.json"]
                .iter()
                .map(|name| dir.join(name))
                .find(|p| p.exists())
                .unwrap_or_else(|| dir.join("opencode.jsonc"));
            let version = Command::new("opencode")
                .arg("--version")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "2.x".into());
            let installed = base_dir_override.is_some() || dir.exists();
            AgentPaths {
                primary_display: "~/.config/opencode/opencode.jsonc".into(),
                files: vec![cfg_file],
                installed,
                version,
            }
        }
        TargetAgentId::Dsh => {
            let dsh_home = if let Some(base) = base_dir_override {
                base.join("dsh")
            } else if let Ok(custom) = std::env::var("DSH_HOME") {
                PathBuf::from(custom)
            } else {
                home.join(".dsh")
            };
            let installed = base_dir_override.is_some() || dsh_home.exists();
            AgentPaths {
                primary_display: "~/.dsh/profiles/desktop/cordis.patch.yml".into(),
                files: vec![
                    dsh_home.join("profiles/desktop/cordis.patch.yml"),
                    dsh_home.join(".env"),
                    dsh_home.join(".credentials.yaml"),
                ],
                installed,
                version: "0.2.x".into(),
            }
        }
        TargetAgentId::CodexDesktop => {
            let codex_home = if let Some(base) = base_dir_override {
                base.join("codex")
            } else if let Ok(custom) = std::env::var("CODEX_HOME") {
                PathBuf::from(custom)
            } else {
                home.join(".codex")
            };
            let installed = base_dir_override.is_some() || codex_home.exists();
            AgentPaths {
                primary_display: "~/.codex/config.toml".into(),
                files: vec![
                    codex_home.join("config.toml"),
                    codex_home.join("qwitch_models.json"),
                ],
                installed,
                version: "Responses".into(),
            }
        }
    }
}

pub fn build_agent_config_snippets(
    agent_id: TargetAgentId,
    mounted_pool_ids: &[String],
    default_model_key: Option<&str>,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    options: &AgentBindingOptions,
    is_drifted: bool,
) -> (String, String) {
    let base_url = format!("http://{host}:{port}");
    let exposed = compute_agent_exposed_models(
        agent_id,
        mounted_pool_ids,
        default_model_key,
        all_pools,
        chrono::Utc::now().timestamp_millis(),
    );
    let default_wire = exposed
        .iter()
        .find(|m| m.is_default)
        .or_else(|| exposed.first())
        .map(|m| m.wire_model_id.clone())
        .unwrap_or_else(|| "coding-main/claude-sonnet-4.6".to_string());

    match agent_id {
        TargetAgentId::ClaudeDesktop => {
            let dir = if options.claude_isolated_twin.unwrap_or(true) {
                "~/Library/Application Support/Claude-Qwitch-3p"
            } else {
                "~/Library/Application Support/Claude-3p"
            };
            let model_lines = if exposed.is_empty() {
                "//   （当前未挂载号池）".to_string()
            } else {
                exposed
                    .iter()
                    .map(|m| {
                        format!(
                            "//   {}{} -> display_name: \"{}\" ({})",
                            if m.is_default { "★ " } else { "" },
                            m.wire_model_id,
                            m.display_name,
                            m.tier_summary
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let before = format!(
                "// {dir}/claude_desktop_config.json\n{{\n  \"deploymentMode\": \"cloud\"\n}}"
            );
            let after = format!(
                "// {dir}/claude_desktop_config.json\n{{\n  \"deploymentMode\": \"3p\"\n}}\n\n// {dir}/configLibrary/{CLAUDE_CONFIG_UUID}.json\n{{\n  \"inferenceProvider\": \"gateway\",\n  \"inferenceGatewayBaseUrl\": \"{base_url}\",\n  \"inferenceGatewayApiKey\": \"{}\",\n  \"inferenceGatewayAuthScheme\": \"bearer\",\n  \"disableDeploymentModeChooser\": true,\n  \"coworkEgressAllowedHosts\": [\"*\"],\n  \"toolSearchEnabled\": true\n}}\n// GET /v1/models 暴露清单:\n{model_lines}",
                agent_token(agent_id)
            );
            (before, after)
        }
        TargetAgentId::CodexDesktop => {
            let catalog_lines = if exposed.is_empty() {
                "#   - (未挂载号池)".to_string()
            } else {
                exposed
                    .iter()
                    .map(|m| format!("#   - {} ({}, {})", m.wire_model_id, m.display_name, m.tier_summary))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let before = "# ~/.codex/config.toml\nmodel = \"gpt-5.4\"\n# 未写入 Qwitch provider".to_string();
            let after = format!(
                "# ~/.codex/config.toml\n# qwitch:managed-start\nmodel_provider = \"qwitch\"\nmodel = \"{default_wire}\"\n# qwitch:managed-end\n\n[model_providers.qwitch]\nname = \"Qwitch Multi-Pool Gateway\"\nbase_url = \"{base_url}/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"{}\"\n# Catalog 模型清单:\n{catalog_lines}",
                agent_token(agent_id)
            );
            (before, after)
        }
        TargetAgentId::Dsh => {
            let before_creds = if is_drifted {
                "# ~/.dsh/.credentials.yaml  [⚠️ 检测缺失 QWITCH_GATEWAY_KEY]\nrefs:\n  DEEPSEEK_API_KEY: \"sk-orig-***\""
            } else {
                "# ~/.dsh/.credentials.yaml\nrefs:\n  DEEPSEEK_API_KEY: \"sk-orig-***\""
            };
            let models_yaml = if exposed.is_empty() {
                format!("          - id: \"{default_wire}\"\n            name: \"Qwitch Default\"")
            } else {
                exposed
                    .iter()
                    .map(|m| format!("          - id: \"{}\"\n            name: \"{}\"", m.wire_model_id, m.display_name))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let before = format!(
                "# ~/.dsh/profiles/desktop/cordis.patch.yml\n- id: llm-deepseek\n  config:\n    model: deepseek-chat\n\n{before_creds}"
            );
            let after = format!(
                "# ~/.dsh/profiles/desktop/cordis.patch.yml\n- id: llm-pi-ai\n  config:\n    providers:\n      qwitch:\n        baseUrl: \"{base_url}/v1\"\n        apiKeyEnv: \"QWITCH_GATEWAY_KEY\"\n        models:\n{models_yaml}\n    agent-default-model: \"qwitch/{default_wire}\"\n\n# ~/.dsh/.env & ~/.dsh/.credentials.yaml\nrefs:\n  QWITCH_GATEWAY_KEY: \"{}\"",
                agent_token(agent_id)
            );
            (before, after)
        }
        TargetAgentId::Opencode => {
            let schema_v1 = options.opencode_schema_version.as_deref() == Some("v1");
            let before = "// ~/.config/opencode/opencode.jsonc\n{\n  \"model\": \"deepseek/deepseek-chat\"\n}".to_string();
            let after_json = if schema_v1 {
                let mut map = serde_json::Map::new();
                for m in &exposed {
                    map.insert(m.wire_model_id.clone(), json!({ "name": m.display_name }));
                }
                json!({
                    "model": format!("qwitch/{default_wire}"),
                    "provider": {
                        "qwitch": {
                            "npm": "@ai-sdk/openai-compatible",
                            "name": "Qwitch 号池网关",
                            "options": {
                                "baseURL": format!("{base_url}/v1"),
                                "apiKey": agent_token(agent_id)
                            },
                            "models": map
                        }
                    }
                })
            } else {
                let arr: Vec<Value> = exposed
                    .iter()
                    .map(|m| json!({ "id": m.wire_model_id, "name": m.display_name }))
                    .collect();
                json!({
                    "model": format!("qwitch/{default_wire}"),
                    "providers": {
                        "qwitch": {
                            "name": "Qwitch 号池网关",
                            "package": "aisdk:@ai-sdk/openai-compatible",
                            "settings": {
                                "baseURL": format!("{base_url}/v1"),
                                "apiKey": agent_token(agent_id)
                            },
                            "models": arr
                        }
                    }
                })
            };
            let after = format!(
                "// ~/.config/opencode/opencode.jsonc\n{}",
                serde_json::to_string_pretty(&after_json).unwrap_or_default()
            );
            (before, after)
        }
    }
}

fn check_disk_drift(
    agent_id: TargetAgentId,
    mounted_pool_ids: &[String],
    paths: &AgentPaths,
    port: u16,
    options: &AgentBindingOptions,
) -> Option<String> {
    if mounted_pool_ids.is_empty() {
        return None;
    }
    let port_needle = format!(":{port}");
    match agent_id {
        TargetAgentId::Dsh => {
            let patch_ok = std::fs::read_to_string(&paths.files[0])
                .map(|c| c.contains("qwitch:") && c.contains(&port_needle))
                .unwrap_or(false);
            let creds_ok = std::fs::read_to_string(&paths.files[2])
                .map(|c| c.contains("QWITCH_GATEWAY_KEY"))
                .unwrap_or(false);
            if !creds_ok {
                return Some(
                    "检测到 ~/.dsh/.credentials.yaml 缺失 QWITCH_GATEWAY_KEY（dsh 桌面版不读取 .env 会报鉴权缺失）"
                        .into(),
                );
            }
            if !patch_ok {
                return Some("检测到 ~/.dsh/profiles/desktop/cordis.patch.yml 未同步最新 Qwitch 网关地址".into());
            }
            None
        }
        TargetAgentId::Opencode => {
            let content = std::fs::read_to_string(&paths.files[0]).unwrap_or_default();
            let wants_v1 = options.opencode_schema_version.as_deref() == Some("v1");
            let has_key = if wants_v1 {
                content.contains("\"provider\"")
            } else {
                content.contains("\"providers\"")
            };
            if !has_key || !content.contains("\"qwitch\"") || !content.contains(&port_needle) {
                return Some("检测到 opencode 配置与当前 Schema / 端口不一致，建议重新同步写入".into());
            }
            None
        }
        TargetAgentId::ClaudeDesktop => {
            let main_ok = std::fs::read_to_string(&paths.files[0])
                .map(|c| c.contains("\"3p\""))
                .unwrap_or(false);
            let lib_ok = std::fs::read_to_string(&paths.files[1])
                .map(|c| c.contains(&port_needle))
                .unwrap_or(false);
            if !main_ok || !lib_ok {
                return Some("检测到 Claude Desktop 3p 网关配置被修改或端口未同步".into());
            }
            None
        }
        TargetAgentId::CodexDesktop => {
            let toml_ok = std::fs::read_to_string(&paths.files[0])
                .map(|c| c.contains("[model_providers.qwitch]") && c.contains("model_provider = \"qwitch\"") && c.contains(&port_needle))
                .unwrap_or(false);
            if !toml_ok {
                return Some("检测到 ~/.codex/config.toml 中 Qwitch provider 配置或端口已漂移".into());
            }
            None
        }
    }
}

fn default_options_for_agent(agent_id: TargetAgentId) -> AgentBindingOptions {
    match agent_id {
        TargetAgentId::ClaudeDesktop => AgentBindingOptions {
            claude_isolated_twin: Some(true),
            ..Default::default()
        },
        TargetAgentId::CodexDesktop => AgentBindingOptions {
            codex_preserve_table: Some(true),
            ..Default::default()
        },
        TargetAgentId::Dsh => AgentBindingOptions {
            dsh_dual_creds_written: Some(false),
            ..Default::default()
        },
        TargetAgentId::Opencode => AgentBindingOptions {
            opencode_schema_version: Some("v2".into()),
            ..Default::default()
        },
    }
}

fn agent_meta(agent_id: TargetAgentId) -> (&'static str, &'static str, WireProtocolKind) {
    match agent_id {
        TargetAgentId::ClaudeDesktop => (
            "Claude 桌面版",
            "官方 3p 网关模式 · 同时挂载多号池，下拉框直选具体模型",
            WireProtocolKind::AnthropicMessages,
        ),
        TargetAgentId::Opencode => (
            "OpenCode",
            "CLI / TUI / OpenChamber 编码代理 · 支持多号池同列切换",
            WireProtocolKind::OpenaiChat,
        ),
        TargetAgentId::Dsh => (
            "DeepSeek Harness",
            "dsh 桌面端与 Web 代理 · 写入 llm-pi-ai 自定义路由",
            WireProtocolKind::OpenaiChat,
        ),
        TargetAgentId::CodexDesktop => (
            "Codex 桌面版",
            "ChatGPT / Codex App · 独立 [model_providers.qwitch] Responses 端点",
            WireProtocolKind::OpenaiResponses,
        ),
    }
}

pub fn build_agent_binding_dto(
    stored: &StoredAgentBinding,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
) -> AgentBinding {
    let paths = resolve_agent_paths(stored.agent_id, &stored.options, base_dir_override);
    let mut options = stored.options.clone();
    let drift_reason = check_disk_drift(
        stored.agent_id,
        &stored.mounted_pool_ids,
        &paths,
        port,
        &options,
    );
    if stored.agent_id == TargetAgentId::Dsh {
        let creds_written = paths
            .files
            .get(2)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .is_some_and(|c| c.contains("QWITCH_GATEWAY_KEY"));
        options.dsh_dual_creds_written = Some(creds_written);
    }

    let status = if stored.mounted_pool_ids.is_empty() {
        AgentBindingStatus::Unbound
    } else if drift_reason.is_some() {
        AgentBindingStatus::Drifted
    } else if stored.status == AgentBindingStatus::NeedsRestart {
        AgentBindingStatus::NeedsRestart
    } else {
        AgentBindingStatus::Bound
    };

    let (before_snippet, after_snippet) = build_agent_config_snippets(
        stored.agent_id,
        &stored.mounted_pool_ids,
        stored.default_model_key.as_deref(),
        all_pools,
        host,
        port,
        &options,
        status == AgentBindingStatus::Drifted,
    );

    let (name, subtitle, protocol) = agent_meta(stored.agent_id);
    let proxy_warning = if stored.agent_id == TargetAgentId::CodexDesktop && global_proxy {
        Some(
            "检测到系统开启 HTTP 全局代理，Codex 内置 reqwest 默认不读系统忽略列表，直连 127.0.0.1 可能返回 502（建议设置 NO_PROXY=127.0.0.1,localhost）"
                .into(),
        )
    } else {
        None
    };

    let restart_hint = if status == AgentBindingStatus::NeedsRestart {
        Some(match stored.agent_id {
            TargetAgentId::ClaudeDesktop => {
                "Claude 桌面版仅在启动时读取 3p 配置，请重启以拉取最新多号池 /v1/models".into()
            }
            TargetAgentId::CodexDesktop => {
                "已写入 ~/.codex/config.toml，请重启 app-server（无需关闭桌面窗口）".into()
            }
            _ => "配置已更新，重启后生效".into(),
        })
    } else {
        None
    };

    AgentBinding {
        agent_id: stored.agent_id,
        name: name.into(),
        subtitle: subtitle.into(),
        protocol,
        config_path: paths.primary_display,
        installed: paths.installed,
        version: paths.version,
        status,
        mounted_pool_ids: stored.mounted_pool_ids.clone(),
        default_model_key: stored.default_model_key.clone(),
        drift_reason,
        proxy_warning,
        restart_hint,
        last_hit_at: stored.last_hit_at.clone(),
        options,
        before_snippet,
        after_snippet,
    }
}

pub fn inspect_all_agent_bindings(
    db: &QwitchDatabase,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
) -> Result<Vec<AgentBinding>, String> {
    let stored_list = db.get_stored_agent_bindings().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(4);
    for agent_id in TargetAgentId::all() {
        let stored = stored_list
            .iter()
            .find(|b| b.agent_id == agent_id)
            .cloned()
            .unwrap_or_else(|| StoredAgentBinding {
                agent_id,
                mounted_pool_ids: Vec::new(),
                default_model_key: None,
                options: default_options_for_agent(agent_id),
                status: AgentBindingStatus::Unbound,
                last_hit_at: None,
            });
        out.push(build_agent_binding_dto(
            &stored,
            all_pools,
            host,
            port,
            global_proxy,
            base_dir_override,
        ));
    }
    Ok(out)
}

fn stash_original_files(
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    agent_id: TargetAgentId,
    paths: &[PathBuf],
) -> Result<(), String> {
    for p in paths {
        let path_str = p.to_string_lossy().to_string();
        match std::fs::read(p) {
            Ok(bytes) => db
                .save_agent_stash(master_key, agent_id, &path_str, true, Some(&bytes))
                .map_err(|e| e.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => db
                .save_agent_stash(master_key, agent_id, &path_str, false, None)
                .map_err(|e| e.to_string())?,
            Err(e) => return Err(format!("读取原始配置 {} 失败: {e}", p.display())),
        }
    }
    Ok(())
}

fn upsert_toml_key_line(content: &str, key: &str, new_line: &str) -> String {
    let prefix = format!("{key} =");
    let prefix_tight = format!("{key}=");
    let mut replaced = false;
    let mut lines: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = true;
        }
        if !in_section && (trimmed.starts_with(&prefix) || trimmed.starts_with(&prefix_tight)) && !replaced {
            lines.push(new_line.to_string());
            replaced = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced {
        lines.insert(0, new_line.to_string());
    }
    lines.join("\n") + "\n"
}

fn strip_toml_section(content: &str, section_header: &str) -> String {
    let mut out = Vec::new();
    let mut skipping = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == section_header {
            skipping = true;
            continue;
        }
        if skipping && trimmed.starts_with('[') {
            skipping = false;
        }
        if !skipping {
            out.push(line);
        }
    }
    out.join("\n")
}

fn strip_jsonc_comments(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    let mut in_str = false;
    let mut escaped = false;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

pub fn apply_agent_wiring(
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    agent_id: TargetAgentId,
    mounted_pool_ids: &[String],
    default_model_key: Option<&str>,
    options: &AgentBindingOptions,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
) -> Result<AgentBinding, String> {
    if mounted_pool_ids.is_empty() {
        return unbind_and_restore_agent(
            db,
            master_key,
            agent_id,
            all_pools,
            host,
            port,
            global_proxy,
            base_dir_override,
        );
    }

    let mut clean_opts = options.clone();
    let paths = resolve_agent_paths(agent_id, &clean_opts, base_dir_override);
    stash_original_files(db, master_key, agent_id, &paths.files)?;

    let exposed = compute_agent_exposed_models(
        agent_id,
        mounted_pool_ids,
        default_model_key,
        all_pools,
        chrono::Utc::now().timestamp_millis(),
    );
    let default_item = exposed.iter().find(|m| m.is_default).or_else(|| exposed.first());
    let effective_default_key = default_item.map(|m| m.key.clone());
    let default_wire = default_item
        .map(|m| m.wire_model_id.clone())
        .unwrap_or_else(|| "coding-main/claude-sonnet-4.6".into());
    let base_url = format!("http://{host}:{port}");
    let token = agent_token(agent_id);

    match agent_id {
        TargetAgentId::ClaudeDesktop => {
            let mut main_json: Value = std::fs::read_to_string(&paths.files[0])
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| json!({}));
            if let Some(obj) = main_json.as_object_mut() {
                obj.insert("deploymentMode".into(), Value::String("3p".into()));
            }
            write_private_file_atomic(
                &paths.files[0],
                serde_json::to_string_pretty(&main_json)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )
            .map_err(|e| e.to_string())?;

            let lib_cfg = json!({
                "inferenceProvider": "gateway",
                "inferenceGatewayBaseUrl": base_url,
                "inferenceGatewayApiKey": token,
                "inferenceGatewayAuthScheme": "bearer",
                "disableDeploymentModeChooser": true,
                "coworkEgressAllowedHosts": ["*"],
                "toolSearchEnabled": true
            });
            write_private_file_atomic(
                &paths.files[1],
                serde_json::to_string_pretty(&lib_cfg)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )
            .map_err(|e| e.to_string())?;

            let meta_cfg = json!({ "appliedId": CLAUDE_CONFIG_UUID });
            write_private_file_atomic(
                &paths.files[2],
                serde_json::to_string_pretty(&meta_cfg)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )
            .map_err(|e| e.to_string())?;
        }
        TargetAgentId::Opencode => {
            let existing = std::fs::read_to_string(&paths.files[0]).unwrap_or_else(|_| "{}".into());
            let mut doc: Value = serde_json::from_str(&strip_jsonc_comments(&existing))
                .unwrap_or_else(|_| json!({}));
            let is_v1 = clean_opts.opencode_schema_version.as_deref() == Some("v1");
            if let Some(obj) = doc.as_object_mut() {
                obj.insert("model".into(), Value::String(format!("qwitch/{default_wire}")));
                if is_v1 {
                    let mut models_map = serde_json::Map::new();
                    for m in &exposed {
                        models_map.insert(m.wire_model_id.clone(), json!({ "name": m.display_name }));
                    }
                    let provider_map = obj
                        .entry("provider")
                        .or_insert_with(|| json!({}))
                        .as_object_mut();
                    if let Some(pm) = provider_map {
                        pm.insert(
                            "qwitch".into(),
                            json!({
                                "npm": "@ai-sdk/openai-compatible",
                                "name": "Qwitch 号池网关",
                                "options": {
                                    "baseURL": format!("{base_url}/v1"),
                                    "apiKey": token
                                },
                                "models": models_map
                            }),
                        );
                    }
                } else {
                    let models_arr: Vec<Value> = exposed
                        .iter()
                        .map(|m| json!({ "id": m.wire_model_id, "name": m.display_name }))
                        .collect();
                    let providers_map = obj
                        .entry("providers")
                        .or_insert_with(|| json!({}))
                        .as_object_mut();
                    if let Some(pm) = providers_map {
                        pm.insert(
                            "qwitch".into(),
                            json!({
                                "name": "Qwitch 号池网关",
                                "package": "aisdk:@ai-sdk/openai-compatible",
                                "settings": {
                                    "baseURL": format!("{base_url}/v1"),
                                    "apiKey": token
                                },
                                "models": models_arr
                            }),
                        );
                    }
                }
            }
            write_private_file_atomic(
                &paths.files[0],
                serde_json::to_string_pretty(&doc)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )
            .map_err(|e| e.to_string())?;
        }
        TargetAgentId::Dsh => {
            let models_yaml = if exposed.is_empty() {
                format!("          - id: \"{default_wire}\"\n            name: \"Qwitch Default\"")
            } else {
                exposed
                    .iter()
                    .map(|m| format!("          - id: \"{}\"\n            name: \"{}\"", m.wire_model_id, m.display_name))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let patch_content = format!(
                "- id: llm-pi-ai\n  config:\n    providers:\n      qwitch:\n        baseUrl: \"{base_url}/v1\"\n        apiKeyEnv: \"QWITCH_GATEWAY_KEY\"\n        models:\n{models_yaml}\n    agent-default-model: \"qwitch/{default_wire}\"\n"
            );
            write_private_file_atomic(&paths.files[0], patch_content.as_bytes())
                .map_err(|e| e.to_string())?;

            // 双写 .env 与 .credentials.yaml（dsh 桌面版强依赖 .credentials.yaml）
            let mut env_text = std::fs::read_to_string(&paths.files[1]).unwrap_or_default();
            if !env_text.contains("QWITCH_GATEWAY_KEY=") {
                if !env_text.is_empty() && !env_text.ends_with('\n') {
                    env_text.push('\n');
                }
                env_text.push_str(&format!("QWITCH_GATEWAY_KEY={token}\n"));
            }
            write_private_file_atomic(&paths.files[1], env_text.as_bytes())
                .map_err(|e| e.to_string())?;

            let mut creds_text = std::fs::read_to_string(&paths.files[2]).unwrap_or_default();
            if !creds_text.contains("QWITCH_GATEWAY_KEY") {
                if !creds_text.contains("refs:") {
                    if !creds_text.is_empty() && !creds_text.ends_with('\n') {
                        creds_text.push('\n');
                    }
                    creds_text.push_str(&format!("refs:\n  QWITCH_GATEWAY_KEY: \"{token}\"\n"));
                } else {
                    if !creds_text.ends_with('\n') {
                        creds_text.push('\n');
                    }
                    creds_text.push_str(&format!("  QWITCH_GATEWAY_KEY: \"{token}\"\n"));
                }
            }
            write_private_file_atomic(&paths.files[2], creds_text.as_bytes())
                .map_err(|e| e.to_string())?;
            clean_opts.dsh_dual_creds_written = Some(true);
        }
        TargetAgentId::CodexDesktop => {
            let existing = std::fs::read_to_string(&paths.files[0]).unwrap_or_default();
            let stripped = strip_toml_section(&existing, "[model_providers.qwitch]");
            let with_provider =
                upsert_toml_key_line(&stripped, "model_provider", "model_provider = \"qwitch\"");
            let mut with_model = upsert_toml_key_line(
                &with_provider,
                "model",
                &format!("model = \"{default_wire}\""),
            );
            if !with_model.ends_with('\n') {
                with_model.push('\n');
            }
            with_model.push_str(&format!(
                "\n[model_providers.qwitch]\nname = \"Qwitch Multi-Pool Gateway\"\nbase_url = \"{base_url}/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"{token}\"\n"
            ));
            write_private_file_atomic(&paths.files[0], with_model.as_bytes())
                .map_err(|e| e.to_string())?;

            let catalog = json!({
                "models": exposed.iter().map(|m| json!({
                    "slug": m.wire_model_id,
                    "display_name": m.display_name,
                    "context_window": 200000,
                    "auto_compact_token_limit": 160000
                })).collect::<Vec<_>>()
            });
            write_private_file_atomic(
                &paths.files[1],
                serde_json::to_string_pretty(&catalog)
                    .map_err(|e| e.to_string())?
                    .as_bytes(),
            )
            .map_err(|e| e.to_string())?;
        }
    }

    let next_status = if matches!(
        agent_id,
        TargetAgentId::ClaudeDesktop | TargetAgentId::CodexDesktop
    ) {
        AgentBindingStatus::NeedsRestart
    } else {
        AgentBindingStatus::Bound
    };

    let stored = StoredAgentBinding {
        agent_id,
        mounted_pool_ids: mounted_pool_ids.to_vec(),
        default_model_key: effective_default_key,
        options: clean_opts,
        status: next_status,
        last_hit_at: None,
    };
    db.upsert_agent_binding(&stored).map_err(|e| e.to_string())?;

    Ok(build_agent_binding_dto(
        &stored,
        all_pools,
        host,
        port,
        global_proxy,
        base_dir_override,
    ))
}

pub fn unbind_and_restore_agent(
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    agent_id: TargetAgentId,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
) -> Result<AgentBinding, String> {
    let stashes = db
        .get_agent_stashes(master_key, agent_id)
        .map_err(|e| e.to_string())?;

    for item in &stashes {
        let p = PathBuf::from(&item.file_path);
        if agent_id == TargetAgentId::CodexDesktop
            && p.file_name().and_then(|s| s.to_str()) == Some("config.toml")
        {
            // Codex 解绑保留 [model_providers.qwitch] 表，防止加载历史线程失败
            if let Ok(curr) = std::fs::read_to_string(&p) {
                let mut restored = String::new();
                if let Some(orig_bytes) = &item.original_content {
                    let orig_str = String::from_utf8_lossy(orig_bytes);
                    restored = orig_str.to_string();
                } else {
                    for line in curr.lines() {
                        let t = line.trim();
                        if !t.starts_with("model_provider = \"qwitch\"") {
                            restored.push_str(line);
                            restored.push('\n');
                        }
                    }
                }
                if !restored.contains("[model_providers.qwitch]") {
                    if let Some(idx) = curr.find("[model_providers.qwitch]") {
                        if !restored.ends_with('\n') {
                            restored.push('\n');
                        }
                        restored.push('\n');
                        restored.push_str(&curr[idx..]);
                    }
                }
                let _ = write_private_file_atomic(&p, restored.as_bytes());
                continue;
            }
        }

        if item.original_existed {
            if let Some(bytes) = &item.original_content {
                write_private_file_atomic(&p, bytes).map_err(|e| e.to_string())?;
            }
        } else {
            let _ = std::fs::remove_file(&p);
        }
    }

    let _ = db.delete_agent_stashes(agent_id);

    let existing_opts = db
        .get_stored_agent_bindings()
        .ok()
        .and_then(|list| list.into_iter().find(|b| b.agent_id == agent_id))
        .map(|b| b.options)
        .unwrap_or_else(|| default_options_for_agent(agent_id));

    let stored = StoredAgentBinding {
        agent_id,
        mounted_pool_ids: Vec::new(),
        default_model_key: None,
        options: existing_opts,
        status: AgentBindingStatus::Unbound,
        last_hit_at: None,
    };
    db.upsert_agent_binding(&stored).map_err(|e| e.to_string())?;

    Ok(build_agent_binding_dto(
        &stored,
        all_pools,
        host,
        port,
        global_proxy,
        base_dir_override,
    ))
}

pub fn restart_agent_process(
    db: &QwitchDatabase,
    agent_id: TargetAgentId,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
) -> Result<AgentBinding, String> {
    if agent_id == TargetAgentId::CodexDesktop {
        #[cfg(unix)]
        {
            let _ = Command::new("pkill")
                .args(["-f", "codex.*app-server"])
                .output();
        }
    }

    let stored_list = db.get_stored_agent_bindings().map_err(|e| e.to_string())?;
    let mut stored = stored_list
        .into_iter()
        .find(|b| b.agent_id == agent_id)
        .unwrap_or_else(|| StoredAgentBinding {
            agent_id,
            mounted_pool_ids: Vec::new(),
            default_model_key: None,
            options: default_options_for_agent(agent_id),
            status: AgentBindingStatus::Unbound,
            last_hit_at: None,
        });
    stored.status = if stored.mounted_pool_ids.is_empty() {
        AgentBindingStatus::Unbound
    } else {
        AgentBindingStatus::Bound
    };
    db.upsert_agent_binding(&stored).map_err(|e| e.to_string())?;

    Ok(build_agent_binding_dto(
        &stored,
        all_pools,
        host,
        port,
        global_proxy,
        base_dir_override,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AffinityMode, CreatePoolInput, CreatePoolMemberInput, CredentialEnvelope,
        CredentialPayload, MemberTier, PoolStrategy,
    };
    use std::collections::HashMap;
    use uuid::Uuid;

    fn setup_wiring_env() -> (
        QwitchDatabase,
        [u8; 32],
        PathBuf,
        PathBuf,
        Vec<AccountPool>,
    ) {
        let root = std::env::temp_dir().join(format!("qwitch-wiring-{}", Uuid::new_v4()));
        let db = QwitchDatabase::open(&root.join("qwitch.db")).expect("open db");
        let home_dir = root.join("agent-home");
        std::fs::create_dir_all(home_dir.join("codex")).unwrap();
        std::fs::create_dir_all(home_dir.join("dsh")).unwrap();
        std::fs::write(home_dir.join("codex/config.toml"), "model = \"gpt-5.4\"\n").unwrap();
        std::fs::write(home_dir.join("dsh/.env"), "DEEPSEEK_API_KEY=sk-orig\n").unwrap();
        std::fs::write(
            home_dir.join("dsh/.credentials.yaml"),
            "refs:\n  DEEPSEEK_API_KEY: \"sk-orig\"\n",
        )
        .unwrap();

        let key = [5u8; 32];
        let acc = db
            .upsert_account(
                &key,
                Some("WB"),
                &CredentialEnvelope {
                    tool_id: "workbuddy".into(),
                    uid: "wb_1".into(),
                    display_name: Some("WB".into()),
                    display_email: None,
                    payload: CredentialPayload::Token {
                        access_token: "tok".into(),
                        refresh_token: None,
                        expires_at: None,
                        extra: HashMap::new(),
                    },
                },
            )
            .unwrap();
        let pool = db
            .create_pool(
                &key,
                &CreatePoolInput {
                    id: None,
                    name: "Coding池".into(),
                    description: "".into(),
                    scene_tag: "Coding".into(),
                    strategy: PoolStrategy::QuotaFirst,
                    affinity_mode: AffinityMode::AutoCache,
                    skip_active_client_account: false,
                    cooldown_seconds: 60,
                    auto_refresh_on_401: true,
                    fallback_api_key_enabled: false,
                    members: vec![CreatePoolMemberInput {
                        account_id: acc.id,
                        supported_models: vec!["claude-sonnet-4.6".into()],
                        tier: MemberTier::T1Primary,
                    }],
                    enabled_model_ids: None,
                },
            )
            .unwrap();
        (db, key, root, home_dir, vec![pool])
    }

    #[test]
    fn dsh_wiring_detects_missing_credentials_drift_and_restores_cleanly() {
        let (db, key, root, home_dir, pools) = setup_wiring_env();
        let pid = pools[0].id.clone();

        let wired = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::Dsh,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions::default(),
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire dsh");
        assert_eq!(wired.status, AgentBindingStatus::Bound);
        assert!(std::fs::read_to_string(home_dir.join("dsh/.credentials.yaml"))
            .unwrap()
            .contains("QWITCH_GATEWAY_KEY"));

        // 模拟外部删掉 .credentials.yaml 中的 Qwitch key -> 必须报配置漂移
        std::fs::write(
            home_dir.join("dsh/.credentials.yaml"),
            "refs:\n  DEEPSEEK_API_KEY: \"sk-orig\"\n",
        )
        .unwrap();
        let inspected = inspect_all_agent_bindings(
            &db,
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .unwrap();
        let dsh = inspected
            .iter()
            .find(|b| b.agent_id == TargetAgentId::Dsh)
            .unwrap();
        assert_eq!(dsh.status, AgentBindingStatus::Drifted);

        // 还原后恢复原始文件内容
        let unbound = unbind_and_restore_agent(
            &db,
            &key,
            TargetAgentId::Dsh,
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("unbind");
        assert_eq!(unbound.status, AgentBindingStatus::Unbound);
        assert_eq!(
            std::fs::read_to_string(home_dir.join("dsh/.env")).unwrap(),
            "DEEPSEEK_API_KEY=sk-orig\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_unbind_restores_top_model_while_preserving_qwitch_provider_table() {
        let (db, key, root, home_dir, pools) = setup_wiring_env();
        let pid = pools[0].id.clone();

        let wired = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::CodexDesktop,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions {
                codex_preserve_table: Some(true),
                ..Default::default()
            },
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire codex");
        assert_eq!(wired.status, AgentBindingStatus::NeedsRestart);

        unbind_and_restore_agent(
            &db,
            &key,
            TargetAgentId::CodexDesktop,
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("unbind codex");

        let restored = std::fs::read_to_string(home_dir.join("codex/config.toml")).unwrap();
        assert!(restored.contains("model = \"gpt-5.4\""));
        assert!(!restored.contains("model_provider = \"qwitch\""));
        assert!(restored.contains("[model_providers.qwitch]"));
        let _ = std::fs::remove_dir_all(root);
    }
}
