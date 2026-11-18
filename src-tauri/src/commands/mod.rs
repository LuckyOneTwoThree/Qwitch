use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::adapter::{
    qoder::QoderAdapter, workbuddy::WorkBuddyAdapter, AdapterError, AdapterManifest, ToolAdapter,
};
use crate::domain::{
    AccountStatus, AccountSummary, ActionLogItem, AppSettingsDto, QuotaSnapshot, UnifiedErrorCode,
};
use crate::platform::PlatformHost;
use crate::scheduler::{AntiRiskScheduler, JitterConfig};
use crate::storage::QwitchDatabase;

const SETTINGS_KEY: &str = "app.settings";

fn load_app_settings(db: &QwitchDatabase) -> AppSettingsDto {
    db.get_setting(SETTINGS_KEY)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<AppSettingsDto>(&json).ok())
        .unwrap_or_default()
}

fn resolve_client_executable(
    host: &dyn PlatformHost,
    settings: &AppSettingsDto,
    tool_id: &str,
) -> Option<std::path::PathBuf> {
    if let Some(custom) = settings
        .custom_client_paths
        .get(tool_id)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let p = std::path::PathBuf::from(custom);
        if p.exists() {
            return Some(p);
        }
    }
    host.detect_client_executable(tool_id)
}

pub struct AppState {
    pub db: Arc<QwitchDatabase>,
    pub master_key: [u8; 32],
    pub host: Arc<dyn PlatformHost>,
    pub adapters: Arc<HashMap<String, Arc<dyn ToolAdapter>>>,
    pub scheduler: Arc<AntiRiskScheduler>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientHostStatus {
    pub tool_id: String,
    pub executable_path: Option<String>,
    pub config_dir: Option<String>,
    pub is_running: bool,
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) {
    if let Some(tray) = app.get_webview_window("tray") {
        let _ = tray.hide();
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

#[tauri::command]
pub fn list_adapters(state: State<'_, AppState>) -> Vec<AdapterManifest> {
    let mut list: Vec<AdapterManifest> = state
        .adapters
        .values()
        .map(|a| a.manifest().clone())
        .collect();
    list.sort_by(|a, b| b.is_builtin.cmp(&a.is_builtin).then(a.id.cmp(&b.id)));
    list
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchProgressEvent {
    pub account_id: String,
    pub phase: String,
}

#[tauri::command]
pub fn get_app_settings(state: State<'_, AppState>) -> Result<AppSettingsDto, String> {
    Ok(load_app_settings(&state.db))
}

#[tauri::command]
pub fn save_app_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettingsDto,
) -> Result<AppSettingsDto, String> {
    if settings.jitter_min_ms > settings.jitter_max_ms {
        return Err("最小抖动间隔不能大于最大抖动间隔".into());
    }
    let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    state
        .db
        .set_setting(SETTINGS_KEY, &json)
        .map_err(|e| e.to_string())?;
    let _ = app.emit("qwitch://settings-updated", &settings);
    Ok(settings)
}

#[tauri::command]
pub fn detect_client_status(
    state: State<'_, AppState>,
    tool_id: String,
) -> Result<ClientHostStatus, String> {
    let settings = load_app_settings(&state.db);
    let executable_path = resolve_client_executable(state.host.as_ref(), &settings, &tool_id)
        .map(|p| p.to_string_lossy().to_string());
    let config_dir = state
        .host
        .client_config_dir(&tool_id)
        .ok()
        .map(|p| p.to_string_lossy().to_string());
    let is_running = state
        .host
        .is_client_running(&tool_id)
        .map_err(|e| e.to_string())?;

    Ok(ClientHostStatus {
        tool_id,
        executable_path,
        config_dir,
        is_running,
    })
}

#[tauri::command]
pub fn list_accounts(
    state: State<'_, AppState>,
    tool_id: Option<String>,
) -> Result<Vec<AccountSummary>, String> {
    state
        .db
        .list_accounts(tool_id.as_deref())
        .map_err(|e| e.to_string())
}

fn resolve_account_label(
    requested: &str,
    adapter_name: &str,
    envelope: &crate::domain::CredentialEnvelope,
) -> String {
    let trimmed = requested.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    if let Some(name) = envelope.display_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return name.to_string();
    }
    if let Some(email) = envelope.display_email.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return email.to_string();
    }
    if !envelope.uid.trim().is_empty() {
        return envelope.uid.clone();
    }
    format!("{adapter_name} 账号")
}

#[tauri::command]
pub async fn import_local_account(
    app: AppHandle,
    state: State<'_, AppState>,
    tool_id: String,
    label: String,
    target_account_id: Option<String>,
) -> Result<AccountSummary, String> {
    let adapter = state
        .adapters
        .get(&tool_id)
        .ok_or_else(|| format!("适配器 {tool_id} 尚未注册"))?;

    let envelope = adapter
        .import_from_local(state.host.as_ref())
        .await
        .map_err(|e| e.to_string())?;

    let target_id = target_account_id.filter(|s| !s.trim().is_empty());
    let summary = if let Some(ref acc_id) = target_id {
        let label_override = (!label.trim().is_empty()).then_some(label.as_str());
        state
            .db
            .bind_account_credential(&state.master_key, acc_id, label_override, &envelope)
            .map_err(|e| e.to_string())?
    } else {
        let effective_label = resolve_account_label(&label, &adapter.manifest().name, &envelope);
        state
            .db
            .upsert_account(&state.master_key, &effective_label, &envelope)
            .map_err(|e| e.to_string())?
    };

    let cred_for_quota = state
        .db
        .get_account_credential(&state.master_key, &summary.id)
        .map(|(_, env)| env)
        .unwrap_or(envelope);
    if let Ok(snapshot) = adapter.inspect_quota(&cred_for_quota).await {
        let _ = state.db.save_quota_snapshot(&summary.id, &snapshot);
    }

    let _ = state.db.set_active_on_client(&tool_id, &summary.id);

    let log_msg = if target_id.is_some() {
        format!("补充/更新本机快照 {} ({})", summary.label, summary.uid_masked)
    } else {
        format!("从本机导入 {} ({})", summary.label, summary.uid_masked)
    };
    let _ = state.db.append_action_log(
        Some(&summary.id),
        &tool_id,
        "import",
        "success",
        None,
        &log_msg,
    );
    let _ = app.emit("qwitch://account-updated", &summary.id);
    state
        .db
        .get_account_summary(&summary.id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_account_by_token(
    app: AppHandle,
    state: State<'_, AppState>,
    tool_id: String,
    label: String,
    raw_input: String,
    target_account_id: Option<String>,
) -> Result<AccountSummary, String> {
    let adapter = state
        .adapters
        .get(&tool_id)
        .ok_or_else(|| format!("适配器 {tool_id} 尚未注册"))?;

    let envelope = adapter
        .parse_manual_token(&raw_input)
        .await
        .map_err(|e| e.to_string())?;

    let target_id = target_account_id.filter(|s| !s.trim().is_empty());
    let summary = if let Some(ref acc_id) = target_id {
        let label_override = (!label.trim().is_empty()).then_some(label.as_str());
        state
            .db
            .bind_account_credential(&state.master_key, acc_id, label_override, &envelope)
            .map_err(|e| e.to_string())?
    } else {
        let effective_label = resolve_account_label(&label, &adapter.manifest().name, &envelope);
        state
            .db
            .upsert_account(&state.master_key, &effective_label, &envelope)
            .map_err(|e| e.to_string())?
    };

    let cred_for_quota = state
        .db
        .get_account_credential(&state.master_key, &summary.id)
        .map(|(_, env)| env)
        .unwrap_or(envelope);
    if let Ok(snapshot) = adapter.inspect_quota(&cred_for_quota).await {
        let _ = state.db.save_quota_snapshot(&summary.id, &snapshot);
    }

    let log_msg = if target_id.is_some() {
        format!("补充/更新凭据 {} ({})", summary.label, summary.uid_masked)
    } else {
        format!("通过手动凭据添加 {} ({})", summary.label, summary.uid_masked)
    };
    let _ = state.db.append_action_log(
        Some(&summary.id),
        &tool_id,
        "import",
        "success",
        None,
        &log_msg,
    );
    let _ = app.emit("qwitch://account-updated", &summary.id);
    state
        .db
        .get_account_summary(&summary.id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn refresh_account_quota(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<QuotaSnapshot, String> {
    let (summary, cred) = state
        .db
        .get_account_credential(&state.master_key, &account_id)
        .map_err(|e| e.to_string())?;

    let adapter = state
        .adapters
        .get(&summary.tool_id)
        .ok_or_else(|| format!("适配器 {} 尚未注册", summary.tool_id))?;

    match adapter.inspect_quota(&cred).await {
        Ok(snapshot) => {
            let _ = state.db.save_quota_snapshot(&summary.id, &snapshot);
            let _ = state.db.append_action_log(
                Some(&summary.id),
                &summary.tool_id,
                "query",
                "success",
                None,
                &format!("已刷新 {} ({}) 的最新额度", summary.label, summary.uid_masked),
            );
            let _ = app.emit("qwitch://account-updated", &summary.id);
            Ok(snapshot)
        }
        Err(AdapterError::Unified { code, message }) => {
            if matches!(code, UnifiedErrorCode::AuthExpired) {
                let _ = state
                    .db
                    .update_account_status(&summary.id, AccountStatus::Expired);
            } else if matches!(
                code,
                UnifiedErrorCode::RiskTriggered | UnifiedErrorCode::RateLimited
            ) {
                let _ = state
                    .db
                    .update_account_status(&summary.id, AccountStatus::RiskPaused);
            }
            let _ = state.db.append_action_log(
                Some(&summary.id),
                &summary.tool_id,
                "query",
                "failed",
                Some(&code),
                &format!("{} 查询失败：{message}", summary.label),
            );
            let _ = app.emit("qwitch://account-updated", &summary.id);
            Err(message)
        }
    }
}

#[tauri::command]
pub async fn switch_desktop_account(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    force_kill: bool,
) -> Result<(), String> {
    let (summary, cred) = state
        .db
        .get_account_credential(&state.master_key, &account_id)
        .map_err(|e| e.to_string())?;

    let adapter = state
        .adapters
        .get(&summary.tool_id)
        .ok_or_else(|| format!("适配器 {} 尚未注册", summary.tool_id))?;

    let running = state
        .host
        .is_client_running(&summary.tool_id)
        .map_err(|e| e.to_string())?;

    if running && !force_kill {
        return Err("CLIENT_RUNNING".into());
    }

    if running {
        let _ = app.emit(
            "qwitch://switch-progress",
            SwitchProgressEvent {
                account_id: summary.id.clone(),
                phase: "closing".into(),
            },
        );
        state
            .host
            .terminate_client_gracefully(&summary.tool_id, 5000)
            .await
            .map_err(|e| e.to_string())?;
    }

    let _ = app.emit(
        "qwitch://switch-progress",
        SwitchProgressEvent {
            account_id: summary.id.clone(),
            phase: "writing".into(),
        },
    );

    adapter
        .apply_session(state.host.as_ref(), &cred)
        .await
        .map_err(|e| e.to_string())?;

    state
        .db
        .set_active_on_client(&summary.tool_id, &summary.id)
        .map_err(|e| e.to_string())?;

    let _ = app.emit(
        "qwitch://switch-progress",
        SwitchProgressEvent {
            account_id: summary.id.clone(),
            phase: "launching".into(),
        },
    );

    let settings = load_app_settings(&state.db);
    if let Some(exe) = resolve_client_executable(state.host.as_ref(), &settings, &summary.tool_id) {
        let _ = state.host.launch_client(&exe, &[]);
    }

    let _ = state.db.append_action_log(
        Some(&summary.id),
        &summary.tool_id,
        "switch",
        "success",
        None,
        &format!("已将本机客户端切换至 {} ({})", summary.label, summary.uid_masked),
    );
    let _ = app.emit(
        "qwitch://switch-progress",
        SwitchProgressEvent {
            account_id: summary.id.clone(),
            phase: "done".into(),
        },
    );
    let _ = app.emit("qwitch://account-updated", &summary.id);
    Ok(())
}

#[tauri::command]
pub fn delete_account(state: State<'_, AppState>, account_id: String) -> Result<(), String> {
    state
        .db
        .delete_account(&account_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_account_status(
    state: State<'_, AppState>,
    account_id: String,
    status: AccountStatus,
) -> Result<(), String> {
    state
        .db
        .update_account_status(&account_id, status)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn claim_daily_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    account_ids: Vec<String>,
) -> Result<(), String> {
    let settings = load_app_settings(&state.db);
    let jitter = JitterConfig {
        min_ms: settings.jitter_min_ms,
        max_ms: settings.jitter_max_ms.max(settings.jitter_min_ms),
    };
    let db = Arc::clone(&state.db);
    let master_key = state.master_key;
    let adapters = Arc::clone(&state.adapters);
    let scheduler = Arc::clone(&state.scheduler);

    tokio::spawn(async move {
        let _ = scheduler
            .run_daily_claim_batch(
                &app,
                &db,
                &master_key,
                &adapters,
                account_ids,
                jitter,
            )
            .await;
    });

    Ok(())
}

#[tauri::command]
pub fn get_action_logs(
    state: State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<ActionLogItem>, String> {
    state
        .db
        .list_action_logs(limit.unwrap_or(50), offset.unwrap_or(0))
        .map_err(|e| e.to_string())
}

const OAUTH_CAPTURE_JS: &str = r#"
(function() {
  if (window.__qwitchOAuthInjected) return;
  window.__qwitchOAuthInjected = true;
  let captured = false;

  const ACC_KEY = '__qwitch_oauth_acc_v2';
  function loadAcc() {
    try {
      const raw = window.sessionStorage.getItem(ACC_KEY);
      return raw ? JSON.parse(raw) : {};
    } catch (_) {
      return {};
    }
  }
  function saveAcc(acc) {
    try {
      window.sessionStorage.setItem(ACC_KEY, JSON.stringify(acc));
    } catch (_) {}
  }

  function decodeJwtPayload(token) {
    if (typeof token !== 'string') return null;
    const parts = token.split('.');
    if (parts.length !== 3) return null;
    try {
      const b64 = parts[1].replace(/-/g, '+').replace(/_/g, '/');
      const json = decodeURIComponent(atob(b64).split('').map(c => '%' + ('00' + c.charCodeAt(0).toString(16)).slice(-2)).join(''));
      return JSON.parse(json);
    } catch (_) {
      return null;
    }
  }

  function isValidUid(val) {
    if (val === null || val === undefined) return false;
    const s = String(val).trim();
    if (!s || s.length < 2) return false;
    if (/^(0|-1|null|undefined|none|guest|anonymous)$/i.test(s)) return false;
    return true;
  }

  function isLikelyAuthToken(val) {
    if (typeof val !== 'string') return false;
    const s = val.trim();
    if (s.length < 24 || /\s/.test(s)) return false;
    if (/^(qr_|qrcode_|csrf_|anon_|guest_)/i.test(s)) return false;
    return true;
  }

  function isOnLoginPage() {
    const h = window.location.hostname || '';
    const p = window.location.pathname || '';
    if (/douyin\.com|doubao\.com|feishu\.cn|larkoffice\.com|weixin\.qq\.com|aliyun\.com|apple\.com|github\.com|google\.com/i.test(h)) return true;
    if (/\/login|\/sign-in|\/signin|\/oauth/i.test(p)) return true;
    return false;
  }

  function isValidDisplayName(val) {
    if (typeof val !== 'string') return false;
    const s = val.trim();
    if (!s || s.length > 64) return false;
    if (/^(success|ok|okay|error|fail|failed|true|false|null|undefined|none|unknown|trae|douyin|doubao|qoder|workbuddy|guest|anonymous|resource|data|result|message|status|code|user|account|info|default|normal)$/i.test(s)) {
      return false;
    }
    return true;
  }

  function collectFields(obj, depth, acc) {
    if (!obj || typeof obj !== 'object' || (depth || 0) > 5) return;
    const d = depth || 0;

    for (const key of ['accessToken', 'access_token', 'jwt', 'idToken', 'id_token', 'Token', 'token']) {
      if (isLikelyAuthToken(obj[key])) {
        acc.token = obj[key].trim();
        break;
      }
    }

    for (const key of ['userId', 'uid', 'user_id', 'UserId', 'uin', 'sub', 'accountId', 'memberId']) {
      if (isValidUid(obj[key])) {
        acc.userId = String(obj[key]).trim();
        break;
      }
    }

    for (const key of ['email', 'Email', 'mail']) {
      if (typeof obj[key] === 'string' && obj[key].includes('@')) {
        acc.email = obj[key].trim();
        break;
      }
    }

    for (const key of ['ScreenName', 'screenName', 'NickName', 'nickName', 'nickname', 'username', 'userName', 'displayName']) {
      if (isValidDisplayName(obj[key])) {
        acc.name = obj[key].trim();
        break;
      }
    }

    for (const key of ['refreshToken', 'refresh_token']) {
      if (isLikelyAuthToken(obj[key])) {
        acc.refreshToken = obj[key].trim();
        break;
      }
    }

    if (Array.isArray(obj)) {
      for (let i = 0; i < Math.min(obj.length, 5); i++) {
        collectFields(obj[i], d + 1, acc);
      }
      return;
    }

    for (const k of Object.keys(obj)) {
      if (obj[k] && typeof obj[k] === 'object') {
        collectFields(obj[k], d + 1, acc);
      }
    }
  }

  function tryEmit(sourceObj, fromTrustedEndpoint) {
    if (captured) return;
    // 仍在登录页或第三方 OAuth 页时，仅放行明确已登录的 GetUserToken / iCubeAuthInfo 凭据
    if (isOnLoginPage() && !fromTrustedEndpoint) return;

    const acc = loadAcc();
    if (sourceObj && typeof sourceObj === 'object') {
      collectFields(sourceObj, 0, acc);
    }

    if (acc.token && !acc.userId) {
      const claims = decodeJwtPayload(acc.token);
      if (claims && typeof claims === 'object') {
        const claimUid = claims.userId || claims.uid || claims.user_id || claims.sub || claims.uin;
        if (isValidUid(claimUid)) acc.userId = String(claimUid).trim();
        if (!acc.email && typeof claims.email === 'string' && claims.email.includes('@')) {
          acc.email = claims.email.trim();
        }
        if (!acc.name && typeof (claims.name || claims.nickname) === 'string') {
          acc.name = (claims.name || claims.nickname).trim();
        }
      }
    }

    saveAcc(acc);

    // 必须同时具备有效 token 与真实用户身份 (userId 或 email)
    if (acc.token && (acc.userId || acc.email)) {
      captured = true;
      try { window.sessionStorage.removeItem(ACC_KEY); } catch (_) {}
      const payloadStr = JSON.stringify({
        token: acc.token,
        userId: acc.userId || undefined,
        email: acc.email || undefined,
        name: acc.name || undefined,
        refreshToken: acc.refreshToken || undefined,
      });
      window.location.href = 'https://qwitch.local/oauth-callback?payload=' + encodeURIComponent(payloadStr);
    }
  }

  function isIgnoredUrl(url) {
    return /qrcode|qr_code|get_qr|check_qr|qrconnect|csrf|captcha|config|anon|guest|metrics|telemetry|report|tea\/v|slardar|passport\/web\/get/i.test(url);
  }

  function isTrustedAuthUrl(url) {
    return /GetUserToken|iCubeAuthInfo|\/v2\/billing\/meter\/|\/sash\/api\/v1\/me\//i.test(url);
  }

  function inspectResponseText(url, text) {
    if (captured || !text || isIgnoredUrl(url)) return;
    const trimmed = text.trim();
    if (trimmed[0] !== '{' && trimmed[0] !== '[') return;
    try {
      const json = JSON.parse(trimmed);
      tryEmit(json, isTrustedAuthUrl(url));
    } catch (_) {}
  }

  const origFetch = window.fetch;
  window.fetch = async function(...args) {
    const resp = await origFetch.apply(this, args);
    try {
      const url = String(args[0] && args[0].url ? args[0].url : args[0] || '');
      if (!isIgnoredUrl(url) && /GetUserToken|GetUserInfo|userinfo|user\/info|account|session|campaigns|checkin|billing/i.test(url)) {
        resp.clone().text().then(txt => inspectResponseText(url, txt)).catch(() => {});
      }
    } catch (_) {}
    return resp;
  };

  const origOpen = XMLHttpRequest.prototype.open;
  const origSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function(method, url) {
    this.__qwitchUrl = String(url || '');
    return origOpen.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function() {
    this.addEventListener('load', function() {
      try {
        const url = this.__qwitchUrl || '';
        if (!isIgnoredUrl(url) && /GetUserToken|GetUserInfo|userinfo|user\/info|account|session|campaigns|checkin|billing/i.test(url)) {
          inspectResponseText(url, this.responseText);
        }
      } catch (_) {}
    });
    return origSend.apply(this, arguments);
  };

  function scanStorage() {
    if (captured) return;
    for (const store of [window.localStorage, window.sessionStorage]) {
      try {
        if (!store) continue;
        for (let i = 0; i < store.length; i++) {
          const k = store.key(i);
          if (!k || k === ACC_KEY) continue;
          // 仅扫描明确的工具登录态键名，绝不扫描埋点/统计键 (__tea_cache 等)
          if (!/iCubeAuthInfo|workbuddy|qoder|auth_info|user_info/i.test(k)) continue;
          const v = store.getItem(k);
          if (!v) continue;
          const trimmed = v.trim();
          if (trimmed[0] === '{') {
            try {
              tryEmit(JSON.parse(trimmed), k.startsWith('iCubeAuthInfo'));
              if (captured) return;
            } catch (_) {}
          }
        }
      } catch (_) {}
    }
  }

  // 将第三方登录（如抖音/微信/阿里云）通过 window.open 或 target="_blank" 打开的页面直接引导到当前窗口显示
  window.open = function(url) {
    if (typeof url === 'string' && url && url !== 'about:blank') {
      window.location.href = url;
      return window;
    }
    return {
      closed: false,
      close: function() {},
      focus: function() {},
      location: {
        set href(u) { if (typeof u === 'string' && u) window.location.href = u; },
        get href() { return window.location.href; },
        replace: function(u) { if (typeof u === 'string' && u) window.location.replace(u); },
        assign: function(u) { if (typeof u === 'string' && u) window.location.assign(u); }
      }
    };
  };

  document.addEventListener('click', function(e) {
    const a = e.target && e.target.closest ? e.target.closest('a[target="_blank"]') : null;
    if (a && a.href && /^https?:\/\//i.test(a.href)) {
      e.preventDefault();
      window.location.href = a.href;
    }
  }, true);

  setInterval(scanStorage, 1000);
})();
"#;

const POPUP_REDIRECT_JS: &str = r#"
(function() {
  if (window.__qwitchPopupInjected) return;
  window.__qwitchPopupInjected = true;
  window.open = function(url) {
    if (typeof url === 'string' && url && url !== 'about:blank') {
      window.location.href = url;
      return window;
    }
    return {
      closed: false,
      close: function() {},
      focus: function() {},
      location: {
        set href(u) { if (typeof u === 'string' && u) window.location.href = u; },
        get href() { return window.location.href; },
        replace: function(u) { if (typeof u === 'string' && u) window.location.replace(u); },
        assign: function(u) { if (typeof u === 'string' && u) window.location.assign(u); }
      }
    };
  };
  document.addEventListener('click', function(e) {
    const a = e.target && e.target.closest ? e.target.closest('a[target="_blank"]') : null;
    if (a && a.href && /^https?:\/\//i.test(a.href)) {
      e.preventDefault();
      window.location.href = a.href;
    }
  }, true);
})();
"#;

async fn persist_oauth_envelope(
    app: &AppHandle,
    db: &Arc<QwitchDatabase>,
    host: &Arc<dyn PlatformHost>,
    master_key: &[u8; 32],
    adapter: &Arc<dyn ToolAdapter>,
    tool_id: &str,
    label: &str,
    target_account_id: Option<&str>,
    envelope: crate::domain::CredentialEnvelope,
) {
    let mut matched_local = false;
    let envelope_to_save = match adapter.import_from_local(host.as_ref()).await {
        Ok(local_env)
            if !local_env.uid.trim().is_empty() && local_env.uid == envelope.uid =>
        {
            matched_local = true;
            local_env.merge_from(&envelope)
        }
        _ => envelope,
    };

    let save_res = if let Some(acc_id) = target_account_id {
        let label_override = (!label.is_empty()).then_some(label);
        db.bind_account_credential(master_key, acc_id, label_override, &envelope_to_save)
    } else {
        let effective_label = resolve_account_label(label, &adapter.manifest().name, &envelope_to_save);
        db.upsert_account(master_key, &effective_label, &envelope_to_save)
    };
    match save_res {
        Ok(summary) => {
            if matched_local {
                let _ = db.set_active_on_client(tool_id, &summary.id);
            }
            let cred_for_quota = db
                .get_account_credential(master_key, &summary.id)
                .map(|(_, env)| env)
                .unwrap_or(envelope_to_save);
            if let Ok(snapshot) = adapter.inspect_quota(&cred_for_quota).await {
                let _ = db.save_quota_snapshot(&summary.id, &snapshot);
            }
            let log_msg = if target_account_id.is_some() {
                format!(
                    "通过内置授权补充/更新 {} ({})",
                    summary.label, summary.uid_masked
                )
            } else {
                format!(
                    "通过内置授权导入 {} ({})",
                    summary.label, summary.uid_masked
                )
            };
            let _ = db.append_action_log(
                Some(&summary.id),
                tool_id,
                "import",
                "success",
                None,
                &log_msg,
            );
            let _ = app.emit("qwitch://account-updated", &summary.id);
        }
        Err(e) => {
            let _ = app.emit("qwitch://oauth-error", format!("保存账号失败: {e}"));
        }
    }
    if let Some(win) = app.get_webview_window("oauth-login") {
        let _ = win.close();
    }
}

#[tauri::command]
pub async fn start_webview_oauth(
    app: AppHandle,
    state: State<'_, AppState>,
    tool_id: String,
    label: String,
    target_account_id: Option<String>,
) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window("oauth-login") {
        let _ = existing.close();
    }

    enum OAuthMode {
        TraeCapture,
        WorkBuddyPoll { state_token: String },
        QoderPkcePoll {
            nonce: String,
            verifier: String,
            machine_id: String,
        },
    }

    let (login_url, mode) = match tool_id.as_str() {
        "trae_cn" => ("https://www.trae.cn/login".to_string(), OAuthMode::TraeCapture),
        "workbuddy" => {
            let (state_token, auth_url) = WorkBuddyAdapter::create_oauth_session().await?;
            (auth_url, OAuthMode::WorkBuddyPoll { state_token })
        }
        "qoder_cn" => {
            let (auth_url, nonce, verifier, machine_id) =
                QoderAdapter::create_pkce_device_flow(state.host.as_ref());
            (
                auth_url,
                OAuthMode::QoderPkcePoll {
                    nonce,
                    verifier,
                    machine_id,
                },
            )
        }
        _ => return Err(format!("工具 {tool_id} 暂不支持内置 WebView 授权")),
    };

    let parsed_url: tauri::Url = login_url.parse().map_err(|e| format!("URL 解析失败: {e}"))?;
    let adapter = state
        .adapters
        .get(&tool_id)
        .cloned()
        .ok_or_else(|| format!("适配器 {tool_id} 尚未注册"))?;

    let db = Arc::clone(&state.db);
    let host_arc = Arc::clone(&state.host);
    let master_key = state.master_key;
    let app_handle = app.clone();
    let tool_id_owned = tool_id.clone();
    let label_owned = label.trim().to_string();
    let target_id_owned = target_account_id.filter(|s| !s.trim().is_empty());

    let app_for_popup = app.clone();
    let init_js = match mode {
        OAuthMode::TraeCapture => OAUTH_CAPTURE_JS,
        OAuthMode::WorkBuddyPoll { .. } | OAuthMode::QoderPkcePoll { .. } => POPUP_REDIRECT_JS,
    };

    let mut builder = WebviewWindowBuilder::new(&app, "oauth-login", WebviewUrl::External(parsed_url))
        .title(format!("Qwitch 内置授权登录 - {}", adapter.manifest().name))
        .inner_size(960.0, 720.0)
        .center()
        .incognito(true)
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .initialization_script(init_js)
        .on_new_window(move |url, _features| {
            if url.as_str() != "about:blank" {
                if let Some(win) = app_for_popup.get_webview_window("oauth-login") {
                    let _ = win.navigate(url);
                }
            }
            tauri::webview::NewWindowResponse::Deny
        });

    if matches!(mode, OAuthMode::TraeCapture) {
        let app_nav = app_handle.clone();
        let db_nav = Arc::clone(&db);
        let host_nav = Arc::clone(&host_arc);
        let adapter_nav = Arc::clone(&adapter);
        let tid_nav = tool_id_owned.clone();
        let lbl_nav = label_owned.clone();
        let target_nav = target_id_owned.clone();

        builder = builder.on_navigation(move |nav_url| {
            if nav_url.host_str() == Some("qwitch.local") {
                let raw_payload = nav_url
                    .query_pairs()
                    .find(|(k, _)| k == "payload")
                    .or_else(|| nav_url.query_pairs().find(|(k, _)| k == "token"))
                    .map(|(_, v)| v.to_string());

                if let Some(payload_str) = raw_payload {
                    let app_clone = app_nav.clone();
                    let db_clone = Arc::clone(&db_nav);
                    let host_clone = Arc::clone(&host_nav);
                    let adapter_clone = Arc::clone(&adapter_nav);
                    let tid = tid_nav.clone();
                    let lbl = lbl_nav.clone();
                    let target_id = target_nav.clone();

                    tauri::async_runtime::spawn(async move {
                        match adapter_clone.parse_manual_token(&payload_str).await {
                            Ok(envelope) => {
                                persist_oauth_envelope(
                                    &app_clone,
                                    &db_clone,
                                    &host_clone,
                                    &master_key,
                                    &adapter_clone,
                                    &tid,
                                    &lbl,
                                    target_id.as_deref(),
                                    envelope,
                                )
                                .await;
                            }
                            Err(e) => {
                                let _ = app_clone.emit(
                                    "qwitch://oauth-error",
                                    format!("解析授权凭据失败: {e}"),
                                );
                                if let Some(win) = app_clone.get_webview_window("oauth-login") {
                                    let _ = win.close();
                                }
                            }
                        }
                    });
                }
                return false;
            }
            true
        });
    }

    builder
        .build()
        .map_err(|e| format!("打开内置授权窗口失败: {e}"))?;

    match mode {
        OAuthMode::TraeCapture => {}
        OAuthMode::WorkBuddyPoll { state_token } => {
            tauri::async_runtime::spawn(async move {
                for _ in 0..180 {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    if app_handle.get_webview_window("oauth-login").is_none() {
                        break;
                    }
                    match WorkBuddyAdapter::poll_oauth_result(&state_token).await {
                        Ok(Some(envelope)) => {
                            persist_oauth_envelope(
                                &app_handle,
                                &db,
                                &host_arc,
                                &master_key,
                                &adapter,
                                &tool_id_owned,
                                &label_owned,
                                target_id_owned.as_deref(),
                                envelope,
                            )
                            .await;
                            break;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            let _ = app_handle.emit("qwitch://oauth-error", err);
                            if let Some(win) = app_handle.get_webview_window("oauth-login") {
                                let _ = win.close();
                            }
                            break;
                        }
                    }
                }
            });
        }
        OAuthMode::QoderPkcePoll {
            nonce,
            verifier,
            machine_id,
        } => {
            tauri::async_runtime::spawn(async move {
                for _ in 0..180 {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    if app_handle.get_webview_window("oauth-login").is_none() {
                        break;
                    }
                    match QoderAdapter::poll_pkce_device_token(&nonce, &verifier, &machine_id).await {
                        Ok(Some(envelope)) => {
                            persist_oauth_envelope(
                                &app_handle,
                                &db,
                                &host_arc,
                                &master_key,
                                &adapter,
                                &tool_id_owned,
                                &label_owned,
                                target_id_owned.as_deref(),
                                envelope,
                            )
                            .await;
                            break;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            let _ = app_handle.emit("qwitch://oauth-error", err);
                            if let Some(win) = app_handle.get_webview_window("oauth-login") {
                                let _ = win.close();
                            }
                            break;
                        }
                    }
                }
            });
        }
    }

    Ok(())
}
