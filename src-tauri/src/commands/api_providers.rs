use tauri::{AppHandle, Emitter, State};

use crate::domain::{
    AccountSummary, AddApiProviderKeyInput, ApiManualModelEntry, ApiProviderRecord,
    ApiProviderTestInput, ApiProviderTestResult, CredentialPayload, SaveApiProviderInput,
    UpdateApiProviderKeyInput,
};
use crate::plugins::sync_global_registry_snapshot;

use super::AppState;

fn sync_api_providers_to_registry(state: &AppState) -> Result<Vec<ApiProviderRecord>, String> {
    let providers = state
        .db
        .list_api_providers()
        .map_err(|e| e.to_string())?;
    let mut reg = state
        .plugins
        .write()
        .map_err(|_| "插件注册表锁异常".to_string())?;
    reg.sync_api_providers(&providers);
    sync_global_registry_snapshot(&reg);
    Ok(providers)
}

fn emit_api_provider_events(app: &AppHandle) {
    let _ = app.emit("qwitch://api-providers-updated", ());
    let _ = app.emit("qwitch://account-updated", ());
    let _ = app.emit("qwitch://pool-workspace-updated", ());
}

#[tauri::command]
pub fn list_api_providers_cmd(state: State<'_, AppState>) -> Result<Vec<ApiProviderRecord>, String> {
    state.db.list_api_providers().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_api_provider_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SaveApiProviderInput,
) -> Result<ApiProviderRecord, String> {
    let saved = state
        .db
        .upsert_api_provider(&state.master_key, input)
        .map_err(|e| e.to_string())?;
    sync_api_providers_to_registry(&state)?;
    emit_api_provider_events(&app);
    Ok(saved)
}

#[tauri::command]
pub fn delete_api_provider_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state
        .db
        .delete_api_provider(&id)
        .map_err(|e| e.to_string())?;
    sync_api_providers_to_registry(&state)?;
    emit_api_provider_events(&app);
    Ok(())
}

#[tauri::command]
pub fn toggle_api_provider_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<Vec<ApiProviderRecord>, String> {
    state
        .db
        .set_api_provider_enabled(&id, enabled)
        .map_err(|e| e.to_string())?;
    let providers = sync_api_providers_to_registry(&state)?;
    emit_api_provider_events(&app);
    Ok(providers)
}

#[tauri::command]
pub async fn test_api_provider_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    input: ApiProviderTestInput,
) -> Result<ApiProviderTestResult, String> {
    let mut fallback_key: Option<String> = None;
    let mut provider_account_ids: Vec<String> = Vec::new();
    if let Some(pid) = input
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Ok(accounts) = state.db.list_accounts(Some(pid)) {
            for acc in accounts {
                provider_account_ids.push(acc.id.clone());
                if fallback_key.is_none() {
                    if let Ok((_, env)) =
                        state.db.get_account_credential(&state.master_key, &acc.id)
                    {
                        if let CredentialPayload::Token { access_token, .. } = env.payload {
                            if !access_token.trim().is_empty() {
                                fallback_key = Some(access_token);
                            }
                        }
                    }
                }
            }
        }
    }

    let result =
        crate::storage::model_discovery::test_api_provider_connection(&input, fallback_key.as_deref())
            .await;

    if let Some(pid) = input
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = state.db.update_api_provider_test_result(
            pid,
            result.ok,
            result.latency_ms,
            result.error.as_deref(),
        );
        if result.ok && !result.discovered_models.is_empty() {
            for aid in provider_account_ids {
                let _ = state
                    .db
                    .save_cached_account_models(&aid, pid, &result.discovered_models);
            }
        }
        emit_api_provider_events(&app);
    }

    Ok(result)
}

#[tauri::command]
pub fn add_api_provider_key_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    input: AddApiProviderKeyInput,
) -> Result<AccountSummary, String> {
    let summary = state
        .db
        .add_api_provider_key(&state.master_key, input)
        .map_err(|e| e.to_string())?;
    emit_api_provider_events(&app);
    Ok(summary)
}

#[tauri::command]
pub fn update_api_provider_key_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    input: UpdateApiProviderKeyInput,
) -> Result<AccountSummary, String> {
    let summary = state
        .db
        .update_api_provider_key(&state.master_key, input)
        .map_err(|e| e.to_string())?;
    emit_api_provider_events(&app);
    Ok(summary)
}

#[tauri::command]
pub fn save_api_provider_models_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    models: Vec<ApiManualModelEntry>,
) -> Result<ApiProviderRecord, String> {
    let saved = state
        .db
        .save_api_provider_manual_models(&id, models)
        .map_err(|e| e.to_string())?;
    sync_api_providers_to_registry(&state)?;
    emit_api_provider_events(&app);
    Ok(saved)
}
