use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

use crate::agent_wiring::{
    apply_agent_wiring, clear_agent_wiring, collect_agent_running_states,
    inspect_all_agent_bindings, restart_agent_process, unbind_and_restore_agent,
};
use crate::commands::{load_app_settings, AppState, SETTINGS_KEY};
use crate::domain::{
    AccountPool, AgentBinding, CreatePoolInput, MemberTier, PoolStrategy, PoolWorkspaceState,
    RunPoolSimulationInput, RunPoolSimulationResult, TargetAgentId, UpdatePoolPatch,
};
use crate::gateway::GatewayRuntime;
use crate::storage::model_discovery::ProbeOutcome;
use crate::storage::QwitchDatabase;

/// 单个账号的探测结果三元组：`(账号 ID, 工具 ID, 探测结果)`。
type AccountProbeResults = Vec<(String, String, ProbeOutcome)>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearAllAgentsResult {
    pub workspace: PoolWorkspaceState,
    pub cleared: Vec<String>,
    pub failed: Vec<String>,
}

/// 单个 Agent 的运行状态（轻量轮询专用）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunningState {
    pub agent_id: TargetAgentId,
    pub running: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayPortCheck {
    pub available: bool,
    pub in_use_by_self: bool,
    pub message: String,
}

/// 单个账号的模型探测结果明细。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamModelProbeItem {
    pub account_id: String,
    pub tool_id: String,
    /// "live"（本次实时探测成功）/ "cache"（本机二级目录）/ "catalog"（静态兜底）
    pub source: String,
    pub model_count: usize,
    /// 探测失败原因（回退时也会带上，便于向用户如实说明）
    pub error: Option<String>,
}

/// 「刷新真实模型」命令的返回体。
///
/// 不再只返回工作区快照：必须能区分「真的探测到了」与「回退到静态目录」，
/// 否则前端只能弹一个无条件成功的绿色提示。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshUpstreamModelsResult {
    pub workspace: PoolWorkspaceState,
    pub probed_count: usize,
    pub live_count: usize,
    pub fallback_count: usize,
    pub failed_count: usize,
    pub items: Vec<UpstreamModelProbeItem>,
}

/// TTL 感知的自动探测结果（导入 / OAuth 完成 / 首次进入 Agent 调度页时触发）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsureUpstreamModelsResult {
    pub probed_count: usize,
    /// 因 live 缓存仍然新鲜而跳过探测的账号数
    pub skipped_count: usize,
    /// 缺少可用凭据、无法探测的账号数
    pub no_credential_count: usize,
    pub live_count: usize,
    pub fallback_count: usize,
    pub failed_count: usize,
}

fn build_workspace_state(
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    gateway: &GatewayRuntime,
) -> Result<PoolWorkspaceState, String> {
    let gw_state = gateway.runtime_state();
    let cooldowns = gateway.cooldown_snapshot();
    // 候选账号只构建一次：`list_pools` 内部同样需要它，历史实现会重复构建两遍
    // （每遍都要逐账号解密凭据），是工作区快照的主要成本之一。
    let candidates = db
        .list_upstream_candidates(master_key)
        .map_err(|e| e.to_string())?;
    let pools = db
        .list_pools_with_candidates(&candidates, &cooldowns)
        .map_err(|e| e.to_string())?;
    let agent_bindings = inspect_all_agent_bindings(
        db,
        &pools,
        &gw_state.host,
        gw_state.port,
        gw_state.global_proxy_detected,
        None,
    )?;
    let traces = db.list_routing_traces(200).map_err(|e| e.to_string())?;

    Ok(PoolWorkspaceState {
        gateway: gw_state,
        pools,
        candidates,
        agent_bindings,
        traces,
    })
}

fn emit_workspace_updated(app: &AppHandle) {
    let _ = app.emit("qwitch://pool-workspace-updated", ());
}

#[tauri::command]
pub async fn get_pool_workspace_state(
    state: State<'_, AppState>,
) -> Result<PoolWorkspaceState, String> {
    // async：Tauri 的同步命令在主线程执行，全量快照（含进程探测与磁盘读）
    // 会直接冻结窗口；改为 async 后落到运行时的线程池。
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

/// 轻量运行状态轮询：只回 6 个 Agent 的 `运行/未运行`。
///
/// 前端每 6 秒刷新按钮状态时曾调用 `get_pool_workspace_state`，
/// 每次都要构建 200KB 全量快照；此命令只做一次进程快照。
#[tauri::command]
pub fn detect_agent_running_cmd() -> Vec<AgentRunningState> {
    collect_agent_running_states()
        .into_iter()
        .map(|(agent_id, running)| AgentRunningState { agent_id, running })
        .collect()
}

/// 套用工具级「内置推荐目录」（码道 / 有道龙虾 / 豆包工作等无探测接口的工具）。
#[tauri::command]
pub fn apply_account_preset_models_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<PoolWorkspaceState, String> {
    let summary = state
        .db
        .get_account_summary(&account_id)
        .map_err(|e| e.to_string())?;
    state
        .db
        .save_preset_account_models(&account_id, &summary.tool_id)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

/// 人工维护某账号的模型目录。
///
/// 用于没有可靠实时探测接口的工具（码道 CodeArts / 有道龙虾 / 豆包工作）：
/// 与其展示静态猜测表，不如让用户按账号填入真实模型 ID。传空数组即清空。
#[tauri::command]
pub fn set_account_manual_models_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    models: Vec<String>,
) -> Result<PoolWorkspaceState, String> {
    let summary = state
        .db
        .get_account_summary(&account_id)
        .map_err(|e| e.to_string())?;
    state
        .db
        .save_manual_account_models(&account_id, &summary.tool_id, &models)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

/// 一轮模型探测的执行摘要。
struct ProbeRun {
    /// 实际发起探测并拿到结果的账号
    results: AccountProbeResults,
    /// 因 live 缓存仍然新鲜而跳过探测的账号
    fresh_skipped: usize,
    /// 缺少可用凭据、无法探测的账号
    no_credential: usize,
}

/// 对指定账号集合发起实时模型探测。
///
/// `stale_only = true` 时只探测「没有新鲜 live 缓存」的账号（TTL 由存储层判定），
/// 用于自动触发场景，避免每次进入页面都打一轮上游请求。
async fn probe_account_models(
    state: &AppState,
    account_id: Option<&str>,
    stale_only: bool,
) -> ProbeRun {
    let Ok(target_accounts) = state.db.list_accounts(None) else {
        return ProbeRun {
            results: Vec::new(),
            fresh_skipped: 0,
            no_credential: 0,
        };
    };
    let cached = state.db.load_cached_models_map().unwrap_or_default();

    let mut handles = Vec::new();
    let mut fresh_skipped = 0usize;
    let mut no_credential = 0usize;
    for acc in target_accounts {
        if let Some(fid) = account_id {
            if acc.id != fid {
                continue;
            }
        }
        if stale_only && cached.contains_key(&acc.id) {
            fresh_skipped += 1;
            continue;
        }
        let Ok((_, cred)) = state.db.get_account_credential(&state.master_key, &acc.id) else {
            no_credential += 1;
            continue;
        };
        let aid = acc.id.clone();
        let tid = acc.tool_id.clone();
        handles.push(tokio::spawn(async move {
            let discovered = crate::storage::model_discovery::probe_models_for_account(&cred).await;
            (aid, tid, discovered)
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(item) = h.await {
            results.push(item);
        }
    }
    ProbeRun {
        results,
        fresh_skipped,
        no_credential,
    }
}

/// 把探测结果落库（**只落权威来源**：实时探测 / 本机客户端）并统计真实结果。
///
/// 回退到静态目录 / 本机二级目录的结果一律不入库：
/// 一旦入库，「探测失败」会被固化成「探测成功」，模型目录从此锁死在旧版本。
fn persist_probe_results(
    state: &AppState,
    results: &AccountProbeResults,
) -> (usize, usize, usize, Vec<UpstreamModelProbeItem>) {
    let mut live_count = 0usize;
    let mut fallback_count = 0usize;
    let mut failed_count = 0usize;
    let mut items = Vec::with_capacity(results.len());

    for (account_id, tool_id, outcome) in results {
        if outcome.is_authoritative() && !outcome.models.is_empty() {
            live_count += 1;
            if let Err(err) =
                state
                    .db
                    .save_cached_account_models(account_id, tool_id, &outcome.models)
            {
                // 落库失败不改变探测事实，但必须让调用方知道缓存没写进去。
                items.push(UpstreamModelProbeItem {
                    account_id: account_id.clone(),
                    tool_id: tool_id.clone(),
                    source: outcome.source.to_string(),
                    model_count: outcome.models.len(),
                    error: Some(format!("写入模型缓存失败: {err}")),
                });
                continue;
            }
        } else if outcome.models.is_empty() {
            failed_count += 1;
        } else {
            fallback_count += 1;
        }

        items.push(UpstreamModelProbeItem {
            account_id: account_id.clone(),
            tool_id: tool_id.clone(),
            source: outcome.source.to_string(),
            model_count: outcome.models.len(),
            error: outcome.error.clone(),
        });
    }

    (live_count, fallback_count, failed_count, items)
}

#[tauri::command]
pub async fn refresh_upstream_models_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> Result<RefreshUpstreamModelsResult, String> {
    let filter_id = account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let run = probe_account_models(&state, filter_id, false).await;
    let (live_count, fallback_count, failed_count, items) =
        persist_probe_results(&state, &run.results);

    emit_workspace_updated(&app);
    let workspace = build_workspace_state(&state.db, &state.master_key, &state.gateway)?;
    Ok(RefreshUpstreamModelsResult {
        workspace,
        probed_count: run.results.len(),
        live_count,
        fallback_count,
        failed_count,
        items,
    })
}

/// TTL 感知的自动探测：只探测「缓存已过期或缺失」的账号。
///
/// 触发点：账号导入完成、OAuth 授权完成、首次进入 Agent 调度页。
/// 目的是让用户不必手动点「刷新真实模型」也能拿到完整目录，
/// 同时避免每次进入页面都打一轮上游请求。
#[tauri::command]
pub async fn ensure_upstream_models_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> Result<EnsureUpstreamModelsResult, String> {
    let filter_id = account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let run = probe_account_models(&state, filter_id, true).await;
    let (live_count, fallback_count, failed_count, _items) =
        persist_probe_results(&state, &run.results);

    if !run.results.is_empty() {
        emit_workspace_updated(&app);
    }

    Ok(EnsureUpstreamModelsResult {
        probed_count: run.results.len(),
        skipped_count: run.fresh_skipped,
        no_credential_count: run.no_credential,
        live_count,
        fallback_count,
        failed_count,
    })
}

#[tauri::command]
pub async fn toggle_gateway_server(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PoolWorkspaceState, String> {
    if state.gateway.runtime_state().running {
        state.gateway.stop_server();
    } else {
        state
            .gateway
            .start_server(
                Arc::clone(&state.db),
                state.master_key,
                Arc::clone(&state.adapters),
            )
            .await?;
    }
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn check_gateway_port_available_cmd(
    state: State<'_, AppState>,
    port: u16,
) -> Result<GatewayPortCheck, String> {
    if port < 1024 {
        return Ok(GatewayPortCheck {
            available: false,
            in_use_by_self: false,
            message: "端口必须 >= 1024".into(),
        });
    }
    let cur = state.gateway.runtime_state();
    if cur.running && cur.port == port {
        return Ok(GatewayPortCheck {
            available: true,
            in_use_by_self: true,
            message: "当前网关正在使用该端口".into(),
        });
    }
    match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            drop(listener);
            Ok(GatewayPortCheck {
                available: true,
                in_use_by_self: false,
                message: "端口可用".into(),
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Ok(GatewayPortCheck {
            available: false,
            in_use_by_self: false,
            message: format!("端口 {port} 已被其它进程占用"),
        }),
        Err(e) => Ok(GatewayPortCheck {
            available: false,
            in_use_by_self: false,
            message: format!("端口检测失败：{e}"),
        }),
    }
}

#[tauri::command]
pub async fn update_gateway_port_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    port: u16,
) -> Result<PoolWorkspaceState, String> {
    if port < 1024 {
        return Err("端口必须 >= 1024".into());
    }
    let cur = state.gateway.runtime_state();
    if cur.running && cur.port == port {
        return build_workspace_state(&state.db, &state.master_key, &state.gateway);
    }
    if cur.port != port {
        match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => drop(listener),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                return Err(format!("端口 {port} 已被其它进程占用，请更换后再试"));
            }
            Err(e) => return Err(format!("端口 {port} 不可用：{e}")),
        }
    }
    let old_port = state.gateway.runtime_state().port;
    let was_running = state.gateway.runtime_state().running;
    if was_running {
        state.gateway.stop_server();
    }
    state.gateway.set_port(port)?;
    if was_running {
        if let Err(e) = state
            .gateway
            .start_server(
                Arc::clone(&state.db),
                state.master_key,
                Arc::clone(&state.adapters),
            )
            .await
        {
            // 启动失败则回滚到旧端口并尽量恢复服务，避免“改端口把网关搞停”
            let _ = state.gateway.set_port(old_port);
            let _ = state
                .gateway
                .start_server(
                    Arc::clone(&state.db),
                    state.master_key,
                    Arc::clone(&state.adapters),
                )
                .await;
            return Err(format!("端口 {port} 启动失败，已回滚到 {old_port}：{e}"));
        }
    }
    // 先持久化已生效的端口，避免后续单个 Agent 写入失败导致运行时端口与持久化配置失步
    {
        let mut settings = load_app_settings(&state.db);
        settings.gateway_port = port;
        if let Ok(json) = serde_json::to_string(&settings) {
            let _ = state.db.set_setting(SETTINGS_KEY, &json);
        }
    }
    // 重新同步已挂载 Agent 配置端口（单个失败不中断其余 Agent，并汇总回报）
    let ws = build_workspace_state(&state.db, &state.master_key, &state.gateway)?;
    let mut sync_errors: Vec<String> = Vec::new();
    for b in &ws.agent_bindings {
        if !b.mounted_pool_ids.is_empty() {
            if let Err(err) = apply_agent_wiring(
                &state.db,
                &state.master_key,
                b.agent_id,
                &b.mounted_pool_ids,
                b.default_model_key.as_deref(),
                &b.options,
                &ws.pools,
                &ws.gateway.host,
                port,
                ws.gateway.global_proxy_detected,
                None,
            ) {
                sync_errors.push(format!("{}：{}", b.name, err));
            }
        }
    }
    emit_workspace_updated(&app);
    if !sync_errors.is_empty() {
        return Err(format!(
            "网关端口已切换为 {port}，但部分 Agent 配置同步失败（请前往「Agent 接入与调度」修复）：{}",
            sync_errors.join("；")
        ));
    }
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn cycle_bridge_runtime_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PoolWorkspaceState, String> {
    state.gateway.cycle_bridge_runtime();
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn toggle_proxy_simulation_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PoolWorkspaceState, String> {
    state.gateway.toggle_proxy_simulation();
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn create_account_pool(
    app: AppHandle,
    state: State<'_, AppState>,
    input: CreatePoolInput,
) -> Result<AccountPool, String> {
    let created = state
        .db
        .create_pool(&state.master_key, &input)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    Ok(created)
}

#[tauri::command]
pub fn update_account_pool(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    patch: UpdatePoolPatch,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .update_pool(&pool_id, &patch)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn delete_account_pool(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let stored = state
        .db
        .get_stored_agent_bindings()
        .map_err(|e| e.to_string())?;
    state.db.delete_pool(&pool_id).map_err(|e| e.to_string())?;

    let remaining_pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;

    for b in stored {
        if b.mounted_pool_ids.contains(&pool_id) {
            let next_mounted: Vec<String> = b
                .mounted_pool_ids
                .into_iter()
                .filter(|id| id != &pool_id)
                .collect();
            apply_agent_wiring(
                &state.db,
                &state.master_key,
                b.agent_id,
                &next_mounted,
                b.default_model_key.as_deref(),
                &b.options,
                &remaining_pools,
                &gw.host,
                gw.port,
                gw.global_proxy_detected,
                None,
            )?;
        }
    }
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn toggle_account_pool_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .toggle_pool_enabled(&pool_id)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn reorder_account_pool_member(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    member_id: String,
    direction: String,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .reorder_pool_member(&pool_id, &member_id, &direction)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn toggle_account_pool_member(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    member_id: String,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .toggle_pool_member(&pool_id, &member_id)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn toggle_account_pool_member_model(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    member_id: String,
    model_id: String,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .toggle_pool_member_model(&pool_id, &member_id, &model_id)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn update_account_pool_member_tier(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    member_id: String,
    tier: MemberTier,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .update_pool_member_tier(&pool_id, &member_id, tier)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn toggle_account_pool_model_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    model_id: String,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .toggle_pool_model_enabled(&pool_id, &model_id)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn set_account_pool_model_fallback(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    model_id: String,
    fallback_model_id: Option<String>,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .set_pool_model_fallback(&pool_id, &model_id, fallback_model_id.as_deref())
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn add_account_pool_members(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    members: Vec<crate::domain::CreatePoolMemberInput>,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .add_members_to_pool(&state.master_key, &pool_id, &members)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn remove_account_pool_member(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    member_id: String,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .remove_pool_member(&pool_id, &member_id)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn inject_pool_member_cooldown(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    member_id: String,
) -> Result<PoolWorkspaceState, String> {
    let cooldowns = state.gateway.cooldown_snapshot();
    let pools = state
        .db
        .list_pools(&state.master_key, &cooldowns)
        .map_err(|e| e.to_string())?;
    let pool = pools
        .iter()
        .find(|p| p.id == pool_id)
        .ok_or_else(|| format!("找不到号池 {pool_id}"))?;
    let member = pool
        .members
        .iter()
        .find(|m| m.id == member_id)
        .ok_or_else(|| format!("找不到成员 {member_id}"))?;
    let until = chrono::Utc::now().timestamp_millis()
        + (pool.cooldown_seconds.clamp(10, 86_400) as i64) * 1000;
    state.gateway.set_member_cooldown(&member.account_id, until);
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn clear_pool_member_cooldown(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    member_id: String,
) -> Result<PoolWorkspaceState, String> {
    let cooldowns = state.gateway.cooldown_snapshot();
    let pools = state
        .db
        .list_pools(&state.master_key, &cooldowns)
        .map_err(|e| e.to_string())?;
    if let Some(member) = pools
        .iter()
        .find(|p| p.id == pool_id)
        .and_then(|p| p.members.iter().find(|m| m.id == member_id))
    {
        state.gateway.clear_member_cooldown(&member.account_id);
    }
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn authorize_codearts_isolated_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<PoolWorkspaceState, String> {
    state
        .db
        .authorize_codearts_isolated(&state.master_key, &account_id)
        .map_err(|e| e.to_string())?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn update_agent_mounted_pools_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
    mounted_pool_ids: Vec<String>,
    default_model_key: Option<String>,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    let existing = state
        .db
        .get_stored_agent_bindings()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|b| b.agent_id == agent_id);
    let options = existing
        .as_ref()
        .map(|b| b.options.clone())
        .unwrap_or_default();
    let effective_default =
        default_model_key.or_else(|| existing.and_then(|b| b.default_model_key));

    let _binding: AgentBinding = apply_agent_wiring(
        &state.db,
        &state.master_key,
        agent_id,
        &mounted_pool_ids,
        effective_default.as_deref(),
        &options,
        &pools,
        &gw.host,
        gw.port,
        gw.global_proxy_detected,
        None,
    )?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn set_agent_default_model_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
    model_key: String,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    let existing = state
        .db
        .get_stored_agent_bindings()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|b| b.agent_id == agent_id);
    let mut mounted = existing
        .as_ref()
        .map(|b| b.mounted_pool_ids.clone())
        .unwrap_or_default();
    if let Some((pid, _)) = model_key.split_once("::") {
        if !mounted.iter().any(|id| id == pid) {
            mounted.push(pid.to_string());
        }
    }
    let options = existing.map(|b| b.options).unwrap_or_default();
    let _ = apply_agent_wiring(
        &state.db,
        &state.master_key,
        agent_id,
        &mounted,
        Some(&model_key),
        &options,
        &pools,
        &gw.host,
        gw.port,
        gw.global_proxy_detected,
        None,
    )?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn mount_pool_to_agents_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    pool_id: String,
    agent_ids: Vec<TargetAgentId>,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    let stored_list = state
        .db
        .get_stored_agent_bindings()
        .map_err(|e| e.to_string())?;

    for agent_id in agent_ids {
        let existing = stored_list.iter().find(|b| b.agent_id == agent_id);
        let mut mounted = existing
            .map(|b| b.mounted_pool_ids.clone())
            .unwrap_or_default();
        if !mounted.contains(&pool_id) {
            mounted.push(pool_id.clone());
        }
        let options = existing.map(|b| b.options.clone()).unwrap_or_default();
        let def_key = existing.and_then(|b| b.default_model_key.clone());
        apply_agent_wiring(
            &state.db,
            &state.master_key,
            agent_id,
            &mounted,
            def_key.as_deref(),
            &options,
            &pools,
            &gw.host,
            gw.port,
            gw.global_proxy_detected,
            None,
        )?;
    }
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn unmount_pool_from_agent_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
    pool_id: String,
) -> Result<PoolWorkspaceState, String> {
    let stored_list = state
        .db
        .get_stored_agent_bindings()
        .map_err(|e| e.to_string())?;
    let Some(existing) = stored_list.into_iter().find(|b| b.agent_id == agent_id) else {
        return build_workspace_state(&state.db, &state.master_key, &state.gateway);
    };
    let next_mounted: Vec<String> = existing
        .mounted_pool_ids
        .into_iter()
        .filter(|id| id != &pool_id)
        .collect();
    update_agent_mounted_pools_cmd(
        app,
        state,
        agent_id,
        next_mounted,
        existing.default_model_key,
    )
}

#[tauri::command]
pub fn unbind_agent_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    unbind_and_restore_agent(
        &state.db,
        &state.master_key,
        agent_id,
        &pools,
        &gw.host,
        gw.port,
        gw.global_proxy_detected,
        None,
    )?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn clear_agent_wiring_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
    mode: String,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    clear_agent_wiring(
        &state.db,
        &state.master_key,
        agent_id,
        &pools,
        &gw.host,
        gw.port,
        gw.global_proxy_detected,
        None,
        &mode,
    )?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn clear_all_agent_wiring_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: String,
    stop_gateway: bool,
) -> Result<ClearAllAgentsResult, String> {
    let gw = state.gateway.runtime_state();
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    let mut cleared = Vec::new();
    let mut failed = Vec::new();
    for agent_id in TargetAgentId::all() {
        match clear_agent_wiring(
            &state.db,
            &state.master_key,
            agent_id,
            &pools,
            &gw.host,
            gw.port,
            gw.global_proxy_detected,
            None,
            &mode,
        ) {
            Ok(_) => cleared.push(agent_id.as_str().to_string()),
            Err(e) => failed.push(format!("{}: {}", agent_id.as_str(), e)),
        }
    }
    if stop_gateway {
        state.gateway.stop_server();
    }
    emit_workspace_updated(&app);
    let workspace = build_workspace_state(&state.db, &state.master_key, &state.gateway)?;
    Ok(ClearAllAgentsResult {
        workspace,
        cleared,
        failed,
    })
}

#[tauri::command]
pub fn repair_agent_drift_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    let existing = state
        .db
        .get_stored_agent_bindings()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|b| b.agent_id == agent_id)
        .ok_or_else(|| "Agent 尚未挂载任何号池".to_string())?;
    apply_agent_wiring(
        &state.db,
        &state.master_key,
        agent_id,
        &existing.mounted_pool_ids,
        existing.default_model_key.as_deref(),
        &existing.options,
        &pools,
        &gw.host,
        gw.port,
        gw.global_proxy_detected,
        None,
    )?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub async fn restart_agent_client_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    let db = Arc::clone(&state.db);
    let host = gw.host.clone();
    let port = gw.port;
    let global_proxy = gw.global_proxy_detected;
    tokio::task::spawn_blocking(move || {
        restart_agent_process(&db, agent_id, &pools, &host, port, global_proxy, None)
    })
    .await
    .map_err(|e| format!("重启客户端任务异常中断：{e}"))??;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn toggle_agent_option_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
    key: String,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    let mut stored = state
        .db
        .get_stored_agent_bindings()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|b| b.agent_id == agent_id)
        .unwrap_or(crate::storage::pools::StoredAgentBinding {
            agent_id,
            mounted_pool_ids: Vec::new(),
            default_model_key: None,
            options: Default::default(),
            status: crate::domain::AgentBindingStatus::Unbound,
            last_hit_at: None,
        });

    match key.as_str() {
        "claudeIsolatedTwin" => {
            stored.options.claude_isolated_twin =
                Some(!stored.options.claude_isolated_twin.unwrap_or(false));
        }
        "codexPreserveTable" => {
            stored.options.codex_preserve_table =
                Some(!stored.options.codex_preserve_table.unwrap_or(true));
        }
        "opencodeSchemaVersion" => {
            let next = if stored.options.opencode_schema_version.as_deref() == Some("v1") {
                "v2"
            } else {
                "v1"
            };
            stored.options.opencode_schema_version = Some(next.to_string());
        }
        _ => {}
    }

    if stored.mounted_pool_ids.is_empty() {
        state
            .db
            .upsert_agent_binding(&stored)
            .map_err(|e| e.to_string())?;
    } else {
        apply_agent_wiring(
            &state.db,
            &state.master_key,
            agent_id,
            &stored.mounted_pool_ids,
            stored.default_model_key.as_deref(),
            &stored.options,
            &pools,
            &gw.host,
            gw.port,
            gw.global_proxy_detected,
            None,
        )?;
    }
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn detect_agent_environment_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PoolWorkspaceState, String> {
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[tauri::command]
pub fn run_pool_simulation_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    input: RunPoolSimulationInput,
) -> Result<RunPoolSimulationResult, String> {
    let res = state
        .gateway
        .run_simulation(&state.db, &state.master_key, &input)?;
    emit_workspace_updated(&app);
    Ok(res)
}

#[tauri::command]
pub async fn set_agent_tool_models_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
    tool_id: String,
    model_ids: Vec<String>,
    default_model_key: Option<String>,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let (pool_id, binding) = state
        .db
        .ensure_agent_dedicated_pool(&state.master_key, agent_id)
        .map_err(|e| e.to_string())?;
    state
        .db
        .set_pool_tool_models(&state.master_key, &pool_id, &tool_id, &model_ids)
        .map_err(|e| e.to_string())?;
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    let effective_default = default_model_key.or(binding.default_model_key);
    let _ = apply_agent_wiring(
        &state.db,
        &state.master_key,
        agent_id,
        &[pool_id],
        effective_default.as_deref(),
        &binding.options,
        &pools,
        &gw.host,
        gw.port,
        gw.global_proxy_detected,
        None,
    )?;
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn update_agent_tool_schedule_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
    tool_id: String,
    ordered_account_ids: Vec<String>,
    enabled_account_ids: Vec<String>,
    strategy: Option<PoolStrategy>,
    skip_active_client_account: Option<bool>,
    cooldown_seconds: Option<u64>,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let (pool_id, binding) = state
        .db
        .ensure_agent_dedicated_pool(&state.master_key, agent_id)
        .map_err(|e| e.to_string())?;
    state
        .db
        .update_pool_tool_account_schedule(
            &state.master_key,
            &pool_id,
            &tool_id,
            &ordered_account_ids,
            &enabled_account_ids,
            strategy,
            skip_active_client_account,
            cooldown_seconds,
        )
        .map_err(|e| e.to_string())?;
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    if !binding.mounted_pool_ids.is_empty() {
        let _ = apply_agent_wiring(
            &state.db,
            &state.master_key,
            agent_id,
            &[pool_id],
            binding.default_model_key.as_deref(),
            &binding.options,
            &pools,
            &gw.host,
            gw.port,
            gw.global_proxy_detected,
            None,
        )?;
    }
    emit_workspace_updated(&app);
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
}
