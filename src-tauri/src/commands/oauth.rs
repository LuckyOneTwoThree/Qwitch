use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::adapter::{
    doubaowork::DoubaoWorkAdapter, lobsterai::LobsterAiAdapter, minimax::MiniMaxAdapter,
    qoder::QoderAdapter, workbuddy::WorkBuddyAdapter, AdapterError, ToolAdapter,
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
        "lobsterai" => &[
            "youdao.com",
            "163.com",
            "netease.com",
            "126.com",
            "126.net",
            "127.net",
            "ydstatic.com",
            "weixin.qq.com",
            "wx.qq.com",
            "qq.com",
            "apple.com",
            "github.com",
            "microsoftonline.com",
        ],
        "doubaowork" => &[
            "doubao.com",
            "doubaocdn.com",
            "douyin.com",
            "bytedance.com",
            "bytegoofy.com",
            "bytetos.com",
            "byteimg.com",
            "snssdk.com",
            "feishu.cn",
            "larkoffice.com",
            "volcengine.com",
            "yhgfb-cn-static.com",
            "zijieapi.com",
            "qq.com",
            "weixin.qq.com",
            "wx.qq.com",
            "apple.com",
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

const LOBSTER_CALLBACK_PORT: u16 = 47821;
const LOBSTER_CALLBACK_PATH: &str = "/auth/callback";
const LOBSTER_CALLBACK_URI: &str = "http://127.0.0.1:47821/auth/callback";

/// 识别并校验有道龙虾内置授权回调地址（本地回环回调与 `lobsterai://auth/callback`）。
/// 返回 `None` 表示非回调导航，继续按官方 HTTPS 白名单放行。
pub(crate) fn parse_lobster_callback_url(
    nav_url: &tauri::Url,
    expected_nonce: &str,
) -> Option<Result<String, &'static str>> {
    let is_loopback = nav_url.scheme() == "http"
        && nav_url.host_str() == Some("127.0.0.1")
        && nav_url.port() == Some(LOBSTER_CALLBACK_PORT)
        && nav_url.path() == LOBSTER_CALLBACK_PATH;
    let is_deeplink = nav_url.scheme() == "lobsterai"
        && nav_url.host_str() == Some("auth")
        && nav_url.path() == "/callback";

    if !is_loopback && !is_deeplink {
        return None;
    }

    let find_param = |key: &str| -> Option<String> {
        nav_url
            .query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };

    if find_param("state").as_deref() != Some(expected_nonce) {
        return Some(Err(
            "有道龙虾内置授权回调校验失败（状态参数不匹配），已拒绝该请求",
        ));
    }

    match find_param("code") {
        Some(code) => Some(Ok(code)),
        None => Some(Err("有道龙虾授权回调缺少 authCode，请重试")),
    }
}

fn close_existing_oauth_windows(app: &AppHandle) {
    for (existing_label, existing_win) in app.webview_windows() {
        if existing_label == "oauth-login" || existing_label.starts_with("oauth-login-") {
            let _ = existing_win.close();
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

    close_existing_oauth_windows(&app);

    enum OAuthMode {
        TraeCapture,
        LobsterAiCallback,
        DoubaoCookiePoll,
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
        "lobsterai" => {
            let auth_url =
                LobsterAiAdapter::build_oauth_login_url(LOBSTER_CALLBACK_URI, &flow_nonce)
                    .map_err(|e| format!("{e}"))?;
            (auth_url, OAuthMode::LobsterAiCallback)
        }
        "doubaowork" => (
            "https://www.doubao.com/chat/".to_string(),
            OAuthMode::DoubaoCookiePoll,
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
        OAuthMode::LobsterAiCallback
        | OAuthMode::WorkBuddyPoll { .. }
        | OAuthMode::QoderPkcePoll { .. } => POPUP_REDIRECT_JS.to_string(),
        // MiniMax / 豆包工作官方登录页依赖自身 window.open / 验证码及安全脚本，不注入自定义 JS
        OAuthMode::MiniMaxDevicePoll { .. } | OAuthMode::DoubaoCookiePoll => String::new(),
    };

    let blank_url: tauri::Url = "about:blank"
        .parse()
        .map_err(|e| format!("构造空白授权初始地址失败: {e}"))?;

    let mut builder = WebviewWindowBuilder::new(&app, &win_label, WebviewUrl::External(blank_url))
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
        let consumed_nonces = Arc::clone(&state.consumed_oauth_nonces);
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
                    // 凭据只允许经 URL fragment 传递（fragment 不会发送到任何服务器），
                    // 且经 nonce/flow/path 校验后在进程内被拦截；不再回退读取 URL query，
                    // 避免凭据出现在真实服务器可见的 query string 中。
                    fragment_url.as_ref().and_then(|fu| {
                        fu.query_pairs()
                            .find(|(k, _)| k == key)
                            .map(|(_, v)| v.to_string())
                    })
                };

                let callback_nonce = find_param("nonce");
                if callback_nonce.as_deref() != Some(expected_nonce.as_str()) {
                    let _ = app_nav.emit(
                        "qwitch://oauth-error",
                        "内置授权回调校验失败（会话状态不匹配），已拒绝该请求",
                    );
                    return false;
                }

                // 一次性消费：同一个 nonce 只允许成功回调一次。
                // 否则授权页内的脚本可反复投递不同 payload，向凭据库注入任意 token。
                {
                    match consume_oauth_nonce(&consumed_nonces, &expected_nonce) {
                        Ok(true) => {}
                        Ok(false) => {
                            let _ = app_nav.emit(
                                "qwitch://oauth-error",
                                "该授权回调已被使用过，已拒绝重复提交",
                            );
                            return false;
                        }
                        Err(()) => {
                            let _ = app_nav.emit(
                                "qwitch://oauth-error",
                                "内置授权回调校验失败（内部状态不可用），已拒绝该请求",
                            );
                            return false;
                        }
                    }
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
                                let _ = app_clone.emit(
                                    "qwitch://oauth-error",
                                    crate::storage::crypto::sanitize_log_message(&format!(
                                        "解析授权凭据失败: {e}"
                                    )),
                                );
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
            nav_url.as_str() == "about:blank" || is_allowed_oauth_url(nav_url, allowed_hosts)
        });
    } else if matches!(mode, OAuthMode::LobsterAiCallback) {
        let app_nav = app_handle.clone();
        let db_nav = Arc::clone(&db);
        let scheduler_nav = Arc::clone(&scheduler_arc);
        let host_nav = Arc::clone(&host_arc);
        let adapter_nav = Arc::clone(&adapter);
        let flow_seq_nav = Arc::clone(&flow_seq);
        let expected_nonce = flow_nonce.clone();
        let consumed_nonces = Arc::clone(&state.consumed_oauth_nonces);
        let tid_nav = tool_id_owned.clone();
        let lbl_nav = label_owned.clone();
        let target_nav = target_id_owned.clone();
        let win_label_nav = win_label.clone();

        builder = builder.on_navigation(move |nav_url| {
            if let Some(cb_res) = parse_lobster_callback_url(nav_url, &expected_nonce) {
                if flow_seq_nav.load(Ordering::SeqCst) != flow_id {
                    return false;
                }

                let close_cb_win = |app_ref: &AppHandle, win_l: &str| {
                    if flow_seq_nav.load(Ordering::SeqCst) == flow_id {
                        if let Some(win) = app_ref.get_webview_window(win_l) {
                            let _ = win.close();
                        }
                    }
                };

                match cb_res {
                    Ok(code) => {
                        match consume_oauth_nonce(&consumed_nonces, &expected_nonce) {
                            Ok(true) => {}
                            Ok(false) => {
                                let _ = app_nav.emit(
                                    "qwitch://oauth-error",
                                    "该授权回调已被使用过，已拒绝重复提交",
                                );
                                return false;
                            }
                            Err(()) => {
                                let _ = app_nav.emit(
                                    "qwitch://oauth-error",
                                    "内置授权回调校验失败（内部状态不可用），已拒绝该请求",
                                );
                                return false;
                            }
                        }

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
                            match LobsterAiAdapter::exchange_auth_code(&code).await {
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
                                    let _ = app_clone.emit(
                                        "qwitch://oauth-error",
                                        crate::storage::crypto::sanitize_log_message(&format!(
                                            "交换有道龙虾授权令牌失败: {e}"
                                        )),
                                    );
                                    if flow_seq_clone.load(Ordering::SeqCst) == flow_id {
                                        if let Some(win) = app_clone.get_webview_window(&win_l) {
                                            let _ = win.close();
                                        }
                                    }
                                }
                            }
                        });
                    }
                    Err(msg) => {
                        let _ = app_nav.emit("qwitch://oauth-error", msg);
                        close_cb_win(&app_nav, &win_label_nav);
                    }
                }
                return false;
            }
            nav_url.as_str() == "about:blank" || is_allowed_oauth_url(nav_url, allowed_hosts)
        });
    } else {
        builder = builder.on_navigation(move |nav_url| {
            nav_url.as_str() == "about:blank" || is_allowed_oauth_url(nav_url, allowed_hosts)
        });
    }

    if state.oauth_flow_seq.load(Ordering::SeqCst) != flow_id {
        return Ok(());
    }
    close_existing_oauth_windows(&app);

    let oauth_win = builder
        .build()
        .map_err(|e| format!("打开内置授权窗口失败: {e}"))?;
    let _ = oauth_win.clear_all_browsing_data();
    oauth_win
        .navigate(parsed_url)
        .map_err(|e| format!("导航至授权页失败: {e}"))?;

    match mode {
        OAuthMode::TraeCapture | OAuthMode::LobsterAiCallback => {}
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
        OAuthMode::DoubaoCookiePoll => {
            tauri::async_runtime::spawn(async move {
                let mut consecutive_errors: u32 = 0;
                let mut last_rejected_cookie: Option<String> = None;
                let mut finished = false;
                for _ in 0..180 {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    if flow_seq.load(Ordering::SeqCst) != flow_id {
                        finished = true;
                        break;
                    }
                    let Some(win) = app_handle.get_webview_window(&win_label) else {
                        finished = true;
                        break;
                    };
                    let win_for_cookies = win.clone();
                    let cookies = match tokio::task::spawn_blocking(move || {
                        win_for_cookies.cookies()
                    })
                    .await
                    {
                        Ok(Ok(list)) => list,
                        _ => continue,
                    };
                    let Some(cookie_header) = build_doubao_cookie_header(&cookies) else {
                        continue;
                    };
                    if last_rejected_cookie.as_deref() == Some(cookie_header.as_str()) {
                        continue;
                    }
                    match DoubaoWorkAdapter::verify_cookie_and_build_envelope(&cookie_header).await
                    {
                        Ok(envelope) => {
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
                        Err(AdapterError::Unified {
                            code:
                                UnifiedErrorCode::InvalidCredential | UnifiedErrorCode::AuthExpired,
                            ..
                        }) => {
                            last_rejected_cookie = Some(cookie_header);
                            consecutive_errors = 0;
                        }
                        Err(err) => {
                            consecutive_errors += 1;
                            if consecutive_errors >= 5 {
                                finished = true;
                                if flow_seq.load(Ordering::SeqCst) == flow_id {
                                    let _ = app_handle.emit(
                                        "qwitch://oauth-error",
                                        crate::storage::crypto::sanitize_log_message(&format!(
                                            "{err}"
                                        )),
                                    );
                                    let _ = win.close();
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
                            "豆包工作内置授权登录超时（4.5 分钟未完成），请重试",
                        );
                        let _ = win.close();
                    }
                }
            });
        }
    }

    Ok(())
}

#[derive(Clone, Copy)]
struct RawDoubaoCookie<'a> {
    domain: Option<&'a str>,
    name: &'a str,
    value: &'a str,
    expired: bool,
}

fn is_doubao_cookie_domain(raw_domain: Option<&str>) -> bool {
    let Some(raw) = raw_domain else {
        return false;
    };
    let d = raw.trim().trim_start_matches('.').to_ascii_lowercase();
    d == "doubao.com" || d.ends_with(".doubao.com")
}

fn is_valid_http_cookie_pair(name: &str, value: &str) -> bool {
    if name.is_empty()
        || value.is_empty()
        || name
            .bytes()
            .any(|b| !(0x21..0x7f).contains(&b) || matches!(b, b';' | b'='))
        || value
            .bytes()
            .any(|b| !(0x20..0x7f).contains(&b) || b == b';')
    {
        return false;
    }
    reqwest::header::HeaderValue::from_str(&format!("{name}={value}")).is_ok()
}

fn build_doubao_cookie_header_from_entries(entries: &[RawDoubaoCookie<'_>]) -> Option<String> {
    // 先按域名从泛到专排序（先 `.doubao.com`，后 `www.doubao.com`），确保主机级 Cookie 覆盖域级同名项
    let mut matched: Vec<(usize, bool, &str, &str)> = Vec::new();
    for entry in entries {
        if entry.expired || !is_doubao_cookie_domain(entry.domain) {
            continue;
        }
        let name = entry.name.trim();
        let val = entry.value.trim();
        if !is_valid_http_cookie_pair(name, val) {
            continue;
        }
        let raw_dom = entry.domain.unwrap_or("").trim();
        let norm_len = raw_dom.trim_start_matches('.').len();
        let is_exact_host = !raw_dom.starts_with('.');
        matched.push((norm_len, is_exact_host, name, val));
    }
    matched.sort_by_key(|(len, exact, _, _)| (*len, *exact));

    let mut map = std::collections::BTreeMap::<&str, &str>::new();
    for (_, _, name, val) in matched {
        map.insert(name, val);
    }

    let has_session = ["sessionid", "sessionid_ss", "sid_tt"]
        .iter()
        .any(|k| map.get(k).is_some_and(|v| !v.is_empty()));
    if !has_session {
        return None;
    }

    let joined = map
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    reqwest::header::HeaderValue::from_str(&joined)
        .ok()
        .map(|_| joined)
}

fn build_doubao_cookie_header(cookies: &[tauri::webview::Cookie<'_>]) -> Option<String> {
    let now_ts = chrono::Utc::now().timestamp();
    let entries: Vec<RawDoubaoCookie<'_>> = cookies
        .iter()
        .map(|cookie| {
            let expired = cookie
                .expires_datetime()
                .is_some_and(|dt| dt.unix_timestamp() <= now_ts);
            RawDoubaoCookie {
                domain: cookie.domain(),
                name: cookie.name(),
                value: cookie.value(),
                expired,
            }
        })
        .collect();
    build_doubao_cookie_header_from_entries(&entries)
}

/// 尝试消费一个 OAuth 回调 nonce（一次性语义）。
///
/// 返回 `true` 表示首次消费成功；`false` 表示该 nonce 已被使用过。
/// 独立成函数以便单元测试覆盖「重放被拒」这一安全属性。
pub(crate) fn consume_oauth_nonce(
    consumed: &std::sync::Mutex<std::collections::HashSet<String>>,
    nonce: &str,
) -> Result<bool, ()> {
    let mut guard = consumed.lock().map_err(|_| ())?;
    Ok(guard.insert(nonce.to_string()))
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

    #[test]
    fn oauth_nonce_is_single_use() {
        use std::collections::HashSet;
        use std::sync::Mutex;

        let consumed: Mutex<HashSet<String>> = Mutex::new(HashSet::new());

        // 首次消费成功
        assert_eq!(consume_oauth_nonce(&consumed, "nonce-A"), Ok(true));
        // 重放被拒（这正是防止授权页内脚本注入任意 token 的关键）
        assert_eq!(consume_oauth_nonce(&consumed, "nonce-A"), Ok(false));
        assert_eq!(consume_oauth_nonce(&consumed, "nonce-A"), Ok(false));
        // 不同 nonce 互不影响
        assert_eq!(consume_oauth_nonce(&consumed, "nonce-B"), Ok(true));
    }

    #[test]
    fn lobsterai_oauth_whitelist_and_callback_validation() {
        let hosts = allowed_oauth_hosts("lobsterai");
        for url in [
            "https://lobsterai-server.youdao.com/login",
            "https://lobsterai.youdao.com/portal",
            "https://cstatic.126.net/dummy.js",
            "https://dun.163.com/api/v2/check",
            "https://urs.netease.com/login",
            "https://open.weixin.qq.com/connect/qrconnect",
        ] {
            assert!(
                is_allowed_oauth_url(&url.parse().unwrap(), hosts),
                "应允许 {url}"
            );
        }
        for bad in [
            "http://lobsterai.youdao.com/portal",
            "https://evil.example.com/phish",
        ] {
            assert!(!is_allowed_oauth_url(&bad.parse().unwrap(), hosts));
        }

        let valid_loopback: tauri::Url =
            "http://127.0.0.1:47821/auth/callback?code=c_123&state=nonce-ok"
                .parse()
                .unwrap();
        assert_eq!(
            parse_lobster_callback_url(&valid_loopback, "nonce-ok"),
            Some(Ok("c_123".to_string()))
        );
        assert!(matches!(
            parse_lobster_callback_url(&valid_loopback, "wrong-nonce"),
            Some(Err(_))
        ));

        let valid_deeplink: tauri::Url = "lobsterai://auth/callback?code=c_deep&state=nonce-ok"
            .parse()
            .unwrap();
        assert_eq!(
            parse_lobster_callback_url(&valid_deeplink, "nonce-ok"),
            Some(Ok("c_deep".to_string()))
        );
        let missing_state_deeplink: tauri::Url =
            "lobsterai://auth/callback?code=c_deep".parse().unwrap();
        assert!(matches!(
            parse_lobster_callback_url(&missing_state_deeplink, "nonce-ok"),
            Some(Err(_))
        ));

        let non_cb: tauri::Url = "https://lobsterai.youdao.com/portal".parse().unwrap();
        assert_eq!(parse_lobster_callback_url(&non_cb, "nonce-ok"), None);
    }

    #[test]
    fn doubaowork_oauth_whitelist_allows_official_domains_and_blocks_untrusted() {
        let hosts = allowed_oauth_hosts("doubaowork");
        for url in [
            "https://www.doubao.com/chat/",
            "https://accounts.doubao.com/passport/web/login/",
            "https://open.douyin.com/platform/oauth/connect",
            "https://lf-rc1.yhgfb-cn-static.com/obj/rc-verifycenter/captcha.js",
            "https://passport.feishu.cn/suite/passport/page/login",
        ] {
            assert!(
                is_allowed_oauth_url(&url.parse().unwrap(), hosts),
                "应允许 {url}"
            );
        }
        for bad in [
            "http://www.doubao.com/chat/",
            "https://evil.example.com/steal",
        ] {
            assert!(!is_allowed_oauth_url(&bad.parse().unwrap(), hosts));
        }
    }

    #[test]
    fn doubao_cookie_header_matches_subdomains_skips_expired_non_ascii_and_prefers_host() {
        let entries = [
            RawDoubaoCookie {
                domain: Some(".doubao.com"),
                name: "sessionid",
                value: "wild-session",
                expired: false,
            },
            RawDoubaoCookie {
                domain: Some("www.doubao.com"),
                name: "sessionid",
                value: "host-session",
                expired: false,
            },
            RawDoubaoCookie {
                domain: Some(".doubao.com"),
                name: "sid_tt",
                value: "expired-sid",
                expired: true,
            },
            RawDoubaoCookie {
                domain: Some(".doubao.com"),
                name: "cn_banner",
                value: "豆包中文值",
                expired: false,
            },
            RawDoubaoCookie {
                domain: Some("evil-doubao.com"),
                name: "sid_tt",
                value: "evil-sid",
                expired: false,
            },
            RawDoubaoCookie {
                domain: Some("accounts.doubao.com"),
                name: "odin_tt",
                value: "ok_odin",
                expired: false,
            },
        ];

        let header =
            build_doubao_cookie_header_from_entries(&entries).expect("valid cookie header");
        assert!(header.contains("sessionid=host-session"));
        assert!(header.contains("odin_tt=ok_odin"));
        assert!(!header.contains("wild-session"));
        assert!(!header.contains("expired-sid"));
        assert!(!header.contains("cn_banner"));
        assert!(!header.contains("evil-sid"));
    }
}
