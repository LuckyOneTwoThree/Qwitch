pub mod ir;
pub mod scheduler;

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::adapter::common::shared_http_client;
use crate::adapter::ToolAdapter;
use crate::agent_wiring::{agent_id_from_token, detect_bridge_runtime, detect_global_proxy_active};
use crate::domain::{
    compute_agent_exposed_models, resolve_vendor_wire_model_id, upstream_model_protocol,
    AccountPool, CredentialEnvelope, CredentialPayload, GatewayRuntimeState, PoolMember,
    RoutingAttempt, RoutingAttemptOutcome, RoutingTraceItem, RoutingTraceStatus,
    RunPoolSimulationInput, RunPoolSimulationResult, SimulationFailureMode, TargetAgentId,
    WireProtocolKind,
};
use crate::storage::QwitchDatabase;

use ir::{
    parse_anthropic_messages_request, parse_openai_chat_request, parse_openai_responses_request,
    to_anthropic_messages_response, to_anthropic_messages_sse, to_openai_chat_response,
    to_openai_chat_sse, to_openai_responses_response, to_openai_responses_sse,
    to_upstream_anthropic_body, to_upstream_openai_chat_body, IrRequest, IrResponse, IrUsage,
};
use scheduler::select_candidates_for_request;

struct GatewayInner {
    running: bool,
    host: String,
    port: u16,
    cooldowns: HashMap<String, i64>,
    session_affinity: HashMap<String, String>,
    sim_proxy_override: Option<bool>,
    sim_bridge_override: Option<(String, String)>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

pub struct GatewayRuntime {
    inner: Mutex<GatewayInner>,
    /// 按 `account_id` 隔离的 RefreshSingleFlight 锁（完全独立于 AntiRiskScheduler 队列锁）
    refresh_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Default for GatewayRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl GatewayRuntime {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(GatewayInner {
                running: false,
                host: "127.0.0.1".into(),
                port: 18425,
                cooldowns: HashMap::new(),
                session_affinity: HashMap::new(),
                sim_proxy_override: None,
                sim_bridge_override: None,
                shutdown_tx: None,
            }),
            refresh_locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn runtime_state(&self) -> GatewayRuntimeState {
        let (running, host, port, proxy_ovr, bridge_ovr) = {
            let g = self.inner.lock().expect("gateway lock");
            (
                g.running,
                g.host.clone(),
                g.port,
                g.sim_proxy_override,
                g.sim_bridge_override.clone(),
            )
        };
        let (bridge_runtime, bridge_version) = bridge_ovr.unwrap_or_else(detect_bridge_runtime);
        let global_proxy_detected = proxy_ovr.unwrap_or_else(detect_global_proxy_active);
        GatewayRuntimeState {
            running,
            host,
            port,
            bridge_runtime,
            bridge_version,
            global_proxy_detected,
        }
    }

    pub fn cooldown_snapshot(&self) -> HashMap<String, i64> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut g = self.inner.lock().expect("gateway lock");
        g.cooldowns.retain(|_, until| *until > now);
        g.cooldowns.clone()
    }

    pub fn set_member_cooldown(&self, account_id: &str, cooldown_until_ms: i64) {
        let mut g = self.inner.lock().expect("gateway lock");
        g.cooldowns
            .insert(account_id.to_string(), cooldown_until_ms);
    }

    pub fn clear_member_cooldown(&self, account_id: &str) {
        let mut g = self.inner.lock().expect("gateway lock");
        g.cooldowns.remove(account_id);
    }

    pub fn set_port(&self, port: u16) -> Result<(), String> {
        if port < 1024 {
            return Err("端口必须 >= 1024".into());
        }
        let mut g = self.inner.lock().expect("gateway lock");
        g.port = port;
        Ok(())
    }

    pub fn toggle_proxy_simulation(&self) -> bool {
        let current = self.runtime_state().global_proxy_detected;
        let next = !current;
        let mut g = self.inner.lock().expect("gateway lock");
        g.sim_proxy_override = Some(next);
        next
    }

    pub fn cycle_bridge_runtime(&self) -> (String, String) {
        let curr = self.runtime_state().bridge_runtime;
        let next = match curr.as_str() {
            "bun_ready" => ("node_ready".into(), "Node v22 (回退)".into()),
            "node_ready" => ("missing".into(), "未检测到运行时".into()),
            _ => ("bun_ready".into(), "Bun 1.2.15".into()),
        };
        let mut g = self.inner.lock().expect("gateway lock");
        g.sim_bridge_override = Some(next.clone());
        next
    }

    pub fn refresh_lock_for(&self, account_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.refresh_locks.lock().expect("refresh locks");
        map.entry(account_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    pub async fn start_server(
        self: &Arc<Self>,
        db: Arc<QwitchDatabase>,
        master_key: [u8; 32],
        adapters: Arc<HashMap<String, Arc<dyn ToolAdapter>>>,
    ) -> Result<u16, String> {
        let (host, port, already_running) = {
            let g = self.inner.lock().expect("gateway lock");
            (g.host.clone(), g.port, g.running)
        };
        if already_running {
            return Ok(port);
        }

        let listener = TcpListener::bind(format!("{host}:{port}"))
            .await
            .map_err(|e| format!("无法绑定网关端口 {host}:{port}: {e}"))?;
        let bound_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        let (tx, mut rx) = oneshot::channel::<()>();

        {
            let mut g = self.inner.lock().expect("gateway lock");
            g.running = true;
            g.port = bound_port;
            g.shutdown_tx = Some(tx);
        }

        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => {
                        break;
                    }
                    accepted = listener.accept() => {
                        let Ok((stream, _peer)) = accepted else {
                            continue;
                        };
                        let rt = Arc::clone(&runtime);
                        let db_c = Arc::clone(&db);
                        let ad_c = Arc::clone(&adapters);
                        tokio::spawn(async move {
                            let _ = handle_http_connection(stream, rt, db_c, master_key, ad_c).await;
                        });
                    }
                }
            }
        });

        Ok(bound_port)
    }

    pub fn stop_server(&self) {
        let mut g = self.inner.lock().expect("gateway lock");
        g.running = false;
        if let Some(tx) = g.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }

    pub fn run_simulation(
        &self,
        db: &QwitchDatabase,
        master_key: &[u8; 32],
        input: &RunPoolSimulationInput,
    ) -> Result<RunPoolSimulationResult, String> {
        let now = chrono::Utc::now().timestamp_millis();
        let cooldowns = self.cooldown_snapshot();
        let pools = db
            .list_pools(master_key, &cooldowns)
            .map_err(|e| e.to_string())?;
        let pool = pools
            .iter()
            .find(|p| p.id == input.pool_id)
            .ok_or_else(|| format!("找不到号池 {}", input.pool_id))?;

        let affinity_snap = {
            self.inner
                .lock()
                .expect("gateway lock")
                .session_affinity
                .clone()
        };

        let requested_model = if input.model_id.trim().is_empty() {
            pool.model_policies
                .iter()
                .find(|m| m.enabled)
                .map(|m| m.model_id.clone())
                .unwrap_or_else(|| "deepseek-v4-pro".into())
        } else {
            input.model_id.clone()
        };

        let plan = select_candidates_for_request(
            pool,
            &requested_model,
            input.session_key.as_deref(),
            &cooldowns,
            &affinity_snap,
            now,
        );

        let mut attempts = plan.skipped_attempts;
        let mut cooled_member_id = None;
        let mut final_member: Option<PoolMember> = None;
        let mut effective_model = plan.effective_model.clone();
        let mut status = RoutingTraceStatus::Success;

        if plan.candidates.is_empty() {
            status = RoutingTraceStatus::Failed;
            attempts.push(RoutingAttempt {
                account_label: pool.name.clone(),
                tool_id: "custom_api_key".into(),
                model: requested_model.clone(),
                tier: None,
                outcome: RoutingAttemptOutcome::Cooldown429,
                latency_ms: 12,
                detail: plan
                    .diagnostic_reason
                    .unwrap_or_else(|| "号池暂无可用成员".into()),
            });
        } else {
            match input.failure_mode {
                SimulationFailureMode::First429 => {
                    if plan.candidates.len() >= 2 {
                        let first = &plan.candidates[0];
                        let second = &plan.candidates[1];
                        cooled_member_id = Some(first.id.clone());
                        self.set_member_cooldown(
                            &first.account_id,
                            now + (pool.cooldown_seconds as i64) * 1000,
                        );
                        final_member = Some(second.clone());
                        status = RoutingTraceStatus::FailoverSuccess;
                        attempts.push(RoutingAttempt {
                            account_label: first.label.clone(),
                            tool_id: first.tool_id.clone(),
                            model: effective_model.clone(),
                            tier: Some(first.tier),
                            outcome: RoutingAttemptOutcome::Cooldown429,
                            latency_ms: 185,
                            detail: format!(
                                "{} 首选号并发限流 HTTP 429，置入 {}s 冷却并触发首字节前 Failover",
                                first.tier.short_label(),
                                pool.cooldown_seconds
                            ),
                        });
                        attempts.push(RoutingAttempt {
                            account_label: second.label.clone(),
                            tool_id: second.tool_id.clone(),
                            model: effective_model.clone(),
                            tier: Some(second.tier),
                            outcome: RoutingAttemptOutcome::Ok,
                            latency_ms: second.last_latency_ms.unwrap_or(560),
                            detail: format!(
                                "{} 接管成功，严格锁定模型 {}",
                                second.tier.short_label(),
                                effective_model
                            ),
                        });
                    } else {
                        let only = &plan.candidates[0];
                        cooled_member_id = Some(only.id.clone());
                        self.set_member_cooldown(
                            &only.account_id,
                            now + (pool.cooldown_seconds as i64) * 1000,
                        );
                        let fallback_id = pool
                            .model_policies
                            .iter()
                            .find(|p| p.model_id == requested_model)
                            .and_then(|p| p.fallback_model_id.as_deref());
                        if !plan.used_explicit_fallback {
                            if let Some(fb) = fallback_id {
                                let fb_plan = select_candidates_for_request(
                                    pool,
                                    fb,
                                    input.session_key.as_deref(),
                                    &self.cooldown_snapshot(),
                                    &affinity_snap,
                                    now,
                                );
                                if let Some(fb_m) = fb_plan.candidates.first() {
                                    effective_model = fb.to_string();
                                    final_member = Some(fb_m.clone());
                                    status = RoutingTraceStatus::FailoverSuccess;
                                    attempts.push(RoutingAttempt {
                                        account_label: only.label.clone(),
                                        tool_id: only.tool_id.clone(),
                                        model: requested_model.clone(),
                                        tier: Some(only.tier),
                                        outcome: RoutingAttemptOutcome::Cooldown429,
                                        latency_ms: 180,
                                        detail: format!(
                                            "「{requested_model}」最后可用账号触发 429，按显式降级配置转入 {effective_model}"
                                        ),
                                    });
                                    attempts.push(RoutingAttempt {
                                        account_label: fb_m.label.clone(),
                                        tool_id: fb_m.tool_id.clone(),
                                        model: effective_model.clone(),
                                        tier: Some(fb_m.tier),
                                        outcome: RoutingAttemptOutcome::Ok,
                                        latency_ms: fb_m.last_latency_ms.unwrap_or(520),
                                        detail: format!(
                                            "显式降级模型「{effective_model}」返回成功"
                                        ),
                                    });
                                }
                            }
                        }
                        if final_member.is_none() {
                            status = RoutingTraceStatus::Failed;
                            attempts.push(RoutingAttempt {
                                account_label: only.label.clone(),
                                tool_id: only.tool_id.clone(),
                                model: requested_model.clone(),
                                tier: Some(only.tier),
                                outcome: RoutingAttemptOutcome::Cooldown429,
                                latency_ms: 180,
                                detail: format!(
                                    "「{requested_model}」最后一个可用账号触发 429，且未配置显式降级模型，严格锁模拒绝换模型"
                                ),
                            });
                        }
                    }
                }
                SimulationFailureMode::Refresh401 => {
                    let first = &plan.candidates[0];
                    final_member = Some(first.clone());
                    status = RoutingTraceStatus::FailoverSuccess;
                    attempts.push(RoutingAttempt {
                        account_label: first.label.clone(),
                        tool_id: first.tool_id.clone(),
                        model: effective_model.clone(),
                        tier: Some(first.tier),
                        outcome: RoutingAttemptOutcome::Refreshed401,
                        latency_ms: 190,
                        detail: "上游返回 401，RefreshSingleFlight 按 account_id 单飞刷新并加密写回 qwitch.db".into(),
                    });
                    attempts.push(RoutingAttempt {
                        account_label: first.label.clone(),
                        tool_id: first.tool_id.clone(),
                        model: effective_model.clone(),
                        tier: Some(first.tier),
                        outcome: RoutingAttemptOutcome::Ok,
                        latency_ms: first.last_latency_ms.unwrap_or(520),
                        detail: format!("使用新 Token 原地重试 {effective_model} 成功"),
                    });
                }
                SimulationFailureMode::None => {
                    let first = &plan.candidates[0];
                    final_member = Some(first.clone());
                    status = if plan.used_explicit_fallback {
                        RoutingTraceStatus::FailoverSuccess
                    } else {
                        RoutingTraceStatus::Success
                    };
                    attempts.push(RoutingAttempt {
                        account_label: first.label.clone(),
                        tool_id: first.tool_id.clone(),
                        model: effective_model.clone(),
                        tier: Some(first.tier),
                        outcome: RoutingAttemptOutcome::Ok,
                        latency_ms: first.last_latency_ms.unwrap_or(540),
                        detail: if plan.used_explicit_fallback {
                            format!("主模型不可用，按显式降级配置由「{effective_model}」接管")
                        } else if plan.cache_affinity_hit {
                            format!(
                                "同会话亲和命中 Prompt Cache（自动跳过已冷却号并粘住 {}）",
                                first.label
                            )
                        } else {
                            format!(
                                "按 {} + 策略命中「{effective_model}」",
                                first.tier.short_label()
                            )
                        },
                    });
                }
            }
        }

        if let Some(ref winner) = final_member {
            if pool.affinity_mode != crate::domain::AffinityMode::Off {
                let aff_key = format!(
                    "{}::{}::{}",
                    pool.id,
                    effective_model,
                    input.session_key.as_deref().unwrap_or("default")
                );
                self.inner
                    .lock()
                    .expect("gateway lock")
                    .session_affinity
                    .insert(aff_key, winner.account_id.clone());
            }
        }

        let incoming_protocol = match input.agent_id {
            TargetAgentId::CodexDesktop => WireProtocolKind::OpenaiResponses,
            TargetAgentId::ClaudeDesktop => WireProtocolKind::AnthropicMessages,
            TargetAgentId::Dsh | TargetAgentId::Opencode => WireProtocolKind::OpenaiChat,
        };
        let upstream_protocol = final_member
            .as_ref()
            .map(|m| upstream_model_protocol(&m.tool_id, &effective_model))
            .unwrap_or(WireProtocolKind::OpenaiChat);
        let ttft_ms: u64 = attempts.iter().map(|a| a.latency_ms).sum();
        let total_tokens = ((input.prompt.trim().len() as u64) * 38 + 2400).max(1200);
        let cache_affinity_hit = final_member.is_some()
            && plan.cache_affinity_hit
            && input.failure_mode == SimulationFailureMode::None;

        let trace = RoutingTraceItem {
            id: format!("tr-{}", &Uuid::new_v4().simple().to_string()[..8]),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            agent_id: input.agent_id,
            pool_id: pool.id.clone(),
            pool_name: pool.name.clone(),
            incoming_protocol,
            upstream_protocol,
            requested_model: requested_model.clone(),
            final_account_label: final_member
                .as_ref()
                .map(|m| m.label.clone())
                .unwrap_or_else(|| "无可用成员".into()),
            final_tool_id: final_member
                .as_ref()
                .map(|m| m.tool_id.clone())
                .unwrap_or_else(|| "custom_api_key".into()),
            final_model: if final_member.is_some() {
                effective_model
            } else {
                "—".into()
            },
            status,
            cache_affinity_hit,
            ttft_ms,
            total_tokens,
            attempts,
        };

        let _ = db.append_routing_trace(&trace);
        let _ = db.increment_pool_stats(
            &pool.id,
            status == RoutingTraceStatus::FailoverSuccess,
            cache_affinity_hit,
        );

        Ok(RunPoolSimulationResult {
            trace,
            cooled_member_id,
        })
    }
}

fn is_valid_loopback_host(host: &str) -> bool {
    let bare = host.split(':').next().unwrap_or("");
    matches!(bare, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in extra_headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

fn resolve_target_pool_and_model(
    pools: &[AccountPool],
    agent_id: Option<TargetAgentId>,
    raw_model: &str,
) -> Option<(AccountPool, String)> {
    let trimmed_raw = raw_model.trim();
    // 1. 优先精确匹配 `<tool_id>/<model_id>` 工具作用域策略
    if let (Some(tid), bare) = crate::domain::split_scoped_model_id(trimmed_raw) {
        for p in pools.iter().filter(|p| p.enabled) {
            if p.model_policies
                .iter()
                .any(|pol| pol.enabled && pol.model_id == trimmed_raw)
            {
                return Some((p.clone(), trimmed_raw.to_string()));
            }
            let has_tool_member = p.members.iter().any(|m| {
                m.enabled
                    && m.tool_id == tid
                    && m.supported_models.iter().any(|s| {
                        s == trimmed_raw || crate::domain::split_scoped_model_id(s).1 == bare
                    })
            });
            if has_tool_member
                && p.model_policies
                    .iter()
                    .any(|pol| pol.enabled && (pol.model_id == trimmed_raw || pol.model_id == bare))
            {
                return Some((p.clone(), trimmed_raw.to_string()));
            }
        }
    }
    // 2. 尝试解包 `<pool_slug>/<model_or_scoped_id>`
    if let Some((slug, model)) = trimmed_raw.split_once('/') {
        if let Some(p) = pools.iter().find(|p| p.slug == slug && p.enabled) {
            return Some((p.clone(), model.to_string()));
        }
    }
    // 3. Claude Desktop 伪装 ID `mythos-qwitch-*` 反向映射回池与模型
    if agent_id == Some(TargetAgentId::ClaudeDesktop) {
        for p in pools.iter().filter(|p| p.enabled) {
            for pol in &p.model_policies {
                if pol.enabled && pol.claude_alias_id.as_deref() == Some(trimmed_raw) {
                    return Some((p.clone(), pol.model_id.clone()));
                }
            }
        }
    }
    // 4. 按启用池顺序精确匹配 model_id
    for p in pools.iter().filter(|p| p.enabled) {
        if p.model_policies
            .iter()
            .any(|pol| pol.enabled && pol.model_id == trimmed_raw)
        {
            return Some((p.clone(), trimmed_raw.to_string()));
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexOwnAuth {
    upstream_url: String,
    bearer_token: String,
    account_id: Option<String>,
}

fn resolve_codex_own_auth(
    path: &str,
    headers: &HashMap<String, String>,
    auth_token: &str,
    codex_home_override: Option<&Path>,
) -> Option<CodexOwnAuth> {
    let codex_home = codex_home_override
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| PathBuf::from(h).join(".codex"))
        });

    let chatgpt_endpoint = || {
        if path.ends_with("/compact") {
            "https://chatgpt.com/backend-api/codex/responses/compact".to_string()
        } else {
            "https://chatgpt.com/backend-api/codex/responses".to_string()
        }
    };

    if let Some(dir) = codex_home {
        let auth_path = dir.join("auth.json");
        if let Ok(raw) = std::fs::read_to_string(&auth_path) {
            if let Ok(doc) = serde_json::from_str::<Value>(&raw) {
                if let Some(access_token) = doc
                    .get("tokens")
                    .and_then(|t| t.get("access_token"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let account_id = headers
                        .get("chatgpt-account-id")
                        .cloned()
                        .or_else(|| {
                            doc.get("tokens")
                                .and_then(|t| t.get("account_id"))
                                .and_then(Value::as_str)
                                .map(String::from)
                        })
                        .filter(|s| !s.trim().is_empty());
                    return Some(CodexOwnAuth {
                        upstream_url: chatgpt_endpoint(),
                        bearer_token: access_token.to_string(),
                        account_id,
                    });
                }
                if let Some(api_key) = doc
                    .get("OPENAI_API_KEY")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    return Some(CodexOwnAuth {
                        upstream_url: format!("https://api.openai.com{path}"),
                        bearer_token: api_key.to_string(),
                        account_id: None,
                    });
                }
            }
        }
    }

    let trimmed_auth = auth_token.trim();
    if !trimmed_auth.is_empty()
        && !trimmed_auth.starts_with("qwitch-bind-")
        && !trimmed_auth.eq_ignore_ascii_case("qwitch")
    {
        let account_id = headers
            .get("chatgpt-account-id")
            .cloned()
            .filter(|s| !s.trim().is_empty());
        let upstream_url = if account_id.is_some() {
            chatgpt_endpoint()
        } else {
            format!("https://api.openai.com{path}")
        };
        return Some(CodexOwnAuth {
            upstream_url,
            bearer_token: trimmed_auth.to_string(),
            account_id,
        });
    }

    None
}

async fn forward_codex_own_signin_request(
    stream: &mut TcpStream,
    auth: &CodexOwnAuth,
    headers: &HashMap<String, String>,
    body_bytes: &[u8],
) -> Result<(u16, usize), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("构造 Codex 直通客户端失败: {e}"))?;

    let mut req = client
        .post(&auth.upstream_url)
        .bearer_auth(&auth.bearer_token)
        .body(body_bytes.to_vec());
    if let Some(acc_id) = &auth.account_id {
        req = req.header("ChatGPT-Account-Id", acc_id);
    }
    if !headers.contains_key("content-type") {
        req = req.header("content-type", "application/json");
    }
    for (k, v) in headers {
        if matches!(
            k.as_str(),
            "content-type"
                | "accept"
                | "user-agent"
                | "originator"
                | "openai-beta"
                | "session_id"
                | "conversation_id"
        ) || k.starts_with("x-codex-")
            || k.starts_with("x-oai-")
        {
            req = req.header(k.as_str(), v.as_str());
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Codex 自有账号直通上游失败: {e}"))?;
    let status = resp.status();
    let status_code = status.as_u16();
    let status_line = format!(
        "{status_code} {}",
        status.canonical_reason().unwrap_or("OK")
    );
    let header_ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let resp_bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取 Codex 上游响应失败: {e}"))?;
    let content_type = header_ct.unwrap_or_else(|| {
        if resp_bytes.starts_with(b"event:") || resp_bytes.starts_with(b"data:") {
            "text/event-stream; charset=utf-8".to_string()
        } else {
            "application/json".to_string()
        }
    });
    let resp_len = resp_bytes.len();
    write_http_response(
        stream,
        &status_line,
        &content_type,
        &resp_bytes,
        &[("Cache-Control", "no-cache")],
    )
    .await
    .map_err(|e| format!("回写 Codex 客户端响应失败: {e}"))?;
    Ok((status_code, resp_len))
}

fn extract_bearer_and_base_url(
    env: &CredentialEnvelope,
) -> (Option<String>, Option<String>, Vec<(&'static str, String)>) {
    match &env.payload {
        CredentialPayload::Token {
            access_token,
            extra,
            ..
        } => {
            let base_url = extra
                .get("base_url")
                .cloned()
                .filter(|s| !s.trim().is_empty());
            let mut headers = Vec::new();
            if env.tool_id == "workbuddy" {
                let domain = extra
                    .get("domain")
                    .cloned()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "copilot.tencent.com".into());
                headers.push((
                    "User-Agent",
                    "WorkBuddy/5.9.3 WorkBuddy/5.9.3 CLI/2.165.0".into(),
                ));
                headers.push(("X-User-Id", env.uid.clone()));
                headers.push(("X-Domain", domain));
                headers.push(("X-Product", "SaaS".into()));
                headers.push(("X-IDE-Type", "WorkBuddy".into()));
                headers.push(("X-IDE-Name", "WorkBuddy".into()));
                headers.push(("X-IDE-Version", "5.9.3".into()));
                headers.push(("X-Requested-With", "XMLHttpRequest".into()));
                headers.push(("X-Agent-Intent", "craft".into()));
                headers.push(("X-Agent-Purpose", "conversation".into()));
                headers.push(("X-Agent-Type", "main".into()));
                headers.push(("X-Agent-Runtime", "local-agent".into()));
                headers.push(("X-Stainless-Retry-Count", "0".into()));
                headers.push(("X-Stainless-Timeout", "1200".into()));
                headers.push(("X-Stainless-Lang", "js".into()));
                headers.push(("X-Stainless-Package-Version", "6.25.0".into()));
                headers.push(("X-Stainless-OS", "MacOS".into()));
                headers.push(("X-Stainless-Arch", "arm64".into()));
                headers.push(("X-Stainless-Runtime", "node".into()));
                headers.push(("X-Stainless-Runtime-Version", "v22.21.1".into()));
            } else if env.tool_id == "minimax_code_cn" {
                headers.push(("User-Agent", "MiniMaxAgent".into()));
                headers.push(("X-Mavis-Agent-Id", "main".into()));
                headers.push(("X-Mavis-Timezone-Offset", "28800".into()));
                headers.push(("accept-encoding", "identity".into()));
            } else if env.tool_id == "qoder_cn" {
                headers.push(("Cosy-ClientType", "10".into()));
            }
            (Some(access_token.clone()), base_url, headers)
        }
        CredentialPayload::LocalSnapshot { files } => {
            let tok = files
                .get("decrypted_token.txt")
                .and_then(|b64| {
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()
                })
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            (tok, None, Vec::new())
        }
        CredentialPayload::Cookies { user_agent, .. } => {
            let mut headers = Vec::new();
            if let Some(ua) = user_agent {
                headers.push(("User-Agent", ua.clone()));
            }
            (None, None, headers)
        }
    }
}

const WORKBUDDY_DEFAULT_SYSTEM: &str = "You are a helpful assistant.";

fn replace_ascii_case_insensitive(input: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    let nlen = needle.len();
    while i < input.len() {
        if input.len() - i >= nlen
            && input.is_char_boundary(i + nlen)
            && input[i..i + nlen].eq_ignore_ascii_case(needle)
        {
            out.push_str(replacement);
            i += nlen;
        } else if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn strip_anthropic_billing_headers(input: &str) -> String {
    const PREFIX: &str = "x-anthropic-billing-header:";
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    let plen = PREFIX.len();
    while i < input.len() {
        if input.len() - i >= plen
            && input.is_char_boundary(i + plen)
            && input[i..i + plen].eq_ignore_ascii_case(PREFIX)
        {
            if let Some(rel_nl) = input[i..].find('\n') {
                i += rel_nl + 1;
            } else {
                break;
            }
        } else if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn rewrite_codex_opener_if_unprefixed(input: &str) -> String {
    const NEEDLE: &str = "Codex CLI is an open source project led by OpenAI.";
    const REPL: &str = "The Codex CLI is an open source project led by OpenAI.";
    let mut out = String::with_capacity(input.len() + 8);
    let mut i = 0usize;
    let nlen = NEEDLE.len();
    while i < input.len() {
        if input.len() - i >= nlen
            && input.is_char_boundary(i + nlen)
            && input[i..i + nlen].eq_ignore_ascii_case(NEEDLE)
        {
            let has_the_prefix = i >= 4
                && input.is_char_boundary(i - 4)
                && input[i - 4..i].eq_ignore_ascii_case("the ");
            if has_the_prefix {
                out.push_str(&input[i..i + nlen]);
            } else {
                out.push_str(REPL);
            }
            i += nlen;
        } else if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn rewrite_web_gui_opener(input: &str) -> String {
    const HEAD: &str = "Web GUI whose URL the runtime context names.";
    const TAIL: &str = "When the user refers";
    const REPL: &str =
        "Web GUI whose URL the runtime context names. The interface, when the user refers";
    let mut out = String::with_capacity(input.len() + 20);
    let mut i = 0usize;
    let hlen = HEAD.len();
    let tlen = TAIL.len();
    while i < input.len() {
        if input.len() - i >= hlen
            && input.is_char_boundary(i + hlen)
            && input[i..i + hlen].eq_ignore_ascii_case(HEAD)
        {
            let mut ws_end = i + hlen;
            let mut saw_ws = false;
            while ws_end < input.len() {
                let b = input.as_bytes()[ws_end];
                if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
                    saw_ws = true;
                    ws_end += 1;
                } else {
                    break;
                }
            }
            if saw_ws
                && input.len() - ws_end >= tlen
                && input.is_char_boundary(ws_end + tlen)
                && input[ws_end..ws_end + tlen].eq_ignore_ascii_case(TAIL)
            {
                out.push_str(REPL);
                i = ws_end + tlen;
                continue;
            }
        }
        if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn insert_comma_in_generic_agent_openers(input: &str) -> String {
    const PHRASES: [&str; 5] = [
        "powered by",
        "running on",
        "running in",
        "built on",
        "based on",
    ];
    let mut out = String::with_capacity(input.len() + 8);
    let mut i = 0usize;
    while i < input.len() {
        let at_word_boundary = i == 0
            || !input[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let matched_prefix = if at_word_boundary {
            ["You are an ", "You are the ", "You are a "]
                .into_iter()
                .find(|p| {
                    input.len() - i >= p.len()
                        && input.is_char_boundary(i + p.len())
                        && input[i..i + p.len()].eq_ignore_ascii_case(p)
                })
        } else {
            None
        };
        if let Some(prefix) = matched_prefix {
            let after_prefix = i + prefix.len();
            let mut scan_end = after_prefix;
            let mut chars_scanned = 0usize;
            while scan_end < input.len() && chars_scanned < 90 {
                let Some(c) = input[scan_end..].chars().next() else {
                    break;
                };
                if matches!(c, '\n' | '.' | ',') {
                    break;
                }
                scan_end += c.len_utf8();
                chars_scanned += 1;
            }
            let slice = &input[after_prefix..scan_end];
            let mut matched_span: Option<(usize, usize)> = None;
            let mut pos = 0usize;
            while pos < slice.len() {
                let role_wb = pos == 0
                    || !slice[..pos]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_');
                let role_word = if role_wb {
                    ["agent", "assistant"].into_iter().find(|w| {
                        slice.len() - pos >= w.len()
                            && slice.is_char_boundary(pos + w.len())
                            && slice[pos..pos + w.len()].eq_ignore_ascii_case(w)
                    })
                } else {
                    None
                };
                if let Some(role) = role_word {
                    let ws_start = pos + role.len();
                    let mut ws_end = ws_start;
                    while ws_end < slice.len() && matches!(slice.as_bytes()[ws_end], b' ' | b'\t') {
                        ws_end += 1;
                    }
                    if ws_end > ws_start {
                        let abs_ws_start = after_prefix + ws_start;
                        let abs_ws_end = after_prefix + ws_end;
                        let phrase_match = PHRASES.into_iter().find(|ph| {
                            input.len() - abs_ws_end >= ph.len()
                                && input.is_char_boundary(abs_ws_end + ph.len())
                                && input[abs_ws_end..abs_ws_end + ph.len()].eq_ignore_ascii_case(ph)
                                && !input[abs_ws_end + ph.len()..]
                                    .chars()
                                    .next()
                                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
                        });
                        if let Some(ph) = phrase_match {
                            matched_span = Some((abs_ws_start, abs_ws_end + ph.len() - ph.len()));
                            break;
                        }
                    }
                }
                if let Some(c) = slice[pos..].chars().next() {
                    pos += c.len_utf8();
                } else {
                    break;
                }
            }
            if let Some((abs_ws_start, abs_phrase_start)) = matched_span {
                out.push_str(&input[i..abs_ws_start]);
                out.push_str(", ");
                i = abs_phrase_start;
                continue;
            }
        }
        if let Some(ch) = input[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

fn unflag_workbuddy_text(text: &str) -> String {
    let mut s = strip_anthropic_billing_headers(text);
    s = replace_ascii_case_insensitive(
        &s,
        "You are Claude Code, Anthropic's official CLI for Claude",
        "You are Claude Code, Anthropic's CLI for Claude",
    );
    s = replace_ascii_case_insensitive(
        &s,
        "You are Claude Code, Anthropic\u{2019}s official CLI for Claude",
        "You are Claude Code, Anthropic's CLI for Claude",
    );
    s = replace_ascii_case_insensitive(
        &s,
        "Main branch (you will usually use this for PRs)",
        "Main branch (usually the base for PRs)",
    );
    s = replace_ascii_case_insensitive(
        &s,
        "You are an AI agent powered by DeepSeek Harness.",
        "You are an AI agent, powered by DeepSeek Harness.",
    );
    s = replace_ascii_case_insensitive(
        &s,
        "You are an AI agent powered by DeepSeek Harness",
        "You are an AI agent, powered by DeepSeek Harness",
    );
    s = rewrite_web_gui_opener(&s);
    s = rewrite_codex_opener_if_unprefixed(&s);
    insert_comma_in_generic_agent_openers(&s)
}

fn extract_message_text(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                p.get("text")
                    .or_else(|| p.get("content"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn sanitize_workbuddy_chat_body(body: &mut Value) {
    if let Some(obj) = body.as_object_mut() {
        if !obj.contains_key("max_tokens") {
            if let Some(mct) = obj.remove("max_completion_tokens") {
                obj.insert("max_tokens".into(), mct);
            }
        }
    }
    let is_kimi_k31 = body
        .get("model")
        .and_then(Value::as_str)
        .map(|m| m.to_ascii_lowercase().starts_with("kimi-k3.1-pro"))
        .unwrap_or(false);

    if let Some(Value::Array(msgs)) = body.get_mut("messages") {
        for msg in msgs.iter_mut() {
            if let Some(obj) = msg.as_object_mut() {
                if obj.get("role").and_then(Value::as_str) == Some("developer") {
                    obj.insert("role".into(), json!("system"));
                }
                match obj.get_mut("content") {
                    Some(Value::String(s)) => {
                        *s = unflag_workbuddy_text(s);
                    }
                    Some(Value::Array(parts)) => {
                        for part in parts.iter_mut() {
                            if let Some(Value::String(t)) = part.get_mut("text") {
                                *t = unflag_workbuddy_text(t);
                            }
                        }
                    }
                    _ => {}
                }
                let is_assistant = obj.get("role").and_then(Value::as_str) == Some("assistant");
                let has_tool_calls = obj
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .is_some_and(|tc| !tc.is_empty());
                if is_kimi_k31 && is_assistant && has_tool_calls {
                    let is_empty = match obj.get("content") {
                        None | Some(Value::Null) => true,
                        Some(Value::String(s)) => s.is_empty(),
                        _ => false,
                    };
                    if is_empty {
                        obj.insert("content".into(), json!(" "));
                    }
                }
            }
        }
        msgs.retain(|m| {
            if m.get("role").and_then(Value::as_str) == Some("system") {
                !extract_message_text(m).trim().is_empty()
            } else {
                true
            }
        });
        let first_is_system = msgs
            .first()
            .and_then(|m| m.get("role"))
            .and_then(Value::as_str)
            == Some("system");
        if !first_is_system {
            msgs.insert(
                0,
                json!({ "role": "system", "content": WORKBUDDY_DEFAULT_SYSTEM }),
            );
        }
    }
}

fn move_system_to_user_for_workbuddy(body: &mut Value) -> bool {
    let Some(Value::Array(msgs)) = body.get_mut("messages") else {
        return false;
    };
    let mut lead_sys = Vec::new();
    let mut lead_count = 0usize;
    while lead_count < msgs.len()
        && msgs[lead_count].get("role").and_then(Value::as_str) == Some("system")
    {
        let txt = extract_message_text(&msgs[lead_count]);
        if !txt.trim().is_empty() {
            lead_sys.push(txt);
        }
        lead_count += 1;
    }
    let has_later_system = msgs[lead_count..]
        .iter()
        .any(|m| m.get("role").and_then(Value::as_str) == Some("system"));
    if (lead_sys.is_empty()
        || (lead_sys.len() == 1 && lead_sys[0].trim() == WORKBUDDY_DEFAULT_SYSTEM))
        && !has_later_system
    {
        return false;
    }

    msgs.drain(0..lead_count);
    for m in msgs.iter_mut() {
        if let Some(obj) = m.as_object_mut() {
            if obj.get("role").and_then(Value::as_str) == Some("system") {
                obj.insert("role".into(), json!("user"));
            }
        }
    }

    if !lead_sys.is_empty() {
        let wrapped = format!("<instructions>\n{}\n</instructions>", lead_sys.join("\n\n"));
        if let Some(user_idx) = msgs
            .iter()
            .position(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        {
            if let Some(obj) = msgs[user_idx].as_object_mut() {
                match obj.get_mut("content") {
                    Some(Value::String(s)) => {
                        if s.is_empty() {
                            *s = wrapped;
                        } else {
                            *s = format!("{wrapped}\n\n{s}");
                        }
                    }
                    Some(Value::Array(parts)) => {
                        parts.insert(0, json!({ "type": "text", "text": wrapped }));
                    }
                    _ => {
                        obj.insert("content".into(), json!(wrapped));
                    }
                }
            }
        } else {
            msgs.insert(0, json!({ "role": "user", "content": wrapped }));
        }
    }

    msgs.insert(
        0,
        json!({ "role": "system", "content": WORKBUDDY_DEFAULT_SYSTEM }),
    );
    true
}

fn is_workbuddy_channel_blocked(status: u16, body: &str) -> bool {
    status == 400 && (body.contains("11128") || body.contains("Illegal API invocation"))
}

async fn call_upstream_member(
    member: &PoolMember,
    cred: &CredentialEnvelope,
    ir_req: &IrRequest,
    upstream_model: &str,
) -> Result<(u16, String), String> {
    let client = shared_http_client();
    let (token_opt, custom_base, extra_headers) = extract_bearer_and_base_url(cred);
    let token = token_opt.ok_or_else(|| format!("账号 {} 缺少有效 API Token", member.label))?;

    let (_, bare_upstream_model) = crate::domain::split_scoped_model_id(upstream_model);
    let wire_model = resolve_vendor_wire_model_id(&member.tool_id, bare_upstream_model);
    let protocol = upstream_model_protocol(&member.tool_id, &wire_model);
    let base_url = custom_base.unwrap_or_else(|| match member.tool_id.as_str() {
        "minimax_code_cn" => "https://agent.minimax.cn/mavis/api/v1/llm".into(),
        "workbuddy" => "https://copilot.tencent.com".into(),
        "qoder_cn" => "https://gateway.qoder.com.cn".into(),
        "trae_cn" => "https://trae-api-cn.mchost.guru".into(),
        _ => "https://api.openai.com".into(),
    });
    let base = base_url.trim_end_matches('/');

    let (url, mut body, auth_is_x_api_key) = match protocol {
        WireProtocolKind::AnthropicMessages => (
            format!("{base}/v1/messages"),
            to_upstream_anthropic_body(ir_req, &wire_model, false),
            true,
        ),
        _ if member.tool_id == "workbuddy" => (
            format!("{base}/v2/chat/completions"),
            to_upstream_openai_chat_body(ir_req, &wire_model, true),
            false,
        ),
        _ => (
            format!("{base}/v1/chat/completions"),
            to_upstream_openai_chat_body(ir_req, &wire_model, false),
            false,
        ),
    };

    let is_workbuddy_chat =
        member.tool_id == "workbuddy" && protocol != WireProtocolKind::AnthropicMessages;
    if is_workbuddy_chat {
        sanitize_workbuddy_chat_body(&mut body);
    }

    for attempt in 0..2 {
        let mut req = client.post(&url).json(&body);
        if member.tool_id == "minimax_code_cn" {
            req = req
                .bearer_auth(&token)
                .header("x-api-key", "minimax-agent")
                .header("anthropic-version", "2023-06-01")
                .header("X-Mavis-Session-Id", Uuid::new_v4().to_string());
        } else if auth_is_x_api_key {
            req = req
                .header("x-api-key", &token)
                .header("anthropic-version", "2023-06-01");
        } else if member.tool_id == "trae_cn" {
            req = req.header("Authorization", format!("Cloud-IDE-JWT {token}"));
        } else {
            req = req.bearer_auth(&token);
        }
        if is_workbuddy_chat {
            req = req
                .header("X-Conversation-ID", Uuid::new_v4().simple().to_string())
                .header(
                    "X-Conversation-Request-ID",
                    Uuid::new_v4().simple().to_string(),
                )
                .header(
                    "X-Conversation-Message-ID",
                    Uuid::new_v4().simple().to_string(),
                )
                .header("X-Request-ID", Uuid::new_v4().simple().to_string())
                .header("Accept", "text/event-stream");
        }
        for (k, v) in &extra_headers {
            req = req.header(*k, v);
        }

        let resp = req.send().await.map_err(|e| format!("上游请求失败: {e}"))?;
        let code = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if attempt == 0
            && is_workbuddy_chat
            && is_workbuddy_channel_blocked(code, &text)
            && move_system_to_user_for_workbuddy(&mut body)
        {
            continue;
        }
        return Ok((code, text));
    }
    unreachable!()
}

fn parse_upstream_response_body(body_txt: &str, model: &str) -> IrResponse {
    let trimmed_body = body_txt.trim();
    if let Ok(parsed) = serde_json::from_str::<Value>(trimmed_body) {
        if parsed.is_object() {
            let mut text = parsed
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_default();
            let mut thinking = parsed
                .pointer("/choices/0/message/reasoning_content")
                .or_else(|| parsed.pointer("/choices/0/message/reasoning"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_default();
            let mut tool_calls: Vec<(String, String, String)> = parsed
                .pointer("/choices/0/message/tool_calls")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .enumerate()
                        .filter_map(|(idx, tc)| {
                            let name = tc.pointer("/function/name").and_then(Value::as_str)?;
                            let id = tc
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("call_{idx}"));
                            let args = tc
                                .pointer("/function/arguments")
                                .and_then(Value::as_str)
                                .unwrap_or("{}")
                                .to_string();
                            Some((id, name.to_string(), args))
                        })
                        .collect()
                })
                .unwrap_or_default();

            if let Some(blocks) = parsed.get("content").and_then(Value::as_array) {
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(s) = block.get("text").and_then(Value::as_str) {
                                text.push_str(s);
                            }
                        }
                        Some("thinking") => {
                            if let Some(s) = block.get("thinking").and_then(Value::as_str) {
                                thinking.push_str(s);
                            }
                        }
                        Some("tool_use") => {
                            if let Some(name) = block.get("name").and_then(Value::as_str) {
                                let id = block
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("call_0")
                                    .to_string();
                                let args = block
                                    .get("input")
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "{}".into());
                                tool_calls.push((id, name.to_string(), args));
                            }
                        }
                        _ => {}
                    }
                }
            }

            let finish_reason = parsed
                .pointer("/choices/0/finish_reason")
                .or_else(|| parsed.get("stop_reason"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if tool_calls.is_empty() {
                        "stop".into()
                    } else {
                        "tool_calls".into()
                    }
                });
            let usage = IrUsage {
                input_tokens: parsed
                    .pointer("/usage/prompt_tokens")
                    .or_else(|| parsed.pointer("/usage/input_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(100),
                output_tokens: parsed
                    .pointer("/usage/completion_tokens")
                    .or_else(|| parsed.pointer("/usage/output_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(64),
            };
            return IrResponse {
                id: Uuid::new_v4().simple().to_string()[..12].to_string(),
                model: model.to_string(),
                text,
                thinking: if thinking.is_empty() {
                    None
                } else {
                    Some(thinking)
                },
                tool_calls,
                finish_reason,
                usage,
            };
        }
    }

    let mut text = String::new();
    let mut thinking = String::new();
    let mut tool_calls_map: std::collections::BTreeMap<usize, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut finish_reason: Option<String> = None;
    let mut input_tokens: Option<u64> = None;
    let mut output_tokens: Option<u64> = None;
    let mut saw_sse_data = false;

    for line in body_txt.lines() {
        let trimmed = line.trim();
        let Some(raw_data) = trimmed.strip_prefix("data:") else {
            continue;
        };
        let data = raw_data.trim();
        if data.is_empty() || data == "[DONE]" {
            saw_sse_data = true;
            continue;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        saw_sse_data = true;

        if let Some(u) = chunk.get("usage").filter(|u| !u.is_null()) {
            if let Some(v) = u
                .get("prompt_tokens")
                .or_else(|| u.get("input_tokens"))
                .and_then(Value::as_u64)
            {
                input_tokens = Some(v);
            }
            if let Some(v) = u
                .get("completion_tokens")
                .or_else(|| u.get("output_tokens"))
                .and_then(Value::as_u64)
            {
                output_tokens = Some(v);
            }
        }

        if let Some(c0) = chunk.pointer("/choices/0") {
            if let Some(fr) = c0.get("finish_reason").and_then(Value::as_str) {
                finish_reason = Some(fr.to_string());
            }
            if let Some(delta) = c0.get("delta") {
                if let Some(s) = delta.get("content").and_then(Value::as_str) {
                    text.push_str(s);
                }
                if let Some(r) = delta
                    .get("reasoning_content")
                    .or_else(|| delta.get("reasoning"))
                    .and_then(Value::as_str)
                {
                    thinking.push_str(r);
                }
                if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tc in tcs {
                        let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        let entry = tool_calls_map.entry(idx).or_insert_with(|| {
                            (format!("call_{idx}"), String::new(), String::new())
                        });
                        if let Some(id) = tc
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                        {
                            entry.0 = id.to_string();
                        }
                        if let Some(name) = tc.pointer("/function/name").and_then(Value::as_str) {
                            entry.1.push_str(name);
                        }
                        if let Some(args) =
                            tc.pointer("/function/arguments").and_then(Value::as_str)
                        {
                            entry.2.push_str(args);
                        }
                    }
                }
            }
        }

        match chunk.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(u) = chunk.pointer("/message/usage") {
                    if let Some(v) = u.get("input_tokens").and_then(Value::as_u64) {
                        input_tokens = Some(v);
                    }
                    if let Some(v) = u.get("output_tokens").and_then(Value::as_u64) {
                        output_tokens = Some(v);
                    }
                }
            }
            Some("content_block_start") => {
                let idx = chunk.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if chunk.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use")
                {
                    let id = chunk
                        .pointer("/content_block/id")
                        .and_then(Value::as_str)
                        .unwrap_or("call_0")
                        .to_string();
                    let name = chunk
                        .pointer("/content_block/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    tool_calls_map.insert(idx, (id, name, String::new()));
                }
            }
            Some("content_block_delta") => {
                let idx = chunk.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let Some(delta) = chunk.get("delta") {
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            if let Some(s) = delta.get("text").and_then(Value::as_str) {
                                text.push_str(s);
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(s) = delta.get("thinking").and_then(Value::as_str) {
                                thinking.push_str(s);
                            }
                        }
                        Some("input_json_delta") => {
                            if let Some(s) = delta.get("partial_json").and_then(Value::as_str) {
                                let entry = tool_calls_map.entry(idx).or_insert_with(|| {
                                    (format!("call_{idx}"), String::new(), String::new())
                                });
                                entry.2.push_str(s);
                            }
                        }
                        _ => {
                            if let Some(s) = delta.get("text").and_then(Value::as_str) {
                                text.push_str(s);
                            }
                        }
                    }
                }
            }
            Some("message_delta") => {
                if let Some(sr) = chunk.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    finish_reason = Some(sr.to_string());
                }
            }
            _ => {}
        }
    }

    if !saw_sse_data && !trimmed_body.is_empty() {
        text = body_txt.to_string();
    }

    let tool_calls: Vec<(String, String, String)> = tool_calls_map
        .into_values()
        .filter(|(_, name, _)| !name.is_empty())
        .collect();
    let final_finish = finish_reason.unwrap_or_else(|| {
        if tool_calls.is_empty() {
            "stop".into()
        } else {
            "tool_calls".into()
        }
    });

    IrResponse {
        id: Uuid::new_v4().simple().to_string()[..12].to_string(),
        model: model.to_string(),
        text,
        thinking: if thinking.is_empty() {
            None
        } else {
            Some(thinking)
        },
        tool_calls,
        finish_reason: final_finish,
        usage: IrUsage {
            input_tokens: input_tokens.unwrap_or(100),
            output_tokens: output_tokens.unwrap_or(64),
        },
    }
}

async fn handle_http_connection(
    mut stream: TcpStream,
    runtime: Arc<GatewayRuntime>,
    db: Arc<QwitchDatabase>,
    master_key: [u8; 32],
    adapters: Arc<HashMap<String, Arc<dyn ToolAdapter>>>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await? == 0 {
        return Ok(());
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return write_http_response(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            b"Bad Request",
            &[],
        )
        .await;
    }
    let method = parts[0].to_string();
    let full_target = parts[1];
    let (path, query_str) = match full_target.split_once('?') {
        Some((p, q)) => (p.to_string(), q),
        None => (full_target.to_string(), ""),
    };

    let mut headers: HashMap<String, String> = HashMap::new();
    let mut content_len: usize = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_string();
            if key == "content-length" {
                content_len = val.parse().unwrap_or(0);
            }
            headers.insert(key, val);
        }
    }

    // 环回安全校验：防 DNS Rebinding 与浏览器跨站注入
    let host_hdr = headers.get("host").map(String::as_str).unwrap_or("");
    if !is_valid_loopback_host(host_hdr) {
        return write_http_response(
            stream_from_reader(reader),
            "403 Forbidden",
            "application/json",
            br#"{"error":"Invalid Host header"}"#,
            &[],
        )
        .await;
    }
    let has_browser_origin = headers.contains_key("origin");
    let cross_site = headers
        .get("sec-fetch-site")
        .is_some_and(|s| s.eq_ignore_ascii_case("cross-site"));
    if has_browser_origin || cross_site {
        return write_http_response(
            stream_from_reader(reader),
            "403 Forbidden",
            "application/json",
            br#"{"error":"Browser cross-origin blocked"}"#,
            &[],
        )
        .await;
    }

    let mut body_bytes = vec![0u8; content_len.min(16 * 1024 * 1024)];
    if !body_bytes.is_empty() {
        reader.read_exact(&mut body_bytes).await?;
    }
    let stream = stream_from_reader(reader);

    // WebSocket Upgrade 426 降级（Codex Desktop Responses 自动降级 HTTP SSE）
    if headers
        .get("upgrade")
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
    {
        return write_http_response(
            stream,
            "426 Upgrade Required",
            "application/json",
            br#"{"error":"WebSocket disabled; use HTTP SSE POST"}"#,
            &[("Upgrade", "HTTP/1.1"), ("Connection", "Upgrade")],
        )
        .await;
    }

    let auth_token = headers
        .get("authorization")
        .and_then(|s| {
            s.strip_prefix("Bearer ")
                .or_else(|| s.strip_prefix("bearer "))
        })
        .or_else(|| headers.get("x-api-key").map(String::as_str))
        .unwrap_or("");
    let query_agent = query_str
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == "agent")
        .and_then(|(_, v)| TargetAgentId::from_id(v));
    let ua_agent = headers.get("user-agent").and_then(|ua| {
        let lower = ua.to_ascii_lowercase();
        if lower.contains("claude") {
            Some(TargetAgentId::ClaudeDesktop)
        } else if lower.contains("codex") {
            Some(TargetAgentId::CodexDesktop)
        } else if lower.contains("dsh") || lower.contains("deepseek") {
            Some(TargetAgentId::Dsh)
        } else if lower.contains("opencode") {
            Some(TargetAgentId::Opencode)
        } else {
            None
        }
    });
    let actor_agent = headers
        .get("x-openai-actor-authorization")
        .filter(|v| v.trim().eq_ignore_ascii_case("qwitch"))
        .map(|_| TargetAgentId::CodexDesktop);
    // 强制鉴权：仅接受携带有效绑定令牌的请求，杜绝任意本机进程白嫖号池额度或本机 OAuth。
    let token_agent = agent_id_from_token(auth_token);
    if token_agent.is_none() {
        return write_http_response(
            stream,
            "401 Unauthorized",
            "application/json",
            br#"{"error":"Missing or invalid gateway binding token"}"#,
            &[],
        )
        .await;
    }
    let caller_agent = token_agent
        .or(query_agent)
        .or(actor_agent)
        .or(ua_agent)
        .or_else(|| {
            if path.starts_with("/v1/responses") {
                Some(TargetAgentId::CodexDesktop)
            } else {
                None
            }
        });

    let cooldowns = runtime.cooldown_snapshot();
    let pools = db.list_pools(&master_key, &cooldowns).unwrap_or_default();

    if method == "GET" && path == "/v1/models" {
        let bindings = db.get_stored_agent_bindings().unwrap_or_default();
        let target_agent = caller_agent.unwrap_or(TargetAgentId::Opencode);
        let mounted_ids = bindings
            .iter()
            .find(|b| b.agent_id == target_agent)
            .map(|b| b.mounted_pool_ids.clone())
            .unwrap_or_else(|| {
                pools
                    .iter()
                    .filter(|p| p.enabled)
                    .map(|p| p.id.clone())
                    .collect()
            });
        let exposed = compute_agent_exposed_models(
            target_agent,
            &mounted_ids,
            None,
            &pools,
            chrono::Utc::now().timestamp_millis(),
        );
        let first_id = exposed.first().map(|m| m.wire_model_id.clone());
        let last_id = exposed.last().map(|m| m.wire_model_id.clone());
        let data: Vec<Value> = exposed
            .into_iter()
            .map(|m| {
                let (scoped_tid, bare_mid) = crate::domain::split_scoped_model_id(&m.model_id);
                let tool_id = scoped_tid
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        pools
                            .iter()
                            .find(|p| p.id == m.pool_id)
                            .and_then(|p| {
                                p.members.iter().find(|mb| {
                                    mb.supported_models.iter().any(|s| {
                                        s == &m.model_id
                                            || crate::domain::split_scoped_model_id(s).1 == bare_mid
                                    })
                                })
                            })
                            .map(|mb| mb.tool_id.clone())
                    })
                    .unwrap_or_else(|| "workbuddy".into());
                let vendor_wire = resolve_vendor_wire_model_id(&tool_id, bare_mid);
                json!({
                    "id": m.wire_model_id,
                    "object": "model",
                    "type": "model",
                    "name": m.display_name,
                    "display_name": m.display_name,
                    "created": 1735689600,
                    "created_at": "2025-01-01T00:00:00Z",
                    "owned_by": format!("qwitch:{}", m.pool_id),
                    "tool_id": tool_id,
                    "vendor_wire_id": vendor_wire
                })
            })
            .collect();
        let out = json!({
            "object": "list",
            "data": data,
            "has_more": false,
            "first_id": first_id,
            "last_id": last_id
        });
        return write_http_response(
            stream,
            "200 OK",
            "application/json",
            out.to_string().as_bytes(),
            &[],
        )
        .await;
    }

    if method == "POST" && path == "/v1/messages/count_tokens" {
        let est = (body_bytes.len() / 4).max(16);
        let out = json!({ "input_tokens": est });
        return write_http_response(
            stream,
            "200 OK",
            "application/json",
            out.to_string().as_bytes(),
            &[],
        )
        .await;
    }

    let body_json: Value = match serde_json::from_slice(&body_bytes) {
        Ok(v) => v,
        Err(_) => {
            return write_http_response(
                stream,
                "400 Bad Request",
                "application/json",
                br#"{"error":"Invalid JSON body"}"#,
                &[],
            )
            .await;
        }
    };

    let ir_req = match path.as_str() {
        "/v1/messages" => parse_anthropic_messages_request(&body_json),
        "/v1/responses" | "/v1/responses/compact" => parse_openai_responses_request(&body_json),
        "/v1/chat/completions" => parse_openai_chat_request(&body_json),
        _ => {
            return write_http_response(
                stream,
                "404 Not Found",
                "application/json",
                br#"{"error":"Unsupported gateway path"}"#,
                &[],
            )
            .await;
        }
    };

    let ir_req = match ir_req {
        Ok(r) => r,
        Err(msg) => {
            let err = json!({ "error": { "message": msg } });
            return write_http_response(
                stream,
                "400 Bad Request",
                "application/json",
                err.to_string().as_bytes(),
                &[],
            )
            .await;
        }
    };

    let (pool, requested_model) =
        match resolve_target_pool_and_model(&pools, caller_agent, &ir_req.model) {
            Some(found) => found,
            None => {
                if matches!(path.as_str(), "/v1/responses" | "/v1/responses/compact")
                    || caller_agent == Some(TargetAgentId::CodexDesktop)
                {
                    if let Some(own_auth) =
                        resolve_codex_own_auth(&path, &headers, auth_token, None)
                    {
                        let t_start = std::time::Instant::now();
                        match forward_codex_own_signin_request(
                            stream,
                            &own_auth,
                            &headers,
                            &body_bytes,
                        )
                        .await
                        {
                            Ok((code, resp_len)) => {
                                let ok = (200..300).contains(&code);
                                let latency_ms = t_start.elapsed().as_millis() as u64;
                                let trace = RoutingTraceItem {
                                    id: format!("tr-{}", &Uuid::new_v4().simple().to_string()[..8]),
                                    timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                                    agent_id: TargetAgentId::CodexDesktop,
                                    pool_id: "codex-native".into(),
                                    pool_name: "Codex 自有账号直通".into(),
                                    incoming_protocol: ir_req.incoming_protocol,
                                    upstream_protocol: WireProtocolKind::OpenaiResponses,
                                    requested_model: ir_req.model.clone(),
                                    final_account_label: "Codex 自有登录态".into(),
                                    final_tool_id: "codex".into(),
                                    final_model: ir_req.model.clone(),
                                    status: if ok {
                                        RoutingTraceStatus::Success
                                    } else {
                                        RoutingTraceStatus::Failed
                                    },
                                    cache_affinity_hit: false,
                                    ttft_ms: latency_ms,
                                    total_tokens: (resp_len / 4) as u64,
                                    attempts: vec![RoutingAttempt {
                                        account_label: "Codex 自有登录态".into(),
                                        tool_id: "codex".into(),
                                        model: ir_req.model.clone(),
                                        tier: None,
                                        outcome: if ok {
                                            RoutingAttemptOutcome::Ok
                                        } else if code == 401 || code == 403 {
                                            RoutingAttemptOutcome::Refreshed401
                                        } else {
                                            RoutingAttemptOutcome::Cooldown429
                                        },
                                        latency_ms,
                                        detail: format!("HTTP {code} -> {}", own_auth.upstream_url),
                                    }],
                                };
                                let _ = db.append_routing_trace(&trace);
                                return Ok(());
                            }
                            Err(msg) => {
                                let err = json!({ "error": { "message": msg } });
                                return write_http_response(
                                    stream,
                                    "502 Bad Gateway",
                                    "application/json",
                                    err.to_string().as_bytes(),
                                    &[],
                                )
                                .await;
                            }
                        }
                    }
                }

                if caller_agent == Some(TargetAgentId::CodexDesktop) {
                    if let Some(found) = pools.iter().find(|p| p.enabled).and_then(|p| {
                        p.model_policies
                            .iter()
                            .find(|m| m.enabled)
                            .map(|m| (p.clone(), m.model_id.clone()))
                    }) {
                        found
                    } else {
                        return write_http_response(
                            stream,
                            "404 Not Found",
                            "application/json",
                            br#"{"error":"No active pool matches requested model"}"#,
                            &[],
                        )
                        .await;
                    }
                } else {
                    return write_http_response(
                        stream,
                        "404 Not Found",
                        "application/json",
                        br#"{"error":"No active pool matches requested model"}"#,
                        &[],
                    )
                    .await;
                }
            }
        };

    let session_key = headers
        .get("x-conversation-id")
        .or_else(|| headers.get("x-session-id"))
        .cloned();
    let affinity_snap = runtime
        .inner
        .lock()
        .expect("gateway lock")
        .session_affinity
        .clone();
    let now = chrono::Utc::now().timestamp_millis();
    let plan = select_candidates_for_request(
        &pool,
        &requested_model,
        session_key.as_deref(),
        &cooldowns,
        &affinity_snap,
        now,
    );

    if plan.candidates.is_empty() {
        let msg = plan
            .diagnostic_reason
            .unwrap_or_else(|| "号池内暂无可用成员".into());
        let err = json!({ "error": { "message": msg } });
        return write_http_response(
            stream,
            "503 Service Unavailable",
            "application/json",
            err.to_string().as_bytes(),
            &[],
        )
        .await;
    }

    let mut attempts = plan.skipped_attempts;
    let mut final_ir_resp: Option<(IrResponse, PoolMember)> = None;
    let t_start = std::time::Instant::now();

    for cand in &plan.candidates {
        let Ok((_, mut cred)) = db.get_account_credential(&master_key, &cand.account_id) else {
            continue;
        };
        let t_hop = std::time::Instant::now();
        let mut res = call_upstream_member(cand, &cred, &ir_req, &plan.effective_model).await;

        if let Ok((401, _)) = res {
            if pool.auto_refresh_on_401 {
                if let Some(adapter) = adapters.get(&cand.tool_id) {
                    let lock = runtime.refresh_lock_for(&cand.account_id);
                    let _guard = lock.lock().await;
                    if let Ok((_, fresh)) = db.get_account_credential(&master_key, &cand.account_id)
                    {
                        cred = fresh;
                    }
                    if let Ok(Some(refreshed)) = adapter.refresh_credential(&cred).await {
                        let _ = db.upsert_account(&master_key, None, &refreshed);
                        cred = refreshed;
                        attempts.push(RoutingAttempt {
                            account_label: cand.label.clone(),
                            tool_id: cand.tool_id.clone(),
                            model: plan.effective_model.clone(),
                            tier: Some(cand.tier),
                            outcome: RoutingAttemptOutcome::Refreshed401,
                            latency_ms: t_hop.elapsed().as_millis() as u64,
                            detail: "401 单飞刷新凭据并回写加密库".into(),
                        });
                        res =
                            call_upstream_member(cand, &cred, &ir_req, &plan.effective_model).await;
                    }
                }
            }
        }

        match res {
            Ok((code, body_txt)) if (200..300).contains(&code) => {
                let ir_resp = parse_upstream_response_body(&body_txt, &ir_req.model);
                attempts.push(RoutingAttempt {
                    account_label: cand.label.clone(),
                    tool_id: cand.tool_id.clone(),
                    model: plan.effective_model.clone(),
                    tier: Some(cand.tier),
                    outcome: RoutingAttemptOutcome::Ok,
                    latency_ms: t_hop.elapsed().as_millis() as u64,
                    detail: format!("真实网关路由成功返回 ({code})"),
                });
                final_ir_resp = Some((ir_resp, cand.clone()));
                break;
            }
            Ok((429, _)) => {
                runtime.set_member_cooldown(
                    &cand.account_id,
                    chrono::Utc::now().timestamp_millis() + (pool.cooldown_seconds as i64) * 1000,
                );
                attempts.push(RoutingAttempt {
                    account_label: cand.label.clone(),
                    tool_id: cand.tool_id.clone(),
                    model: plan.effective_model.clone(),
                    tier: Some(cand.tier),
                    outcome: RoutingAttemptOutcome::Cooldown429,
                    latency_ms: t_hop.elapsed().as_millis() as u64,
                    detail: format!(
                        "HTTP 429 限流，置入 {}s 冷却并切换下一账号",
                        pool.cooldown_seconds
                    ),
                });
            }
            Ok((code, err_body)) => {
                attempts.push(RoutingAttempt {
                    account_label: cand.label.clone(),
                    tool_id: cand.tool_id.clone(),
                    model: plan.effective_model.clone(),
                    tier: Some(cand.tier),
                    outcome: RoutingAttemptOutcome::Cooldown429,
                    latency_ms: t_hop.elapsed().as_millis() as u64,
                    detail: format!(
                        "HTTP {code}: {}",
                        err_body.chars().take(80).collect::<String>()
                    ),
                });
            }
            Err(e) => {
                attempts.push(RoutingAttempt {
                    account_label: cand.label.clone(),
                    tool_id: cand.tool_id.clone(),
                    model: plan.effective_model.clone(),
                    tier: Some(cand.tier),
                    outcome: RoutingAttemptOutcome::Cooldown429,
                    latency_ms: t_hop.elapsed().as_millis() as u64,
                    detail: e,
                });
            }
        }
    }

    let Some((ir_resp, winner)) = final_ir_resp else {
        let trace = RoutingTraceItem {
            id: format!("tr-{}", &Uuid::new_v4().simple().to_string()[..8]),
            timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            agent_id: caller_agent.unwrap_or(TargetAgentId::Opencode),
            pool_id: pool.id.clone(),
            pool_name: pool.name.clone(),
            incoming_protocol: ir_req.incoming_protocol,
            upstream_protocol: WireProtocolKind::OpenaiChat,
            requested_model,
            final_account_label: "全部候选均失败".into(),
            final_tool_id: "custom_api_key".into(),
            final_model: "—".into(),
            status: RoutingTraceStatus::Failed,
            cache_affinity_hit: false,
            ttft_ms: t_start.elapsed().as_millis() as u64,
            total_tokens: 0,
            attempts,
        };
        let _ = db.append_routing_trace(&trace);
        return write_http_response(
            stream,
            "502 Bad Gateway",
            "application/json",
            br#"{"error":"All pool candidates failed before first byte"}"#,
            &[],
        )
        .await;
    };

    let trace = RoutingTraceItem {
        id: format!("tr-{}", &Uuid::new_v4().simple().to_string()[..8]),
        timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
        agent_id: caller_agent.unwrap_or(TargetAgentId::Opencode),
        pool_id: pool.id.clone(),
        pool_name: pool.name.clone(),
        incoming_protocol: ir_req.incoming_protocol,
        upstream_protocol: upstream_model_protocol(&winner.tool_id, &plan.effective_model),
        requested_model,
        final_account_label: winner.label.clone(),
        final_tool_id: winner.tool_id.clone(),
        final_model: plan.effective_model.clone(),
        status: if attempts.len() > 1 {
            RoutingTraceStatus::FailoverSuccess
        } else {
            RoutingTraceStatus::Success
        },
        cache_affinity_hit: plan.cache_affinity_hit,
        ttft_ms: t_start.elapsed().as_millis() as u64,
        total_tokens: ir_resp.usage.input_tokens + ir_resp.usage.output_tokens,
        attempts,
    };
    let _ = db.append_routing_trace(&trace);
    let _ = db.increment_pool_stats(
        &pool.id,
        trace.status == RoutingTraceStatus::FailoverSuccess,
        plan.cache_affinity_hit,
    );

    if ir_req.stream {
        let sse = match ir_req.incoming_protocol {
            WireProtocolKind::AnthropicMessages => to_anthropic_messages_sse(&ir_resp),
            WireProtocolKind::OpenaiResponses => to_openai_responses_sse(&ir_resp),
            WireProtocolKind::OpenaiChat => to_openai_chat_sse(&ir_resp),
        };
        write_http_response(
            stream,
            "200 OK",
            "text/event-stream; charset=utf-8",
            sse.as_bytes(),
            &[("Cache-Control", "no-cache")],
        )
        .await
    } else {
        let payload = match ir_req.incoming_protocol {
            WireProtocolKind::AnthropicMessages => to_anthropic_messages_response(&ir_resp),
            WireProtocolKind::OpenaiResponses => to_openai_responses_response(&ir_resp),
            WireProtocolKind::OpenaiChat => to_openai_chat_response(&ir_resp),
        };
        write_http_response(
            stream,
            "200 OK",
            "application/json",
            payload.to_string().as_bytes(),
            &[],
        )
        .await
    }
}

fn stream_from_reader(reader: BufReader<&mut TcpStream>) -> &mut TcpStream {
    reader.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AffinityMode, CreatePoolInput, CreatePoolMemberInput, MemberTier, PoolStrategy,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn gateway_serves_models_blocks_browser_origin_and_downgrades_ws_to_426() {
        let root = std::env::temp_dir().join(format!("qwitch-gw-{}", Uuid::new_v4()));
        let db = Arc::new(QwitchDatabase::open(&root.join("qwitch.db")).unwrap());
        let key = [4u8; 32];

        let mock_upstream = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": "pong from mock upstream" } }],
                "usage": { "prompt_tokens": 12, "completion_tokens": 8 }
            })))
            .mount(&mock_upstream)
            .await;

        let mut extra = HashMap::new();
        extra.insert("base_url".into(), mock_upstream.uri());
        let acc = db
            .upsert_account(
                &key,
                Some("API兜底"),
                &CredentialEnvelope {
                    tool_id: "custom_api_key".into(),
                    uid: "sk-test".into(),
                    display_name: Some("API兜底".into()),
                    display_email: None,
                    payload: CredentialPayload::Token {
                        access_token: "sk-test-token".into(),
                        refresh_token: None,
                        expires_at: None,
                        extra,
                    },
                },
            )
            .unwrap();

        let _pool = db
            .create_pool(
                &key,
                &CreatePoolInput {
                    id: None,
                    name: "Gateway Pool".into(),
                    description: "".into(),
                    scene_tag: "Coding".into(),
                    strategy: PoolStrategy::Order,
                    affinity_mode: AffinityMode::AutoCache,
                    skip_active_client_account: false,
                    cooldown_seconds: 60,
                    auto_refresh_on_401: false,
                    fallback_api_key_enabled: true,
                    members: vec![CreatePoolMemberInput {
                        account_id: acc.id,
                        supported_models: vec!["deepseek-v4-pro".into()],
                        tier: MemberTier::T3Fallback,
                    }],
                    enabled_model_ids: None,
                },
            )
            .unwrap();

        let rt = Arc::new(GatewayRuntime::new());
        {
            rt.inner.lock().unwrap().port = 0; // ephemeral port for test
        }
        let port = rt
            .start_server(Arc::clone(&db), key, Arc::new(HashMap::new()))
            .await
            .expect("start gateway");

        let client = reqwest::Client::new();

        // 0. /v1/models 返回兼容 Anthropic / OpenAI / DSH 的完整字段并支持 User-Agent 识别 Claude
        let models_resp = client
            .get(format!("http://127.0.0.1:{port}/v1/models"))
            .header("User-Agent", "ClaudeDesktop/2.7")
            .bearer_auth("qwitch-bind-claude-desktop")
            .send()
            .await
            .unwrap();
        assert_eq!(models_resp.status().as_u16(), 200);
        let models_json: Value = models_resp.json().await.unwrap();
        assert_eq!(models_json["has_more"], false);
        assert_eq!(models_json["data"][0]["type"], "model");
        assert_eq!(models_json["data"][0]["name"], "deepseek-v4-pro");
        assert_eq!(models_json["data"][0]["display_name"], "deepseek-v4-pro");
        assert!(models_json["data"][0]["id"]
            .as_str()
            .unwrap()
            .starts_with("mythos-qwitch-"));

        // 1. 浏览器 Origin 请求必须被 403 拦截
        let evil = client
            .get(format!("http://127.0.0.1:{port}/v1/models"))
            .header("Origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(evil.status().as_u16(), 403);

        // 1b. 缺少 / 伪造绑定令牌必须 401（防止任意本机进程白嫖）
        let no_token = client
            .get(format!("http://127.0.0.1:{port}/v1/models"))
            .send()
            .await
            .unwrap();
        assert_eq!(no_token.status().as_u16(), 401);
        let bad_token = client
            .get(format!("http://127.0.0.1:{port}/v1/models"))
            .bearer_auth("qwitch-bind-evil")
            .send()
            .await
            .unwrap();
        assert_eq!(bad_token.status().as_u16(), 401);

        // 2. Codex WebSocket 升级返回 426
        let ws = client
            .post(format!("http://127.0.0.1:{port}/v1/responses"))
            .header("Upgrade", "websocket")
            .send()
            .await
            .unwrap();
        assert_eq!(ws.status().as_u16(), 426);

        // 3. OpenAI Chat 正常请求打通上游 Mock 并写库 trace
        let chat = client
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .bearer_auth("qwitch-bind-opencode")
            .json(&json!({
                "model": "gateway-pool/deepseek-v4-pro",
                "messages": [{ "role": "user", "content": "ping" }]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(chat.status().as_u16(), 200);
        let body: Value = chat.json().await.unwrap();
        assert_eq!(
            body["choices"][0]["message"]["content"],
            "pong from mock upstream"
        );

        let traces = db.list_routing_traces(10).unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].final_model, "deepseek-v4-pro");

        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn codex_unmatched_model_resolves_own_signin_and_forwards_upstream() {
        assert!(resolve_target_pool_and_model(
            &[],
            Some(TargetAgentId::CodexDesktop),
            "gpt-6-luna"
        )
        .is_none());

        let tmp = std::env::temp_dir().join(format!("qwitch-codex-own-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("auth.json"),
            serde_json::to_string_pretty(&json!({
                "auth_mode": "chatgpt",
                "OPENAI_API_KEY": null,
                "tokens": {
                    "access_token": "oauth-jwt-sample",
                    "account_id": "acc-uuid-123"
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let headers = HashMap::new();
        let resolved = resolve_codex_own_auth(
            "/v1/responses",
            &headers,
            "qwitch-bind-codex-desktop",
            Some(&tmp),
        )
        .expect("should resolve ChatGPT OAuth from auth.json");
        assert_eq!(
            resolved.upstream_url,
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(resolved.bearer_token, "oauth-jwt-sample");
        assert_eq!(resolved.account_id.as_deref(), Some("acc-uuid-123"));

        let compact = resolve_codex_own_auth(
            "/v1/responses/compact",
            &headers,
            "qwitch-bind-codex-desktop",
            Some(&tmp),
        )
        .expect("compact endpoint");
        assert_eq!(
            compact.upstream_url,
            "https://chatgpt.com/backend-api/codex/responses/compact"
        );

        let mock_chatgpt = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/backend-api/codex/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(
                        "event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
                    ),
            )
            .mount(&mock_chatgpt)
            .await;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let auth_override = CodexOwnAuth {
            upstream_url: format!("{}/backend-api/codex/responses", mock_chatgpt.uri()),
            bearer_token: resolved.bearer_token.clone(),
            account_id: resolved.account_id.clone(),
        };
        let mut req_headers = HashMap::new();
        req_headers.insert("content-type".into(), "application/json".into());
        req_headers.insert("x-codex-turn-metadata".into(), "turn-1".into());
        req_headers.insert("x-openai-actor-authorization".into(), "qwitch".into());

        let srv = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let res = forward_codex_own_signin_request(
                &mut s,
                &auth_override,
                &req_headers,
                br#"{"model":"gpt-6-luna","input":[]}"#,
            )
            .await
            .expect("forward");
            assert_eq!(res.0, 200);
        });

        let client = reqwest::Client::new();
        let resp = client.get(format!("http://{addr}/")).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert!(resp.text().await.unwrap().contains("response.completed"));
        srv.await.unwrap();

        let received = mock_chatgpt.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(
            received[0]
                .headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer oauth-jwt-sample"
        );
        assert_eq!(
            received[0]
                .headers
                .get("chatgpt-account-id")
                .unwrap()
                .to_str()
                .unwrap(),
            "acc-uuid-123"
        );
        assert_eq!(
            received[0]
                .headers
                .get("x-codex-turn-metadata")
                .unwrap()
                .to_str()
                .unwrap(),
            "turn-1"
        );
        assert!(received[0]
            .headers
            .get("x-openai-actor-authorization")
            .is_none());

        let _ = std::fs::remove_dir_all(tmp);
    }

    #[tokio::test]
    async fn workbuddy_uses_v2_stream_and_parses_sse_and_json_responses() {
        let sse_sample = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think-\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"world\",\"tool_calls\":[{\"index\":0,\"id\":\"call_wb_1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":22}}\n\n",
            "data: [DONE]\n\n"
        );
        let sse_ir = parse_upstream_response_body(sse_sample, "workbuddy/hy4flash-free");
        assert_eq!(sse_ir.text, "hello world");
        assert_eq!(sse_ir.thinking.as_deref(), Some("think-"));
        assert_eq!(sse_ir.finish_reason, "tool_calls");
        assert_eq!(sse_ir.usage.input_tokens, 11);
        assert_eq!(sse_ir.usage.output_tokens, 22);
        assert_eq!(sse_ir.tool_calls.len(), 1);
        assert_eq!(sse_ir.tool_calls[0].0, "call_wb_1");
        assert_eq!(sse_ir.tool_calls[0].1, "read_file");
        assert_eq!(sse_ir.tool_calls[0].2, "{\"path\":\"a.rs\"}");

        let json_ir = parse_upstream_response_body(
            r#"{"choices":[{"message":{"content":"plain ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":4}}"#,
            "custom/model",
        );
        assert_eq!(json_ir.text, "plain ok");
        assert_eq!(json_ir.usage.input_tokens, 3);
        assert_eq!(json_ir.usage.output_tokens, 4);

        let root = std::env::temp_dir().join(format!("qwitch-wb-{}", Uuid::new_v4()));
        let db = Arc::new(QwitchDatabase::open(&root.join("qwitch.db")).unwrap());
        let key = [7u8; 32];

        let mock_wb = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(concat!(
                        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"step1\"}}]}\n\n",
                        "data: {\"choices\":[{\"delta\":{\"content\":\"wb-pong\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":15,\"completion_tokens\":9}}\n\n",
                        "data: [DONE]\n\n"
                    )),
            )
            .mount(&mock_wb)
            .await;

        let mut extra = HashMap::new();
        extra.insert("base_url".into(), mock_wb.uri());
        let acc = db
            .upsert_account(
                &key,
                Some("WB 账号"),
                &CredentialEnvelope {
                    tool_id: "workbuddy".into(),
                    uid: "wb-uid-1".into(),
                    display_name: Some("WB 账号".into()),
                    display_email: None,
                    payload: CredentialPayload::Token {
                        access_token: "wb-jwt-token".into(),
                        refresh_token: None,
                        expires_at: None,
                        extra,
                    },
                },
            )
            .unwrap();

        let _pool = db
            .create_pool(
                &key,
                &CreatePoolInput {
                    id: None,
                    name: "WB Pool".into(),
                    description: "".into(),
                    scene_tag: "Coding".into(),
                    strategy: PoolStrategy::Order,
                    affinity_mode: AffinityMode::AutoCache,
                    skip_active_client_account: false,
                    cooldown_seconds: 60,
                    auto_refresh_on_401: false,
                    fallback_api_key_enabled: false,
                    members: vec![CreatePoolMemberInput {
                        account_id: acc.id,
                        supported_models: vec!["workbuddy/hy4flash-free".into()],
                        tier: MemberTier::T1Primary,
                    }],
                    enabled_model_ids: None,
                },
            )
            .unwrap();

        let rt = Arc::new(GatewayRuntime::new());
        rt.inner.lock().unwrap().port = 0;
        let port = rt
            .start_server(Arc::clone(&db), key, Arc::new(HashMap::new()))
            .await
            .expect("start gateway");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .bearer_auth("qwitch-bind-dsh")
            .json(&json!({
                "model": "workbuddy/hy4flash-free",
                "messages": [{ "role": "user", "content": "hi" }]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["choices"][0]["message"]["content"], "wb-pong");

        let received = mock_wb.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let req_json: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(req_json["stream"], true);
        assert_eq!(req_json["model"], "hy4flash-free");
        assert_eq!(
            received[0]
                .headers
                .get("user-agent")
                .unwrap()
                .to_str()
                .unwrap(),
            "WorkBuddy/5.9.3 WorkBuddy/5.9.3 CLI/2.165.0"
        );
        assert_eq!(
            received[0]
                .headers
                .get("x-domain")
                .unwrap()
                .to_str()
                .unwrap(),
            "copilot.tencent.com"
        );
        assert_eq!(
            received[0]
                .headers
                .get("x-product")
                .unwrap()
                .to_str()
                .unwrap(),
            "SaaS"
        );
        assert!(received[0].headers.get("x-conversation-id").is_some());

        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn workbuddy_sanitizes_flagged_openers_headers_developer_and_kimi_tool_calls() {
        let mut body = json!({
            "model": "kimi-k3.1-pro-preview",
            "max_completion_tokens": 2048,
            "messages": [
                { "role": "system", "content": "x-anthropic-billing-header: cc=1\n" },
                {
                    "role": "developer",
                    "content": concat!(
                        "You are Claude Code, Anthropic\u{2019}s official CLI for Claude.\n",
                        "Main branch (you will usually use this for PRs)\n",
                        "You are an AI agent powered by DeepSeek Harness.\n",
                        "Codex CLI is an open source project led by OpenAI.\n",
                        "Web GUI whose URL the runtime context names.  When the user refers to UI.\n",
                        "You are an AI agent powered by CustomHarness."
                    )
                },
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "ls", "arguments": "{}" } }]
                },
                {
                    "role": "user",
                    "content": [{ "type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude" }]
                }
            ]
        });

        sanitize_workbuddy_chat_body(&mut body);

        assert_eq!(body["max_tokens"], 2048);
        assert!(body.get("max_completion_tokens").is_none());

        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "system");
        let sys_txt = msgs[0]["content"].as_str().unwrap();
        assert!(!sys_txt.contains("x-anthropic-billing-header"));
        assert!(sys_txt.contains("You are Claude Code, Anthropic's CLI for Claude."));
        assert!(sys_txt.contains("Main branch (usually the base for PRs)"));
        assert!(sys_txt.contains("You are an AI agent, powered by DeepSeek Harness."));
        assert!(sys_txt.contains("The Codex CLI is an open source project led by OpenAI."));
        assert!(sys_txt.contains("Web GUI whose URL the runtime context names. The interface, when the user refers to UI."));
        assert!(sys_txt.contains("You are an AI agent, powered by CustomHarness."));
        // Ensure already-prefixed Codex opener stays idempotent
        assert_eq!(
            unflag_workbuddy_text("The Codex CLI is an open source project led by OpenAI."),
            "The Codex CLI is an open source project led by OpenAI."
        );

        assert_eq!(msgs[1]["content"], " ");
        assert_eq!(
            msgs[2]["content"][0]["text"],
            "You are Claude Code, Anthropic's CLI for Claude"
        );

        let mut no_sys = json!({
            "model": "hy4flash-free",
            "messages": [{ "role": "user", "content": "hi" }]
        });
        sanitize_workbuddy_chat_body(&mut no_sys);
        assert_eq!(no_sys["messages"][0]["role"], "system");
        assert_eq!(no_sys["messages"][0]["content"], WORKBUDDY_DEFAULT_SYSTEM);
    }

    #[test]
    fn workbuddy_move_system_to_user_wraps_instructions_and_is_idempotent() {
        let mut body = json!({
            "model": "hy4flash-free",
            "messages": [
                { "role": "system", "content": "Rule A" },
                { "role": "user", "content": "hello" },
                { "role": "system", "content": "late note" }
            ]
        });

        assert!(move_system_to_user_for_workbuddy(&mut body));
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(
            msgs[0],
            json!({ "role": "system", "content": WORKBUDDY_DEFAULT_SYSTEM })
        );
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(
            msgs[1]["content"],
            "<instructions>\nRule A\n</instructions>\n\nhello"
        );
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"], "late note");

        assert!(!move_system_to_user_for_workbuddy(&mut body));
    }

    #[tokio::test]
    async fn workbuddy_retries_once_with_system_as_user_on_11128() {
        let mock_wb = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                r#"{"code":11128,"msg":"Illegal API invocation from an unapproved channel"}"#,
            ))
            .up_to_n_times(1)
            .mount(&mock_wb)
            .await;
        Mock::given(method("POST"))
            .and(path("/v2/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"recovered-11128\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                    ),
            )
            .mount(&mock_wb)
            .await;

        let mut extra = HashMap::new();
        extra.insert("base_url".into(), mock_wb.uri());
        let cred = CredentialEnvelope {
            tool_id: "workbuddy".into(),
            uid: "wb-uid-retry".into(),
            display_name: Some("WB".into()),
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "wb-tok".into(),
                refresh_token: None,
                expires_at: None,
                extra,
            },
        };
        let member = PoolMember {
            id: "mem-1".into(),
            account_id: "acc-1".into(),
            tool_id: "workbuddy".into(),
            label: "WB".into(),
            uid_masked: "wb-***".into(),
            supported_models: vec!["workbuddy/hy4flash-free".into()],
            tier: MemberTier::T1Primary,
            channel: crate::domain::UpstreamChannel::RustNative,
            enabled: true,
            priority: 0,
            quota_percent: 100,
            quota_text: "100%".into(),
            cooldown_until_ms: None,
            is_active_on_client: None,
            account_status: None,
            code_arts_mode: None,
            last_latency_ms: None,
        };
        let ir_req = IrRequest {
            incoming_protocol: WireProtocolKind::AnthropicMessages,
            model: "workbuddy/hy4flash-free".into(),
            stream: true,
            system_prompt: Some("Custom Agent Prompt".into()),
            messages: vec![crate::gateway::ir::IrMessage {
                role: crate::gateway::ir::IrRole::User,
                parts: vec![crate::gateway::ir::IrPart::Text("ping".into())],
            }],
            tools: Vec::new(),
            tool_choice: None,
            max_tokens: Some(128),
            temperature: None,
        };

        let (code, body_txt) =
            call_upstream_member(&member, &cred, &ir_req, "workbuddy/hy4flash-free")
                .await
                .unwrap();
        assert_eq!(code, 200);
        assert!(body_txt.contains("recovered-11128"));

        let received = mock_wb.received_requests().await.unwrap();
        assert_eq!(received.len(), 2);
        let req1: Value = serde_json::from_slice(&received[0].body).unwrap();
        let req2: Value = serde_json::from_slice(&received[1].body).unwrap();
        assert_eq!(req1["messages"][0]["content"], "Custom Agent Prompt");
        assert_eq!(req2["messages"][0]["content"], WORKBUDDY_DEFAULT_SYSTEM);
        assert_eq!(req2["messages"][1]["role"], "user");
        assert_eq!(
            req2["messages"][1]["content"],
            "<instructions>\nCustom Agent Prompt\n</instructions>\n\nping"
        );
    }
}
