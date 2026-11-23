use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

use crate::agent_wiring::{
    apply_agent_wiring, inspect_all_agent_bindings, restart_agent_process,
    unbind_and_restore_agent,
};
use crate::commands::AppState;
use crate::domain::{
    AccountPool, AgentBinding, CreatePoolInput, MemberTier, PoolWorkspaceState,
    RunPoolSimulationInput, RunPoolSimulationResult, TargetAgentId, UpdatePoolPatch,
};
use crate::gateway::GatewayRuntime;
use crate::storage::QwitchDatabase;

fn build_workspace_state(
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    gateway: &GatewayRuntime,
) -> Result<PoolWorkspaceState, String> {
    let gw_state = gateway.runtime_state();
    let cooldowns = gateway.cooldown_snapshot();
    let pools = db
        .list_pools(master_key, &cooldowns)
        .map_err(|e| e.to_string())?;
    let candidates = db
        .list_upstream_candidates(master_key)
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
pub fn get_pool_workspace_state(state: State<'_, AppState>) -> Result<PoolWorkspaceState, String> {
    build_workspace_state(&state.db, &state.master_key, &state.gateway)
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
pub async fn update_gateway_port_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    port: u16,
) -> Result<PoolWorkspaceState, String> {
    let was_running = state.gateway.runtime_state().running;
    if was_running {
        state.gateway.stop_server();
    }
    state.gateway.set_port(port)?;
    if was_running {
        state
            .gateway
            .start_server(
                Arc::clone(&state.db),
                state.master_key,
                Arc::clone(&state.adapters),
            )
            .await?;
    }
    // 重新同步已挂载 Agent 配置端口
    let ws = build_workspace_state(&state.db, &state.master_key, &state.gateway)?;
    for b in &ws.agent_bindings {
        if !b.mounted_pool_ids.is_empty() {
            let _ = apply_agent_wiring(
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
            );
        }
    }
    emit_workspace_updated(&app);
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
    state
        .db
        .delete_pool(&pool_id)
        .map_err(|e| e.to_string())?;

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
            let _ = apply_agent_wiring(
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
            );
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
    let until = chrono::Utc::now().timestamp_millis() + (pool.cooldown_seconds as i64) * 1000;
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
    let options = existing.as_ref().map(|b| b.options.clone()).unwrap_or_default();
    let effective_default = default_model_key.or_else(|| existing.and_then(|b| b.default_model_key));

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
    update_agent_mounted_pools_cmd(app, state, agent_id, next_mounted, existing.default_model_key)
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
pub fn restart_agent_client_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: TargetAgentId,
) -> Result<PoolWorkspaceState, String> {
    let gw = state.gateway.runtime_state();
    let pools = state
        .db
        .list_pools(&state.master_key, &state.gateway.cooldown_snapshot())
        .map_err(|e| e.to_string())?;
    restart_agent_process(
        &state.db,
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
                Some(!stored.options.claude_isolated_twin.unwrap_or(true));
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
