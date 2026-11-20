use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::adapter::{
    minimax::MiniMaxAdapter, qoder::QoderAdapter, workbuddy::WorkBuddyAdapter, AdapterError,
    ToolAdapter,
};
use crate::domain::UnifiedErrorCode;
use crate::platform::PlatformHost;
use crate::scheduler::AntiRiskScheduler;
use crate::storage::QwitchDatabase;

use super::{is_valid_oauth_uid, record_import_quota_check, AppState};

const OAUTH_CAPTURE_JS: &str = r#"
(function() {
  if (window.__qwitchOAuthInjected) return;
  window.__qwitchOAuthInjected = true;
  let captured = false;

  function isTraeCaptureOrigin() {
    const h = (window.location.hostname || '').toLowerCase();
    return h === 'trae.cn' || h.endsWith('.trae.cn');
  }

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
    if (typeof val !== 'string' && typeof val !== 'number') return false;
    const s = String(val).trim();
    if (!s || s.length < 2 || s.length > 128) return false;
    if (/[\s,\[\]{}"']/.test(s)) return false;
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
    if (captured || !isTraeCaptureOrigin()) return;
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
      window.location.href = 'https://qwitch.local/oauth-callback#nonce=' + encodeURIComponent('__QWITCH_NONCE__') + '&payload=' + encodeURIComponent(payloadStr);
    }
  }

  function isIgnoredUrl(url) {
    return /qrcode|qr_code|get_qr|check_qr|qrconnect|csrf|captcha|config|anon|guest|metrics|telemetry|report|tea\/v|slardar|passport\/web\/get/i.test(url);
  }

  function isTrustedAuthUrl(url) {
    return /GetUserToken|iCubeAuthInfo|\/v2\/billing\/meter\/|\/sash\/api\/v1\/me\//i.test(url);
  }

  function inspectResponseText(url, text) {
    if (captured || !text || !isTraeCaptureOrigin() || isIgnoredUrl(url)) return;
    const trimmed = text.trim();
    if (trimmed[0] !== '{' && trimmed[0] !== '[') return;
    try {
      const json = JSON.parse(trimmed);
      tryEmit(json, isTrustedAuthUrl(url));
    } catch (_) {}
  }

  if (isTraeCaptureOrigin()) {
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
  }

  function scanStorage() {
    if (captured || !isTraeCaptureOrigin()) return;
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

  if (isTraeCaptureOrigin()) {
    setInterval(scanStorage, 1000);
  }
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

#[cfg(target_os = "windows")]
const OAUTH_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
#[cfg(not(target_os = "windows"))]
const OAUTH_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

fn allowed_oauth_hosts(tool_id: &str) -> &'static [&'static str] {
    match tool_id {
        "trae_cn" => &[
            "trae.cn",
            "douyin.com",
            "bytedance.com",
            "snssdk.com",
            "volcengine.com",
            "bytegoofy.com",
            "doubao.com",
            "feishu.cn",
            "larkoffice.com",
            "github.com",
            "apple.com",
            "microsoftonline.com",
            "okta.com",
        ],
        "workbuddy" => &[
            "copilot.tencent.com",
            "workbuddy.cn",
            "tencent.com",
            "qcloud.com",
            "qq.com",
            "weixin.qq.com",
            "wx.qq.com",
            "work.weixin.qq.com",
            "wechat.com",
            "microsoftonline.com",
            "okta.com",
        ],
        "qoder_cn" => &[
            "qoder.cn",
            "qoder.com.cn",
            "aliyun.com",
            "alibabacloud.com",
            "alipay.com",
            "dingtalk.com",
            "taobao.com",
            "alibaba.com",
            "github.com",
            "google.com",
            "microsoftonline.com",
            "okta.com",
        ],
        "minimax_code_cn" => &[
            "minimax.cn",
            "minimaxi.com",
            "xaminim.com",
            // 登录页资源与滑块验证码 iframe（腾讯 TCaptcha 及其 CDN）
            "hailuoai.com",
            "hailuo.ai",
            "qcloud.com",
            "gtimg.com",
            "tencentcs.com",
            "tencentrio.com",
            "qq.com",
            "github.com",
            "google.com",
            "microsoftonline.com",
            "apple.com",
            "weixin.qq.com",
        ],
        _ => &[],
    }
}

fn is_allowed_oauth_url(url: &tauri::Url, allowed_hosts: &[&str]) -> bool {
    if url.as_str() == "about:blank" {
        return true;
    }
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str().map(|h| h.to_ascii_lowercase()) else {
        return false;
    };
    allowed_hosts
        .iter()
        .any(|base| host == *base || host.ends_with(&format!(".{base}")))
}

#[allow(clippy::too_many_arguments)]
async fn persist_oauth_envelope(
    app: &AppHandle,
    db: &Arc<QwitchDatabase>,
    host: &Arc<dyn PlatformHost>,
    master_key: &[u8; 32],
    adapter: &Arc<dyn ToolAdapter>,
    scheduler: &Arc<AntiRiskScheduler>,
    flow_seq: &Arc<AtomicU64>,
    flow_id: u64,
    win_label: &str,
    tool_id: &str,
    label: &str,
    target_account_id: Option<&str>,
    envelope: crate::domain::CredentialEnvelope,
) {
    let close_window = || {
        if flow_seq.load(Ordering::SeqCst) == flow_id {
            if let Some(win) = app.get_webview_window(win_label) {
                let _ = win.close();
            }
        }
    };

    if flow_seq.load(Ordering::SeqCst) != flow_id {
        return;
    }

    if envelope.tool_id != tool_id {
        let _ = app.emit(
            "qwitch://oauth-error",
            format!(
                "授权返回的工具类型不匹配：期望 {tool_id}，实际 {}",
                envelope.tool_id
            ),
        );
        close_window();
        return;
    }

    if !is_valid_oauth_uid(&envelope.uid) {
        let _ = app.emit(
            "qwitch://oauth-error",
            "授权返回的账号 UID 无效（疑似非字符串身份字段），已拒绝导入",
        );
        close_window();
        return;
    }

    let mut matched_local = false;
    let envelope_to_save = match adapter.import_from_local(host.as_ref()).await {
        Ok(local_env) if !local_env.uid.trim().is_empty() && local_env.uid == envelope.uid => {
            matched_local = true;
            local_env.merge_from(&envelope)
        }
        _ => envelope,
    };

    if flow_seq.load(Ordering::SeqCst) != flow_id {
        return;
    }

    let save_res = if let Some(acc_id) = target_account_id {
        let label_override = (!label.is_empty()).then_some(label);
        db.bind_account_credential(master_key, acc_id, label_override, &envelope_to_save)
    } else {
        db.upsert_account(
            master_key,
            (!label.is_empty()).then_some(label),
            &envelope_to_save,
        )
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
            let _ = app.emit("qwitch://logs-updated", ());
            close_window();

            if summary.has_api_token && adapter.manifest().capabilities.supports_quota_query {
                let app_bg = app.clone();
                let db_bg = Arc::clone(db);
                let key_bg = *master_key;
                let adapter_bg = Arc::clone(adapter);
                let scheduler_bg = Arc::clone(scheduler);
                tauri::async_runtime::spawn(async move {
                    let _guard = scheduler_bg.queue_lock().lock_owned().await;
                    record_import_quota_check(
                        &db_bg,
                        &key_bg,
                        &adapter_bg,
                        &summary,
                        &cred_for_quota,
                    )
                    .await;
                    let _ = app_bg.emit("qwitch://account-updated", &summary.id);
                    let _ = app_bg.emit("qwitch://logs-updated", ());
                });
            }
        }
        Err(e) => {
            let _ = app.emit("qwitch://oauth-error", format!("保存账号失败: {e}"));
            close_window();
        }
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
    let flow_id = state.oauth_flow_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let flow_nonce = uuid::Uuid::new_v4().to_string();
    let win_label = format!("oauth-login-{flow_id}");

    for (existing_label, existing_win) in app.webview_windows() {
        if existing_label == "oauth-login" || existing_label.starts_with("oauth-login-") {
            let _ = existing_win.close();
        }
    }

    enum OAuthMode {
        TraeCapture,
        WorkBuddyPoll {
            state_token: String,
        },
        QoderPkcePoll {
            nonce: String,
            verifier: String,
            machine_id: String,
        },
        MiniMaxDevicePoll {
            auth: crate::adapter::minimax::MiniMaxDeviceAuth,
        },
    }

    let (login_url, mode) = match tool_id.as_str() {
        "trae_cn" => (
            "https://www.trae.cn/login".to_string(),
            OAuthMode::TraeCapture,
        ),
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
        "minimax_code_cn" => {
            let auth = MiniMaxAdapter::create_device_authorization()
                .await
                .map_err(|e| format!("{e}"))?;
            let auth_url = auth.login_url().to_string();
            (auth_url, OAuthMode::MiniMaxDevicePoll { auth })
        }
        _ => return Err(format!("工具 {tool_id} 暂不支持内置 WebView 授权")),
    };

    let allowed_hosts = allowed_oauth_hosts(&tool_id);
    let parsed_url: tauri::Url = login_url
        .parse()
        .map_err(|e| format!("URL 解析失败: {e}"))?;
    if !is_allowed_oauth_url(&parsed_url, allowed_hosts) {
        return Err(format!("授权入口地址不在官方 HTTPS 白名单内: {parsed_url}"));
    }

    let adapter = state
        .adapters
        .get(&tool_id)
        .cloned()
        .ok_or_else(|| format!("适配器 {tool_id} 尚未注册"))?;

    let db = Arc::clone(&state.db);
    let host_arc = Arc::clone(&state.host);
    let scheduler_arc = Arc::clone(&state.scheduler);
    let master_key = state.master_key;
    let flow_seq = Arc::clone(&state.oauth_flow_seq);
    let app_handle = app.clone();
    let tool_id_owned = tool_id.clone();
    let label_owned = label.trim().to_string();
    let target_id_owned = target_account_id.filter(|s| !s.trim().is_empty());

    let app_for_popup = app.clone();
    let win_label_popup = win_label.clone();
    let init_js: String = match mode {
        OAuthMode::TraeCapture => OAUTH_CAPTURE_JS.replace("__QWITCH_NONCE__", &flow_nonce),
        OAuthMode::WorkBuddyPoll { .. } | OAuthMode::QoderPkcePoll { .. } => {
            POPUP_REDIRECT_JS.to_string()
        }
        // MiniMax 官方登录页依赖自身 window.open / 验证码 iframe，注入任何脚本都可能干扰
        OAuthMode::MiniMaxDevicePoll { .. } => String::new(),
    };

    let mut builder = WebviewWindowBuilder::new(&app, &win_label, WebviewUrl::External(parsed_url))
        .title(format!("Qwitch 内置授权登录 - {}", adapter.manifest().name))
        .inner_size(960.0, 720.0)
        .center()
        .incognito(true)
        .user_agent(OAUTH_USER_AGENT)
        .on_new_window(move |url, _features| {
            if url.as_str() != "about:blank" && is_allowed_oauth_url(&url, allowed_hosts) {
                if let Some(win) = app_for_popup.get_webview_window(&win_label_popup) {
                    let _ = win.navigate(url);
                }
            }
            tauri::webview::NewWindowResponse::Deny
        });

    if !init_js.is_empty() {
        builder = builder.initialization_script(&init_js);
    }

    if matches!(mode, OAuthMode::TraeCapture) {
        let app_nav = app_handle.clone();
        let db_nav = Arc::clone(&db);
        let scheduler_nav = Arc::clone(&scheduler_arc);
        let host_nav = Arc::clone(&host_arc);
        let adapter_nav = Arc::clone(&adapter);
        let flow_seq_nav = Arc::clone(&flow_seq);
        let expected_nonce = flow_nonce.clone();
        let tid_nav = tool_id_owned.clone();
        let lbl_nav = label_owned.clone();
        let target_nav = target_id_owned.clone();
        let win_label_nav = win_label.clone();

        builder = builder.on_navigation(move |nav_url| {
            if nav_url.host_str() == Some("qwitch.local") {
                if nav_url.scheme() != "https"
                    || nav_url.path() != "/oauth-callback"
                    || flow_seq_nav.load(Ordering::SeqCst) != flow_id
                {
                    return false;
                }
                let fragment_url = nav_url
                    .fragment()
                    .filter(|f| !f.is_empty())
                    .and_then(|frag| {
                        reqwest::Url::parse(&format!("https://qwitch.local/?{frag}")).ok()
                    });
                let find_param = |key: &str| -> Option<String> {
                    if let Some(ref fu) = fragment_url {
                        if let Some((_, v)) = fu.query_pairs().find(|(k, _)| k == key) {
                            return Some(v.to_string());
                        }
                    }
                    nav_url
                        .query_pairs()
                        .find(|(k, _)| k == key)
                        .map(|(_, v)| v.to_string())
                };

                let callback_nonce = find_param("nonce");
                if callback_nonce.as_deref() != Some(expected_nonce.as_str()) {
                    let _ = app_nav.emit(
                        "qwitch://oauth-error",
                        "内置授权回调校验失败（会话状态不匹配），已拒绝该请求",
                    );
                    return false;
                }

                let raw_payload = find_param("payload").or_else(|| find_param("token"));

                if let Some(payload_str) = raw_payload {
                    let app_clone = app_nav.clone();
                    let db_clone = Arc::clone(&db_nav);
                    let scheduler_clone = Arc::clone(&scheduler_nav);
                    let host_clone = Arc::clone(&host_nav);
                    let adapter_clone = Arc::clone(&adapter_nav);
                    let flow_seq_clone = Arc::clone(&flow_seq_nav);
                    let tid = tid_nav.clone();
                    let lbl = lbl_nav.clone();
                    let target_id = target_nav.clone();
                    let win_l = win_label_nav.clone();

                    tauri::async_runtime::spawn(async move {
                        match adapter_clone.parse_manual_token(&payload_str).await {
                            Ok(envelope) => {
                                persist_oauth_envelope(
                                    &app_clone,
                                    &db_clone,
                                    &host_clone,
                                    &master_key,
                                    &adapter_clone,
                                    &scheduler_clone,
                                    &flow_seq_clone,
                                    flow_id,
                                    &win_l,
                                    &tid,
                                    &lbl,
                                    target_id.as_deref(),
                                    envelope,
                                )
                                .await;
                            }
                            Err(e) => {
                                let _ = app_clone
                                    .emit("qwitch://oauth-error", format!("解析授权凭据失败: {e}"));
                                if flow_seq_clone.load(Ordering::SeqCst) == flow_id {
                                    if let Some(win) = app_clone.get_webview_window(&win_l) {
                                        let _ = win.close();
                                    }
                                }
                            }
                        }
                    });
                }
                return false;
            }
            is_allowed_oauth_url(nav_url, allowed_hosts)
        });
    } else {
        builder =
            builder.on_navigation(move |nav_url| is_allowed_oauth_url(nav_url, allowed_hosts));
    }

    builder
        .build()
        .map_err(|e| format!("打开内置授权窗口失败: {e}"))?;

    match mode {
        OAuthMode::TraeCapture => {}
        OAuthMode::WorkBuddyPoll { state_token } => {
            tauri::async_runtime::spawn(async move {
                let mut consecutive_errors = 0u32;
                let mut finished = false;
                for _ in 0..180 {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    if flow_seq.load(Ordering::SeqCst) != flow_id
                        || app_handle.get_webview_window(&win_label).is_none()
                    {
                        finished = true;
                        break;
                    }
                    match WorkBuddyAdapter::poll_oauth_result(&state_token).await {
                        Ok(Some(envelope)) => {
                            finished = true;
                            persist_oauth_envelope(
                                &app_handle,
                                &db,
                                &host_arc,
                                &master_key,
                                &adapter,
                                &scheduler_arc,
                                &flow_seq,
                                flow_id,
                                &win_label,
                                &tool_id_owned,
                                &label_owned,
                                target_id_owned.as_deref(),
                                envelope,
                            )
                            .await;
                            break;
                        }
                        Ok(None) => {
                            consecutive_errors = 0;
                        }
                        Err(err) => {
                            let fatal = err.contains("无法唯一确认账号身份")
                                || err.contains("未获取到账号 UID");
                            consecutive_errors += 1;
                            if fatal || consecutive_errors >= 5 {
                                finished = true;
                                if flow_seq.load(Ordering::SeqCst) == flow_id {
                                    let _ = app_handle.emit("qwitch://oauth-error", err);
                                    if let Some(win) = app_handle.get_webview_window(&win_label) {
                                        let _ = win.close();
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
                if !finished && flow_seq.load(Ordering::SeqCst) == flow_id {
                    if let Some(win) = app_handle.get_webview_window(&win_label) {
                        let _ = app_handle.emit(
                            "qwitch://oauth-error",
                            "WorkBuddy 内置授权登录超时（4.5 分钟未完成），请重试",
                        );
                        let _ = win.close();
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
                let mut consecutive_errors = 0u32;
                let mut finished = false;
                for _ in 0..180 {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    if flow_seq.load(Ordering::SeqCst) != flow_id
                        || app_handle.get_webview_window(&win_label).is_none()
                    {
                        finished = true;
                        break;
                    }
                    match QoderAdapter::poll_pkce_device_token(&nonce, &verifier, &machine_id).await
                    {
                        Ok(Some(envelope)) => {
                            finished = true;
                            persist_oauth_envelope(
                                &app_handle,
                                &db,
                                &host_arc,
                                &master_key,
                                &adapter,
                                &scheduler_arc,
                                &flow_seq,
                                flow_id,
                                &win_label,
                                &tool_id_owned,
                                &label_owned,
                                target_id_owned.as_deref(),
                                envelope,
                            )
                            .await;
                            break;
                        }
                        Ok(None) => {
                            consecutive_errors = 0;
                        }
                        Err(err) => {
                            consecutive_errors += 1;
                            if consecutive_errors >= 5 {
                                finished = true;
                                if flow_seq.load(Ordering::SeqCst) == flow_id {
                                    let _ = app_handle.emit("qwitch://oauth-error", err);
                                    if let Some(win) = app_handle.get_webview_window(&win_label) {
                                        let _ = win.close();
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
                if !finished && flow_seq.load(Ordering::SeqCst) == flow_id {
                    if let Some(win) = app_handle.get_webview_window(&win_label) {
                        let _ = app_handle.emit(
                            "qwitch://oauth-error",
                            "Qoder CN 内置授权登录超时（4.5 分钟未完成），请重试",
                        );
                        let _ = win.close();
                    }
                }
            });
        }
        OAuthMode::MiniMaxDevicePoll { auth } => {
            tauri::async_runtime::spawn(async move {
                let mut consecutive_errors: u32 = 0;
                let mut poll_interval_ms = auth.poll_interval_ms().clamp(1_000, 30_000);
                loop {
                    if flow_seq.load(Ordering::SeqCst) != flow_id
                        || app_handle.get_webview_window(&win_label).is_none()
                    {
                        break;
                    }
                    if auth.is_expired() {
                        if flow_seq.load(Ordering::SeqCst) == flow_id {
                            let _ = app_handle.emit(
                                "qwitch://oauth-error",
                                "MiniMax 内置授权登录已超时，请重新发起授权",
                            );
                            if let Some(win) = app_handle.get_webview_window(&win_label) {
                                let _ = win.close();
                            }
                        }
                        break;
                    }
                    match MiniMaxAdapter::poll_device_authorization(&auth, host_arc.as_ref()).await
                    {
                        Ok(crate::adapter::minimax::DeviceAuthPoll::Pending) => {
                            consecutive_errors = 0;
                            tokio::time::sleep(std::time::Duration::from_millis(poll_interval_ms))
                                .await;
                        }
                        Ok(crate::adapter::minimax::DeviceAuthPoll::SlowDown) => {
                            consecutive_errors = 0;
                            poll_interval_ms = (poll_interval_ms.saturating_add(5_000)).min(30_000);
                            tokio::time::sleep(std::time::Duration::from_millis(poll_interval_ms))
                                .await;
                        }
                        Ok(crate::adapter::minimax::DeviceAuthPoll::Ready(envelope)) => {
                            persist_oauth_envelope(
                                &app_handle,
                                &db,
                                &host_arc,
                                &master_key,
                                &adapter,
                                &scheduler_arc,
                                &flow_seq,
                                flow_id,
                                &win_label,
                                &tool_id_owned,
                                &label_owned,
                                target_id_owned.as_deref(),
                                *envelope,
                            )
                            .await;
                            break;
                        }
                        Err(err) => {
                            let is_transient = matches!(
                                err,
                                AdapterError::Unified {
                                    code: UnifiedErrorCode::NetworkError,
                                    ..
                                }
                            );
                            if is_transient {
                                consecutive_errors += 1;
                                if consecutive_errors < 5 {
                                    tokio::time::sleep(std::time::Duration::from_millis(
                                        poll_interval_ms,
                                    ))
                                    .await;
                                    continue;
                                }
                            }
                            if flow_seq.load(Ordering::SeqCst) == flow_id {
                                let _ = app_handle.emit("qwitch://oauth-error", format!("{err}"));
                                if let Some(win) = app_handle.get_webview_window(&win_label) {
                                    let _ = win.close();
                                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_uid_validation_rejects_objects_and_blacklist() {
        assert!(is_valid_oauth_uid("u_123456"));
        assert!(is_valid_oauth_uid("1234567890"));
        assert!(!is_valid_oauth_uid("[object Object]"));
        assert!(!is_valid_oauth_uid("a,b"));
        assert!(!is_valid_oauth_uid("has space"));
        assert!(!is_valid_oauth_uid("{\"id\":1}"));
        assert!(!is_valid_oauth_uid(""));
        assert!(!is_valid_oauth_uid("x"));
        assert!(!is_valid_oauth_uid("null"));
        assert!(!is_valid_oauth_uid("0"));
        assert!(!is_valid_oauth_uid("guest"));
    }

    #[test]
    fn oauth_url_whitelist_allows_official_sso_https_and_blocks_http_or_evil() {
        let trae = allowed_oauth_hosts("trae_cn");
        assert!(is_allowed_oauth_url(
            &"https://www.trae.cn/login".parse().unwrap(),
            trae
        ));
        assert!(is_allowed_oauth_url(
            &"https://open.douyin.com/platform/oauth/connect"
                .parse()
                .unwrap(),
            trae
        ));
        assert!(is_allowed_oauth_url(
            &"https://passport.feishu.cn/suite/passport/page/login"
                .parse()
                .unwrap(),
            trae
        ));
        assert!(!is_allowed_oauth_url(
            &"http://www.trae.cn/login".parse().unwrap(),
            trae
        ));
        assert!(!is_allowed_oauth_url(
            &"https://evil-trae.cn/login".parse().unwrap(),
            trae
        ));

        let wb = allowed_oauth_hosts("workbuddy");
        assert!(is_allowed_oauth_url(
            &"https://open.weixin.qq.com/connect/qrconnect"
                .parse()
                .unwrap(),
            wb
        ));
        assert!(!is_allowed_oauth_url(
            &"https://phish.example.com".parse().unwrap(),
            wb
        ));

        let qd = allowed_oauth_hosts("qoder_cn");
        assert!(is_allowed_oauth_url(
            &"https://account.aliyun.com/login/login.htm"
                .parse()
                .unwrap(),
            qd
        ));
        assert!(is_allowed_oauth_url(
            &"https://github.com/login".parse().unwrap(),
            qd
        ));
        assert!(!is_allowed_oauth_url(
            &"http://qoder.cn/login".parse().unwrap(),
            qd
        ));
        assert!(is_allowed_oauth_url(&"about:blank".parse().unwrap(), qd));
    }

    #[test]
    fn minimax_oauth_whitelist_covers_captcha_iframe_domains() {
        // on_navigation 会校验 iframe 导航：腾讯 TCaptcha 的 iframe 必须在白名单内，
        // 否则登录页会一直停在“获取验证码/验证组件加载失败”。
        let mm = allowed_oauth_hosts("minimax_code_cn");
        for url in [
            "https://account.minimax.cn/oauth-authorize?user_code=ABCD-EFGH",
            "https://account.minimax.cn/unified-login?login_redirect=%2Foauth-authorize",
            "https://turing.captcha.qcloud.com/TCaptcha.js",
            "https://ca.turing.captcha.qcloud.com/",
            "https://cloudcache.tencentcs.com/",
            "https://static.hailuoai.com/app.js",
        ] {
            assert!(
                is_allowed_oauth_url(&url.parse().unwrap(), mm),
                "应允许 {url}"
            );
        }
        assert!(!is_allowed_oauth_url(
            &"https://evil.example.com/steal".parse().unwrap(),
            mm
        ));
    }
}
