mod doubao;
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

use crate::adapter::common::long_running_http_client;
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
    /// 每号池进程内轮询游标（避免并发请求读到同一 today_requests 快照而全部命中同一账号）
    rr_counters: HashMap<String, u64>,
    /// OpenAI Responses 协议跨协议增量流式开关（默认关闭，关闭时走缓冲+合成，保证 codex 稳定）
    responses_streaming: bool,
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
                port: 8187,
                cooldowns: HashMap::new(),
                session_affinity: HashMap::new(),
                sim_proxy_override: None,
                sim_bridge_override: None,
                shutdown_tx: None,
                rr_counters: HashMap::new(),
                responses_streaming: std::env::var("QWITCH_GATEWAY_RESPONSES_STREAMING")
                    .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
                    .unwrap_or(false),
            }),
            refresh_locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn runtime_state(&self) -> GatewayRuntimeState {
        let (running, host, port, proxy_ovr, bridge_ovr) = {
            let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.cooldowns.retain(|_, until| *until > now);
        g.cooldowns.clone()
    }

    pub fn set_member_cooldown(&self, account_id: &str, cooldown_until_ms: i64) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.cooldowns
            .insert(account_id.to_string(), cooldown_until_ms);
    }

    pub fn clear_member_cooldown(&self, account_id: &str) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.cooldowns.remove(account_id);
    }

    pub fn set_port(&self, port: u16) -> Result<(), String> {
        if port < 1024 {
            return Err("端口必须 >= 1024".into());
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.port = port;
        Ok(())
    }

    pub fn toggle_proxy_simulation(&self) -> bool {
        let current = self.runtime_state().global_proxy_detected;
        let next = !current;
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.sim_bridge_override = Some(next.clone());
        next
    }

    /// 返回该号池下一个轮询序号（进程内单调递增，避免并发同序）。
    pub fn next_round_robin(&self, pool_id: &str) -> u64 {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let c = g.rr_counters.entry(pool_id.to_string()).or_insert(0);
        let v = *c;
        *c = c.wrapping_add(1);
        v
    }

    pub fn set_responses_streaming(&self, enabled: bool) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.responses_streaming = enabled;
    }

    pub fn responses_streaming(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .responses_streaming
    }

    pub fn refresh_lock_for(&self, account_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.refresh_locks.lock().unwrap_or_else(|e| e.into_inner());
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
            let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
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
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
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
                .unwrap_or_else(|e| e.into_inner())
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
                let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                if g.session_affinity.len() >= 4096 {
                    g.session_affinity.clear();
                }
                g.session_affinity
                    .insert(aff_key, winner.account_id.clone());
            }
        }

        let incoming_protocol = match input.agent_id {
            TargetAgentId::CodexDesktop => WireProtocolKind::OpenaiResponses,
            TargetAgentId::ClaudeDesktop => WireProtocolKind::AnthropicMessages,
            TargetAgentId::Dsh | TargetAgentId::Opencode | TargetAgentId::Mimo => {
                WireProtocolKind::OpenaiChat
            }
            TargetAgentId::Zcode => WireProtocolKind::AnthropicMessages,
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
            timestamp: now_beijing_hms(),
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

        log_db_result("append_routing_trace", db.append_routing_trace(&trace));
        log_db_result(
            "increment_pool_stats",
            db.increment_pool_stats(
                &pool.id,
                status == RoutingTraceStatus::FailoverSuccess,
                cache_affinity_hit,
            ),
        );

        Ok(RunPoolSimulationResult {
            trace,
            cooled_member_id,
        })
    }
}

/// 统一按北京时间（UTC+8）输出时分秒，避免部分环境时区不一致或直接截 UTC 串。
fn now_beijing_hms() -> String {
    let bj = chrono::FixedOffset::east_opt(8 * 3600).expect("valid UTC+8 offset");
    chrono::Utc::now()
        .with_timezone(&bj)
        .format("%H:%M:%S")
        .to_string()
}

/// 失败或无候选成员时，从请求模型前缀或最后一次尝试推断归属订阅工具 ID，
/// 避免所有失败记录在「路由观测」里都被错标成 `custom_api_key`。
fn infer_trace_tool_id(requested_model: &str, attempts: &[RoutingAttempt]) -> String {
    if let (Some(tid), _) = crate::domain::split_scoped_model_id(requested_model) {
        return tid.to_string();
    }
    if let Some(last) = attempts.last() {
        return last.tool_id.clone();
    }
    "custom_api_key".to_string()
}

/// 记录 best-effort 持久化失败（路由 trace / 统计），避免 `let _` 完全吞错。
fn log_db_result<T, E: std::fmt::Display>(ctx: &str, r: Result<T, E>) {
    if let Err(e) = r {
        eprintln!("[qwitch][gateway] {ctx} 写入失败: {e}");
    }
}

/// 判定一次上游响应是否为「永久性鉴权失败」——这类失败重试无意义，
/// 应立即标记账号过期并返回可操作提示，而不是当作限流反复空转。
///
/// 覆盖：
/// - HTTP 401（所有工具的通用语义）
/// - 华为云 APIG.0301 `verify ak sk signature fail` / `the access key ... is not found`
/// - 华为云 STS 错误码（STS.0601 / 5_1806 token 过期）
fn is_auth_expired_response(code: u16, body: &str) -> bool {
    if code == 401 {
        return true;
    }
    let lower = body.to_ascii_lowercase();
    lower.contains("apig.0301")
        || lower.contains("apig.0602")
        || lower.contains("security token has expired")
        || lower.contains("verify ak sk signature")
        || lower.contains("access key") && lower.contains("not found")
        || lower.contains("sts.0601")
        || lower.contains("5_1806")
        || lower.contains("incorrect iam authentication")
}

/// 判定一次上游响应是否为「限流 / 并发会话数上限」——可短暂冷却并尝试 Failover 下一账号。
fn is_concurrency_limit_response(code: u16, body: &str) -> bool {
    if code == 429 {
        return true;
    }
    let lower = body.to_ascii_lowercase();
    lower.contains("tm.00001041") || body.contains("并发会话数已达上限")
}

/// 从上游错误体里提炼一段人类可读的凭据失效原因（尽量含 error_code/error_msg）。
fn auth_expired_reason(tool_id: &str, body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let msg = v
            .get("error_msg")
            .or_else(|| v.get("message"))
            .or_else(|| v.get("error_description"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let code = v
            .get("error_code")
            .or_else(|| v.get("code"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if !msg.is_empty() || !code.is_empty() {
            let base = if code.is_empty() {
                msg.to_string()
            } else if msg.is_empty() {
                code.to_string()
            } else {
                format!("{code}: {msg}")
            };
            if tool_id == "codearts" {
                return format!(
                    "{base}（AK/SK 临时凭据已失效/不存在，请在码道客户端重新登录后重新导入）"
                );
            }
            return base;
        }
    }
    let snippet: String = body.chars().take(120).collect();
    snippet
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
            req = req.header(k, v);
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
            } else if env.tool_id == "lobsterai" {
                headers.push(("User-Agent", "LobsterAI/2026.11.20".into()));
                headers.push((
                    "X-LobsterAI-Client-Capabilities",
                    "virtual-models-v1,kimi-k3-agentic-v1,thinking-level-control-v1,upstream-error-details-v1,context-window-selection-v1".into(),
                ));
                headers.push(("X-LobsterAI-Client-Version", "2026.11.20".into()));
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

/// 解析账号的上游 base_url / token / 额外请求头（统一校验 base_url）。
#[allow(clippy::type_complexity)]
const MAX_UPSTREAM_BODY: u64 = 64 * 1024 * 1024;

fn validate_custom_base_url(
    label: &str,
    custom_base: Option<String>,
    default_base: String,
) -> Result<String, String> {
    match custom_base {
        Some(b) => {
            let is_loopback_http = b.starts_with("http://127.0.0.1")
                || b.starts_with("http://localhost")
                || b.starts_with("http://[::1]");
            if !(b.starts_with("https://") || is_loopback_http) {
                return Err(format!(
                    "账号 {} 的 base_url 非法（仅支持 https 或本机 http）",
                    label
                ));
            }
            Ok(b)
        }
        None => Ok(default_base),
    }
}

pub(super) async fn read_bounded_response_text(
    resp: reqwest::Response,
    label: &str,
) -> Result<String, String> {
    if resp
        .content_length()
        .is_some_and(|len| len > MAX_UPSTREAM_BODY)
    {
        return Err(format!(
            "{label} 上游响应体过大（>{} MB）已拒绝",
            MAX_UPSTREAM_BODY / (1024 * 1024)
        ));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取 {label} 响应失败: {}", e.without_url()))?;
    if bytes.len() as u64 > MAX_UPSTREAM_BODY {
        return Err(format!(
            "{label} 上游响应体过大（>{} MB）已拒绝",
            MAX_UPSTREAM_BODY / (1024 * 1024)
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

type UpstreamAuthResult = (String, String, Vec<(&'static str, String)>);

fn resolve_upstream_auth(
    member: &PoolMember,
    cred: &CredentialEnvelope,
) -> Result<UpstreamAuthResult, String> {
    let (token_opt, custom_base, extra_headers) = extract_bearer_and_base_url(cred);
    let token = token_opt.ok_or_else(|| format!("账号 {} 缺少有效 API Token", member.label))?;
    let default_base = match member.tool_id.as_str() {
        "minimax_code_cn" => "https://agent.minimax.cn/mavis/api/v1/llm".to_string(),
        "workbuddy" => "https://copilot.tencent.com".to_string(),
        "qoder_cn" => "https://gateway.qoder.com.cn".to_string(),
        "trae_cn" => "https://trae-api-cn.mchost.guru".to_string(),
        // 有道龙虾：桌面端经本地 token 代理把 /v1 前缀映射到 /api/proxy/v1，这里直接对齐真实上游
        "lobsterai" => "https://lobsterai-server.youdao.com/api/proxy".to_string(),
        _ => "https://api.openai.com".to_string(),
    };
    let base_url = validate_custom_base_url(&member.label, custom_base, default_base)?;
    Ok((base_url, token, extra_headers))
}

/// 以流式（SSE）方式向上游发起「与客户端同协议」的请求，返回未读取 body 的响应，
/// 供调用方在首字节前完成状态判定后直接透传给客户端。
async fn send_upstream_stream(
    member: &PoolMember,
    cred: &CredentialEnvelope,
    ir_req: &IrRequest,
    upstream_model: &str,
) -> Result<reqwest::Response, String> {
    let client = long_running_http_client();
    let (_, bare) = crate::domain::split_scoped_model_id(upstream_model);
    let wire_model = resolve_vendor_wire_model_id(&member.tool_id, bare);
    // 码道不接受 bearer：它用华为云 AK/SK 签名，只能在拿到 body 之后再签，因此单独走一条路
    if member.tool_id == "codearts" {
        return send_codearts_stream(client, member, cred, ir_req, &wire_model).await;
    }
    // Trae CN 走 IDE agent 私有协议 `/api/agent/v3/llm_utils_chat`，原生 SSE 需转成 OpenAI SSE
    if member.tool_id == "trae_cn" {
        return send_trae_stream(client, member, cred, ir_req, &wire_model).await;
    }
    // Qoder CN 走 COSY 签名私有协议 `/algo/api/v2/service/pro/sse/agent_chat_generation`
    if member.tool_id == "qoder_cn" {
        return send_qoder_stream(client, member, cred, ir_req, &wire_model).await;
    }
    // 豆包工作走网页/桌面端 Cookie 会话协议 `POST https://www.doubao.com/chat/completion`
    if member.tool_id == "doubaowork" {
        return doubao::send_doubao_stream(client, member, cred, ir_req, &wire_model).await;
    }
    let (base_url, token, extra_headers) = resolve_upstream_auth(member, cred)?;
    let base = base_url.trim_end_matches('/');
    let protocol = upstream_model_protocol(&member.tool_id, &wire_model);
    let (url, body, auth_x_api_key) = match protocol {
        WireProtocolKind::AnthropicMessages => (
            format!("{base}/v1/messages"),
            to_upstream_anthropic_body(ir_req, &wire_model, true),
            true,
        ),
        _ => (
            format!("{base}/v1/chat/completions"),
            to_upstream_openai_chat_body(ir_req, &wire_model, true),
            false,
        ),
    };
    let mut req = client
        .post(&url)
        .json(&body)
        .header("Accept", "text/event-stream");
    if member.tool_id == "minimax_code_cn" {
        req = req
            .bearer_auth(&token)
            .header("x-api-key", "minimax-agent")
            .header("anthropic-version", "2023-06-01")
            .header("X-Mavis-Session-Id", Uuid::new_v4().to_string());
    } else if auth_x_api_key {
        req = req
            .header("x-api-key", &token)
            .header("anthropic-version", "2023-06-01");
    } else {
        req = req.bearer_auth(&token);
    }
    for (k, v) in &extra_headers {
        req = req.header(*k, v);
    }
    req.send().await.map_err(|e| format!("上游请求失败: {e}"))
}

/// 码道 CodeArts 的对话上游。
///
/// 协议依据 magpie 的 `opencode-codearts-auth`：`POST {snap-access}/api/v2/chat/completions`，
/// OpenAI 兼容，**上游始终流式**（码道只对流式计费），鉴权用 SDK-HMAC-SHA256
/// 对「含请求体哈希」的规范请求签名。会话用 `x-ot-session-id` 区分，每账号 3 个并发。
async fn send_codearts_stream(
    client: &reqwest::Client,
    member: &PoolMember,
    cred: &CredentialEnvelope,
    ir_req: &IrRequest,
    wire_model: &str,
) -> Result<reqwest::Response, String> {
    use crate::adapter::codearts::CodeArtsAdapter;

    let mut body = to_upstream_openai_chat_body(ir_req, wire_model, true);
    // 码道把思考单独放在 reasoning_content，不开 thinking 就拿不到
    if body.get("thinking").is_none() {
        body["thinking"] = serde_json::json!({ "type": "enabled" });
    }
    // 输出预算上限 65536（上游硬限制，超了会被拒）
    if let Some(max) = body.get("max_tokens").and_then(Value::as_u64) {
        if max > 65536 {
            body["max_tokens"] = serde_json::json!(65536u64);
        }
    }
    let body_bytes = serde_json::to_vec(&body).map_err(|e| format!("序列化码道请求体失败: {e}"))?;
    let bases = CodeArtsAdapter::snap_base_urls(cred);
    let mut last_err = String::new();

    for base in bases {
        let url = format!("{base}/api/v2/chat/completions");
        let signed = CodeArtsAdapter::signed_request_headers(cred, "POST", &url, &body_bytes)?;

        let mut req = client
            .post(&url)
            .header("Accept", "text/event-stream")
            // 每账号一个稳定会话 id，避免同一账号被码道当成 3 个不同会话
            .header("x-ot-session-id", member.account_id.clone())
            .body(body_bytes.clone());
        for (k, v) in signed {
            req = req.header(k, v);
        }
        match req.send().await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                last_err = format!("码道上游请求失败: {}", e.without_url());
            }
        }
    }

    Err(last_err)
}

/// Trae CN 对话上游：IDE agent 私有协议 `POST {host}/api/agent/v3/llm_utils_chat`。
///
/// 协议依据 magpie 的 `opencode-trae-auth`：
/// - 鉴权头 `Authorization: Cloud-IDE-JWT {token}`，另需 `x-app-id`、`x-device-id`、
///   `x-uid`、`x-app-version(-code)`、`x-ide-version(-code)(-type)`、`x-device-*`、
///   `x-system-type` 等一整套 IDE 头（缺 `x-app-version-code` 会报
///   `binding: expr_path=app_version_code`）；
/// - 请求体 `{messages:[{role,content:string}], function:"chat_v3", config_name, model,
///   stream:true, request_id, session_id}`；
/// - 响应是 Trae 原生 SSE（`event:output/delta/token_usage/done/error`），且**即便出错也返回
///   HTTP 200 + `event:error`**，因此这里读全量后：遇错误事件即合成一个非 2xx 响应交给上层
///   的 429/鉴权/HttpError 分支处理；正常则把原生 SSE 转成 OpenAI `chat.completion.chunk`
///   SSE 再合成 200 响应交给下游（下游按 OpenAI passthrough 消费）。
async fn send_trae_stream(
    client: &reqwest::Client,
    member: &PoolMember,
    cred: &CredentialEnvelope,
    ir_req: &IrRequest,
    wire_model: &str,
) -> Result<reqwest::Response, String> {
    use crate::adapter::trae::TraeAdapter;

    let (token_opt, custom_base, _extra) = extract_bearer_and_base_url(cred);
    let token = token_opt.ok_or_else(|| format!("账号 {} 缺少有效 API Token", member.label))?;
    let host = validate_custom_base_url(
        &member.label,
        custom_base,
        "https://trae-api-cn.mchost.guru".to_string(),
    )?;
    let host = host.trim_end_matches('/').to_string();
    let identity = TraeAdapter::chat_request_identity(cred, &token);

    // 对话消息：Trae 要求 [{role, content: [{text:string}, ...]}]——content 必须是
    // 「内容块数组」（LLMRawMessageContent），string 会触发 Go 反序列化错误，而
    // 用 text 字段则被服务端判为 4001 参数非法。
    let openai_body = to_upstream_openai_chat_body(ir_req, wire_model, true);
    let mut messages: Vec<Value> = Vec::new();
    if let Some(arr) = openai_body.get("messages").and_then(Value::as_array) {
        for m in arr {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
            let text = match m.get("content") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(""),
                _ => String::new(),
            };
            messages.push(json!({ "role": role, "content": [{ "text": text }] }));
        }
    }
    if messages.is_empty() {
        return Err("Trae 请求体为空（无有效对话消息）".to_string());
    }

    let mut body = json!({
        "messages": messages,
        "function": "chat_v3",
        "config_name": wire_model,
        "model": wire_model,
        "stream": true,
        "request_id": Uuid::new_v4().to_string(),
        "session_id": Uuid::new_v4().to_string(),
    });
    if let Some(max) = ir_req.max_tokens {
        if max > 0 {
            body["max_tokens"] = json!(max);
        }
    }
    if let Some(t) = ir_req.temperature {
        body["temperature"] = json!(t);
    }

    let app_version = if identity.app_version.trim().is_empty() {
        "0.1.69".to_string()
    } else {
        identity.app_version.clone()
    };
    let device_id = identity.device_id.clone();
    let uid = identity.uid.clone();

    let url = format!("{host}/api/agent/v3/llm_utils_chat");
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .header("Authorization", format!("Cloud-IDE-JWT {token}"))
        .header("X-Cloudide-Token", &token)
        .header("x-ide-token", &token)
        .header("x-app-id", "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8")
        .header("x-app-version", &app_version)
        .header("x-app-version-code", "20260917")
        .header("x-ide-version", &app_version)
        .header("x-ide-version-code", "20260917")
        .header("x-ide-version-type", "stable")
        .header("x-device-type", "windows")
        .header("x-os-version", "Windows 10")
        .header("x-system-type", "Windows")
        .header("x-device-cpu", "AMD")
        .header("x-request-id", Uuid::new_v4().to_string())
        .json(&body);
    if !device_id.trim().is_empty() {
        req = req
            .header("x-device-id", &device_id)
            .header("x-machine-id", &device_id);
    }
    if !uid.trim().is_empty() {
        req = req.header("x-uid", &uid);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Trae 上游请求失败: {}", e.without_url()))?;
    let status = resp.status();
    let raw = read_bounded_response_text(resp, "Trae").await?;

    // 若 HTTP 非 2xx 或首个有效事件是 event:error，则合成一个非 2xx 响应喂给上层的失败分支
    if let Some((code, msg)) = trae_first_error(status.as_u16(), &raw) {
        let http_status = match code.as_deref() {
            Some("429") | Some("4290") => reqwest::StatusCode::TOO_MANY_REQUESTS,
            Some("401") | Some("403") => reqwest::StatusCode::UNAUTHORIZED,
            _ => {
                if status.is_success() {
                    reqwest::StatusCode::BAD_REQUEST
                } else {
                    status
                }
            }
        };
        let body_bytes = serde_json::to_vec(&json!({
            "error": { "message": msg, "code": code, "type": "trae_error" }
        }))
        .unwrap_or_default();
        return Ok(synthetic_response(
            http_status,
            "application/json",
            body_bytes,
        ));
    }

    // 正常：把 Trae 原生 SSE 转成 OpenAI chat.completion.chunk SSE 字节
    let openai_sse = trae_sse_to_openai(&raw, &ir_req.model, wire_model);
    Ok(synthetic_response(
        reqwest::StatusCode::OK,
        "text/event-stream; charset=utf-8",
        openai_sse.into_bytes(),
    ))
}

/// Qoder CN 对话上游：COSY 签名私有协议 `/algo/api/v2/service/pro/sse/agent_chat_generation`。
///
/// - 请求体是 `encodeBody(JSON)`（打乱 base64 + 首尾三分之一互换）；
/// - 鉴权 `Authorization: Bearer COSY.{payload}.{sig}`，`Cosy-Key` 包裹 AES 密钥；
/// - 响应 SSE 每帧 `data: {statusCodeValue, body:"<OpenAI chunk JSON 字符串>"}`。
///
/// 这里读全量后：`statusCodeValue != 200` 或帧内 error 即合成非 2xx 交给上层失败分支；
/// 正常则把内层 chunk 还原成 OpenAI SSE 再合成 200 响应。
async fn send_qoder_stream(
    client: &reqwest::Client,
    member: &PoolMember,
    cred: &CredentialEnvelope,
    ir_req: &IrRequest,
    wire_model: &str,
) -> Result<reqwest::Response, String> {
    use crate::adapter::qoder_cosy;

    let (token_opt, custom_base, _extra) = extract_bearer_and_base_url(cred);
    let token = token_opt.ok_or_else(|| format!("账号 {} 缺少有效 API Token", member.label))?;
    let api = validate_custom_base_url(
        &member.label,
        custom_base,
        "https://gateway.qoder.com.cn".to_string(),
    )?;
    let api = api.trim_end_matches('/').to_string();

    // machine_id / 身份信息从凭据里取
    let (machine_id, disp_name, disp_email) = extract_qoder_identity(cred);

    let model_key = wire_model;
    let model_source = "system";
    let display_name = model_key;

    let plaintext =
        qoder_cosy::build_qoder_body(ir_req, model_key, model_source, display_name, 200_000);
    let wire_body = qoder_cosy::encode_body(&plaintext.to_string());

    let url = format!("{api}{}", qoder_cosy::CHAT_PATH);
    let headers = qoder_cosy::cosy_headers(
        &url,
        &cred.uid,
        disp_name.as_deref().unwrap_or(""),
        disp_email.as_deref().unwrap_or(""),
        &token,
        &machine_id,
        &wire_body,
        true,
        Some(model_key),
        Some(model_source),
    )?;

    let mut req = client.post(&url).body(wire_body);
    for (k, v) in headers {
        req = req.header(k, v);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Qoder 上游请求失败: {}", e.without_url()))?;
    let status = resp.status();
    let raw = read_bounded_response_text(resp, "Qoder").await?;

    // 先判定是否整体失败（HTTP 非 2xx，或 SSE 帧里 statusCodeValue != 200 / 帧内 error）
    if let Some((code, msg)) = qoder_first_error(status.as_u16(), &raw) {
        let http_status = match code.as_deref() {
            Some("429") => reqwest::StatusCode::TOO_MANY_REQUESTS,
            Some("401") | Some("403") => reqwest::StatusCode::UNAUTHORIZED,
            // Qoder 登录过期 / 需要重新登录
            Some("105") => reqwest::StatusCode::UNAUTHORIZED,
            _ => {
                if status.is_success() {
                    reqwest::StatusCode::BAD_REQUEST
                } else {
                    status
                }
            }
        };
        let body_bytes = serde_json::to_vec(&json!({
            "error": { "message": msg, "code": code, "type": "qoder_error" }
        }))
        .unwrap_or_default();
        return Ok(synthetic_response(
            http_status,
            "application/json",
            body_bytes,
        ));
    }

    let openai_sse = qoder_sse_to_openai(&raw, &ir_req.model);
    Ok(synthetic_response(
        reqwest::StatusCode::OK,
        "text/event-stream; charset=utf-8",
        openai_sse.into_bytes(),
    ))
}

/// 从 Qoder 凭据里取 (machine_id, display_name, display_email)。
fn extract_qoder_identity(cred: &CredentialEnvelope) -> (String, Option<String>, Option<String>) {
    let machine_id = match &cred.payload {
        CredentialPayload::Token { extra, .. } => extra
            .get("machine_id")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default(),
        CredentialPayload::LocalSnapshot { files } => files
            .get("auth.machine-id")
            .and_then(|b64| {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()
            })
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default(),
        _ => String::new(),
    };
    (
        machine_id,
        cred.display_name.clone(),
        cred.display_email.clone(),
    )
}

/// 从 Qoder SSE 文本里提取首个错误。`status` 为 HTTP 状态码。
fn qoder_first_error(status: u16, raw: &str) -> Option<(Option<String>, String)> {
    if !(200..300).contains(&status) {
        let msg = raw.trim();
        let msg = if msg.is_empty() {
            format!("HTTP {status}")
        } else {
            msg.chars().take(512).collect()
        };
        return Some((Some(status.to_string()), msg));
    }
    for line in raw.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(env) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(sc) = env.get("statusCodeValue").and_then(Value::as_u64) {
            if sc != 200 {
                let inner = env.get("body").and_then(Value::as_str).unwrap_or("");
                let msg = if inner.trim().is_empty() {
                    data.to_string()
                } else {
                    inner.chars().take(512).collect()
                };
                return Some((Some(sc.to_string()), msg));
            }
        }
        // 帧内 body 里带 error 且无 choices
        let inner = env.get("body").and_then(Value::as_str).unwrap_or("");
        if let Ok(chunk) = serde_json::from_str::<Value>(inner.trim()) {
            let has_choices = chunk.get("choices").map(|c| c.is_array()).unwrap_or(false);
            if !has_choices
                && (chunk.get("error").is_some()
                    || (chunk.get("code").is_some() && chunk.get("message").is_some()))
            {
                let msg = chunk
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .or_else(|| chunk.get("message").and_then(Value::as_str))
                    .unwrap_or("Qoder 返回错误")
                    .to_string();
                let code = chunk
                    .get("code")
                    .or_else(|| chunk.pointer("/error/code"))
                    .map(|c| {
                        c.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| c.to_string())
                    });
                return Some((code, msg));
            }
        }
    }
    // 非 SSE 纯 JSON 错误响应（如网关返回 HTTP 200 但 body 为 {"code":105,"message":"..."} 或 {"error":{...}}）
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            let has_choices = v.get("choices").is_some_and(Value::is_array);
            if !has_choices
                && (v.get("error").is_some()
                    || (v.get("code").is_some() && v.get("message").is_some()))
            {
                let msg = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .or_else(|| v.get("message").and_then(Value::as_str))
                    .unwrap_or("Qoder 返回错误")
                    .to_string();
                let code = v.get("code").or_else(|| v.pointer("/error/code")).map(|c| {
                    c.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| c.to_string())
                });
                return Some((code, msg));
            }
        }
    }
    None
}

/// 把 Qoder SSE（外层信封包内层 OpenAI chunk）还原成标准 OpenAI SSE 文本。
fn qoder_sse_to_openai(raw: &str, model: &str) -> String {
    let id = format!(
        "chatcmpl-qoder-{}",
        &Uuid::new_v4().simple().to_string()[..12]
    );
    let created = chrono::Utc::now().timestamp();
    let mut out = String::new();
    let mut wrote_role = false;
    let mut last_usage: Option<Value> = None;
    let mut finish_reason: Option<Value> = None;

    for line in raw.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        let Ok(env) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let inner = env.get("body").and_then(Value::as_str).unwrap_or("");
        if inner.trim() == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(inner.trim()) else {
            continue;
        };
        // usage 既可能挂在有 choices 的帧上，也可能在只带 usage 的 choices:[] 帧上
        if let Some(u) = chunk.get("usage").filter(|u| !u.is_null()) {
            last_usage = Some(u.clone());
        }
        let delta = chunk
            .pointer("/choices/0/delta")
            .cloned()
            .unwrap_or(Value::Null);
        if let Some(f) = chunk
            .pointer("/choices/0/finish_reason")
            .filter(|f| f.is_string())
        {
            finish_reason = Some(f.clone());
        }

        if !wrote_role {
            out.push_str(&format!(
                "data: {}\n\n",
                json!({"id": id, "object":"chat.completion.chunk","created":created,"model":model,
                       "choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]})
            ));
            wrote_role = true;
        }

        let mut out_delta = serde_json::Map::new();
        if let Some(c) = delta.get("content").and_then(Value::as_str) {
            if !c.is_empty() {
                out_delta.insert("content".into(), json!(c));
            }
        }
        if let Some(r) = delta.get("reasoning_content").and_then(Value::as_str) {
            if !r.is_empty() {
                out_delta.insert("reasoning_content".into(), json!(r));
            }
        }
        if let Some(tc) = delta
            .get("tool_calls")
            .filter(|v| v.as_array().is_some_and(|arr| !arr.is_empty()))
        {
            out_delta.insert("tool_calls".into(), tc.clone());
        }
        if !out_delta.is_empty() {
            out.push_str(&format!(
                "data: {}\n\n",
                json!({"id": id, "object":"chat.completion.chunk","created":created,"model":model,
                       "choices":[{"index":0,"delta":Value::Object(out_delta),"finish_reason":Value::Null}]})
            ));
        }
    }
    if !wrote_role {
        out.push_str(&format!(
            "data: {}\n\n",
            json!({"id": id, "object":"chat.completion.chunk","created":created,"model":model,
                   "choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]})
        ));
    }
    // 结束帧：带上 finish_reason 与累积到的 usage（Qoder 的 usage 常在 finish 之后的
    // 独立 choices:[] 帧里，所以统一在循环后一次性发出）
    let finish = finish_reason.unwrap_or_else(|| json!("stop"));
    let mut end = json!({"id": id, "object":"chat.completion.chunk","created":created,"model":model,
           "choices":[{"index":0,"delta":{},"finish_reason":finish}]});
    if let Some(u) = &last_usage {
        end["usage"] = u.clone();
    }
    out.push_str(&format!("data: {}\n\n", end));
    out.push_str("data: [DONE]\n\n");
    out
}

/// 用 `http::Response<reqwest::Body>` 合成一个 `reqwest::Response`，供下游统一消费。
fn synthetic_response(
    status: reqwest::StatusCode,
    content_type: &str,
    body: Vec<u8>,
) -> reqwest::Response {
    let mut builder = http::Response::builder().status(status);
    if !content_type.is_empty() {
        builder = builder.header("content-type", content_type);
    }
    let resp = builder
        .body(reqwest::Body::from(body))
        .expect("构造合成响应失败");
    reqwest::Response::from(resp)
}

/// 从 Trae 原生 SSE 文本（或非 2xx / 纯 JSON 错误响应）里提取首个错误的 (code, message)。
fn trae_first_error(status: u16, raw: &str) -> Option<(Option<String>, String)> {
    let normalized = raw.replace("\r\n", "\n");
    if !(200..300).contains(&status) {
        if let Ok(v) = serde_json::from_str::<Value>(normalized.trim()) {
            let code = v
                .get("code")
                .or_else(|| v.pointer("/error/code"))
                .map(|c| {
                    c.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| c.to_string())
                })
                .or_else(|| Some(status.to_string()));
            let msg = v
                .pointer("/error/message")
                .and_then(Value::as_str)
                .or_else(|| v.get("message").and_then(Value::as_str))
                .unwrap_or_else(|| normalized.trim())
                .chars()
                .take(512)
                .collect();
            return Some((code, msg));
        }
        let msg = normalized.trim();
        let msg = if msg.is_empty() {
            format!("HTTP {status}")
        } else {
            msg.chars().take(512).collect()
        };
        return Some((Some(status.to_string()), msg));
    }
    for block in normalized.split("\n\n") {
        let mut event = "";
        let mut data_line = "";
        for line in block.lines() {
            if let Some(ev) = line.strip_prefix("event:") {
                event = ev.trim();
            } else if let Some(d) = line.strip_prefix("data:") {
                data_line = d.trim();
            }
        }
        if data_line.is_empty() || data_line == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data_line) else {
            continue;
        };
        let is_error = event == "error"
            || (v.get("code").is_some()
                && v.get("message").is_some()
                && v.get("content").is_none());
        if is_error {
            let code = v.get("code").map(|c| {
                c.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| c.to_string())
            });
            let msg = v
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Trae 返回错误")
                .to_string();
            return Some((code, msg));
        }
    }
    // 非 SSE 纯 JSON 错误响应
    let trimmed = normalized.trim();
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            if v.get("error").is_some()
                || (v.get("code").is_some()
                    && v.get("message").is_some()
                    && v.get("content").is_none())
            {
                let code = v.get("code").or_else(|| v.pointer("/error/code")).map(|c| {
                    c.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| c.to_string())
                });
                let msg = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .or_else(|| v.get("message").and_then(Value::as_str))
                    .unwrap_or("Trae 返回错误")
                    .to_string();
                return Some((code, msg));
            }
        }
    }
    None
}

/// 把 Trae 原生 SSE 转成 OpenAI `chat.completion.chunk` SSE 文本。
/// 逐事件读取：文本字段合并为增量 delta，token_usage→usage 收尾，
/// done/finish_reason→结束并发送 [DONE]。
fn trae_sse_to_openai(raw: &str, model: &str, _wire_model: &str) -> String {
    let normalized = raw.replace("\r\n", "\n");
    let id = format!(
        "chatcmpl-trae-{}",
        &Uuid::new_v4().simple().to_string()[..12]
    );
    let created = chrono::Utc::now().timestamp();
    let mut out = String::new();
    let mut finish_reason = "stop".to_string();
    let mut usage_obj: Option<Value> = None;
    let mut wrote_any = false;

    let push_chunk =
        |out: &mut String, delta: Value, finish: Option<&str>, usage: &Option<Value>| {
            let mut payload = json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
            });
            if let Some(u) = usage {
                payload["usage"] = u.clone();
            }
            out.push_str("data: ");
            out.push_str(&payload.to_string());
            out.push_str("\n\n");
        };

    for block in normalized.split("\n\n") {
        let mut event = String::new();
        let mut data_line = String::new();
        for line in block.lines() {
            if let Some(ev) = line.strip_prefix("event:") {
                event = ev.trim().to_string();
            } else if let Some(d) = line.strip_prefix("data:") {
                data_line = d.trim().to_string();
            }
        }
        if data_line.is_empty() || data_line == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&data_line) else {
            continue;
        };
        // 只有 output 事件承载模型增量；metadata/timing_cost/extra_info/token_usage/done
        // 等控制事件不能当正文读（extra_info.content 是全量答案 JSON，会污染输出）。
        let is_output = event == "output" || event.is_empty();
        if !is_output {
            // 仍处理 usage / finish 控制事件
            if event == "token_usage" {
                if let Some(u) = tokens_of_trae(&v) {
                    usage_obj = Some(u);
                }
            }
            if event == "done" || event == "response_done" || event == "stream_done" {
                if let Some(u) = v.get("usage").and_then(tokens_of_trae) {
                    usage_obj = Some(u);
                }
            }
            if let Some(fr) = v.get("finish_reason").and_then(Value::as_str) {
                finish_reason = match fr.to_lowercase().as_str() {
                    "length" | "max_tokens" | "max_output_tokens" | "max_length" => {
                        "length".to_string()
                    }
                    "content_filter" | "sensitive" => "content_filter".to_string(),
                    _ => "stop".to_string(),
                };
            }
            continue;
        }
        // 统一取文本增量：response / content / delta.content / delta(字符串)
        let delta_obj = v.get("delta").cloned().unwrap_or(Value::Null);
        let reasoning = [
            v.get("reasoning_content"),
            v.get("reasoning"),
            delta_obj.get("reasoning_content"),
            delta_obj.get("reasoning"),
        ]
        .into_iter()
        .flatten()
        .find_map(|x| x.as_str().filter(|s| !s.is_empty()).map(str::to_string));
        let text = v
            .get("response")
            .and_then(Value::as_str)
            .or_else(|| v.get("content").and_then(Value::as_str))
            .or_else(|| delta_obj.get("content").and_then(Value::as_str))
            .or_else(|| v.get("delta").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        if !wrote_any {
            push_chunk(
                &mut out,
                json!({ "role": "assistant", "content": "" }),
                None,
                &usage_obj,
            );
            wrote_any = true;
        }
        if let Some(r) = reasoning {
            push_chunk(
                &mut out,
                json!({ "reasoning_content": r }),
                None,
                &usage_obj,
            );
        }
        if !text.is_empty()
            && !text.starts_with("Building prompt:")
            && !text.starts_with("Completed building prompt")
        {
            push_chunk(&mut out, json!({ "content": text }), None, &usage_obj);
        }
        if let Some(u) = v.get("usage") {
            let prompt = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
            let completion = u
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if prompt != 0 || completion != 0 {
                usage_obj = Some(json!({
                    "prompt_tokens": prompt,
                    "completion_tokens": completion,
                    "total_tokens": u.get("total_tokens").and_then(Value::as_u64).unwrap_or(prompt + completion),
                }));
            }
        }
        if let Some(fr) = v.get("finish_reason").and_then(Value::as_str) {
            finish_reason = match fr.to_lowercase().as_str() {
                "length" | "max_tokens" | "max_output_tokens" | "max_length" => {
                    "length".to_string()
                }
                "content_filter" | "sensitive" => "content_filter".to_string(),
                _ => "stop".to_string(),
            };
        }
    }
    if !wrote_any {
        push_chunk(
            &mut out,
            json!({ "role": "assistant", "content": "" }),
            None,
            &usage_obj,
        );
    }
    // 结束帧
    let mut end = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": finish_reason }],
    });
    if let Some(u) = &usage_obj {
        end["usage"] = u.clone();
    }
    out.push_str("data: ");
    out.push_str(&end.to_string());
    out.push_str("\n\n");
    out.push_str("data: [DONE]\n\n");
    out
}

fn tokens_of_trae(u: &Value) -> Option<Value> {
    let prompt = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let completion = u
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if prompt == 0 && completion == 0 {
        return None;
    }
    Some(json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": u.get("total_tokens").and_then(Value::as_u64).unwrap_or(prompt + completion),
    }))
}

async fn write_sse_head(stream: &mut TcpStream) -> std::io::Result<()> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-cache\r\nConnection: close\r\nX-Accel-Buffering: no\r\n\r\n";
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await
}

async fn write_sse_frame(
    stream: &mut TcpStream,
    event: Option<&str>,
    data: &str,
) -> std::io::Result<()> {
    let mut out = String::new();
    if let Some(ev) = event {
        out.push_str("event: ");
        out.push_str(ev);
        out.push('\n');
    }
    out.push_str("data: ");
    out.push_str(data);
    out.push_str("\n\n");
    stream.write_all(out.as_bytes()).await?;
    stream.flush().await
}

fn map_finish_openai_to_anthropic(fr: &str) -> &'static str {
    match fr {
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        _ => "end_turn",
    }
}

fn map_stop_anthropic_to_openai(stop: &str) -> &'static str {
    match stop {
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        _ => "stop",
    }
}

/// 跨 TCP 分块保持 UTF-8 多字节字符与 `\r\n` 换行符完整性的流式解码器。
#[derive(Default)]
struct Utf8SseDecoder {
    pending: Vec<u8>,
}

impl Utf8SseDecoder {
    fn push_chunk(&mut self, bytes: &[u8], out: &mut String) {
        self.pending.extend_from_slice(bytes);
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    if let Some(prefix) = valid.strip_suffix('\r') {
                        if !prefix.is_empty() {
                            out.push_str(&prefix.replace("\r\n", "\n"));
                        }
                        self.pending.clear();
                        self.pending.push(b'\r');
                    } else {
                        if !valid.is_empty() {
                            out.push_str(&valid.replace("\r\n", "\n"));
                        }
                        self.pending.clear();
                    }
                    break;
                }
                Err(err) => {
                    let valid_up_to = err.valid_up_to();
                    if valid_up_to > 0 {
                        if let Ok(prefix) = std::str::from_utf8(&self.pending[..valid_up_to]) {
                            out.push_str(&prefix.replace("\r\n", "\n"));
                        }
                    }
                    if let Some(bad_len) = err.error_len() {
                        out.push('\u{FFFD}');
                        self.pending.drain(..valid_up_to + bad_len);
                    } else {
                        self.pending.drain(..valid_up_to);
                        break;
                    }
                }
            }
        }
    }

    fn finish(&mut self, out: &mut String) {
        if !self.pending.is_empty() {
            out.push_str(&String::from_utf8_lossy(&self.pending).replace("\r\n", "\n"));
            self.pending.clear();
        }
    }
}

#[derive(Default)]
struct OpenAiToAnthropicState {
    next_block: usize,
    text_idx: Option<usize>,
    stop_reason: String,
    out_tokens: u64,
    tools: Vec<(i64, String, String, String)>,
}

async fn drain_openai_events_to_anthropic(
    buf: &mut String,
    stream: &mut TcpStream,
    st: &mut OpenAiToAnthropicState,
) -> std::io::Result<()> {
    while let Some(idx) = buf.find("\n\n") {
        let block = buf[..idx].to_string();
        *buf = buf[idx + 2..].to_string();
        for line in block.lines() {
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(t) = v
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
            {
                if !t.is_empty() {
                    if st.text_idx.is_none() {
                        let bi = st.next_block;
                        st.next_block += 1;
                        st.text_idx = Some(bi);
                        write_sse_frame(
                            stream,
                            Some("content_block_start"),
                            &json!({"type":"content_block_start","index":bi,"content_block":{"type":"text","text":""}}).to_string(),
                        )
                        .await?;
                    }
                    let bi = st.text_idx.unwrap_or(0);
                    write_sse_frame(
                        stream,
                        Some("content_block_delta"),
                        &json!({"type":"content_block_delta","index":bi,"delta":{"type":"text_delta","text":t}}).to_string(),
                    )
                    .await?;
                }
            }
            if let Some(arr) = v
                .pointer("/choices/0/delta/tool_calls")
                .and_then(Value::as_array)
            {
                for tc in arr {
                    let oi = tc.get("index").and_then(Value::as_i64).unwrap_or(0);
                    let id = tc
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = tc
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let args = tc
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if !st.tools.iter().any(|e| e.0 == oi) {
                        st.tools
                            .push((oi, String::new(), String::new(), String::new()));
                    }
                    if let Some(slot) = st.tools.iter_mut().find(|e| e.0 == oi) {
                        if !id.is_empty() {
                            slot.1 = id;
                        }
                        if !name.is_empty() {
                            slot.2 = name;
                        }
                        slot.3.push_str(&args);
                    }
                }
            }
            if let Some(fr) = v
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
            {
                st.stop_reason = map_finish_openai_to_anthropic(fr).to_string();
            }
            if let Some(o) = v
                .pointer("/usage/completion_tokens")
                .and_then(Value::as_u64)
            {
                st.out_tokens = o;
            }
        }
    }
    Ok(())
}

/// OpenAI Chat 上游 → Anthropic 客户端（文本实时增量，工具块末尾整块给出以保证顺序语义）。
async fn relay_openai_chat_to_anthropic(
    mut resp: reqwest::Response,
    model: &str,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    write_sse_head(stream).await?;
    let msg_id = format!("msg_{}", &Uuid::new_v4().simple().to_string()[..12]);
    write_sse_frame(
        stream,
        Some("message_start"),
        &json!({"type":"message_start","message":{"id":msg_id,"type":"message","role":"assistant","model":model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}).to_string(),
    )
    .await?;
    let mut buf = String::new();
    let mut decoder = Utf8SseDecoder::default();
    let mut st = OpenAiToAnthropicState {
        stop_reason: "end_turn".into(),
        ..Default::default()
    };
    while let Ok(Some(bytes)) = resp.chunk().await {
        decoder.push_chunk(&bytes, &mut buf);
        drain_openai_events_to_anthropic(&mut buf, stream, &mut st).await?;
    }
    decoder.finish(&mut buf);
    drain_openai_events_to_anthropic(&mut buf, stream, &mut st).await?;
    if let Some(ti) = st.text_idx {
        write_sse_frame(
            stream,
            Some("content_block_stop"),
            &json!({"type":"content_block_stop","index":ti}).to_string(),
        )
        .await?;
    }
    let tools = st.tools.clone();
    for (_oi, id, name, args) in &tools {
        let bi = st.next_block;
        st.next_block += 1;
        write_sse_frame(
            stream,
            Some("content_block_start"),
            &json!({"type":"content_block_start","index":bi,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}).to_string(),
        )
        .await?;
        if !args.is_empty() {
            write_sse_frame(
                stream,
                Some("content_block_delta"),
                &json!({"type":"content_block_delta","index":bi,"delta":{"type":"input_json_delta","partial_json":args}}).to_string(),
            )
            .await?;
        }
        write_sse_frame(
            stream,
            Some("content_block_stop"),
            &json!({"type":"content_block_stop","index":bi}).to_string(),
        )
        .await?;
    }
    let stop = if tools.is_empty() {
        st.stop_reason.clone()
    } else {
        "tool_use".to_string()
    };
    write_sse_frame(
        stream,
        Some("message_delta"),
        &json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":st.out_tokens}}).to_string(),
    )
    .await?;
    write_sse_frame(
        stream,
        Some("message_stop"),
        &json!({"type":"message_stop"}).to_string(),
    )
    .await
}

#[derive(Default)]
struct AnthropicToOpenAiState {
    stop_reason: String,
    out_tokens: u64,
    in_tokens: u64,
    tools: Vec<(i64, String, String, String)>,
}

async fn drain_anthropic_events_to_openai(
    buf: &mut String,
    stream: &mut TcpStream,
    id: &str,
    model: &str,
    st: &mut AnthropicToOpenAiState,
) -> std::io::Result<()> {
    while let Some(idx) = buf.find("\n\n") {
        let block = buf[..idx].to_string();
        *buf = buf[idx + 2..].to_string();
        for line in block.lines() {
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            match v.get("type").and_then(Value::as_str).unwrap_or("") {
                "content_block_start" => {
                    if v.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use")
                    {
                        let bi = v.get("index").and_then(Value::as_i64).unwrap_or(0);
                        let idv = v
                            .pointer("/content_block/id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let name = v
                            .pointer("/content_block/name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if !st.tools.iter().any(|e| e.0 == bi) {
                            st.tools.push((bi, idv, name, String::new()));
                        }
                    }
                }
                "content_block_delta" => {
                    if let Some(t) = v.pointer("/delta/text").and_then(Value::as_str) {
                        if !t.is_empty() {
                            let chunk = json!({"id":id,"object":"chat.completion.chunk","created":chrono::Utc::now().timestamp(),"model":model,"choices":[{"index":0,"delta":{"content":t},"finish_reason":null}]});
                            write_sse_frame(stream, None, &chunk.to_string()).await?;
                        }
                    }
                    if let Some(pj) = v.pointer("/delta/partial_json").and_then(Value::as_str) {
                        let bi = v.get("index").and_then(Value::as_i64).unwrap_or(0);
                        if let Some(e) = st.tools.iter_mut().find(|e| e.0 == bi) {
                            e.3.push_str(pj);
                        }
                    }
                }
                "message_delta" => {
                    if let Some(sr) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                        st.stop_reason = map_stop_anthropic_to_openai(sr).to_string();
                    }
                    if let Some(o) = v.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                        st.out_tokens = o;
                    }
                }
                "message_start" => {
                    if let Some(i) = v
                        .pointer("/message/usage/input_tokens")
                        .and_then(Value::as_u64)
                    {
                        st.in_tokens = i;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Anthropic 上游 → OpenAI Chat 客户端（文本实时增量，工具调用末尾整块给出）。
async fn relay_anthropic_to_openai_chat(
    mut resp: reqwest::Response,
    model: &str,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    write_sse_head(stream).await?;
    let id = format!("chatcmpl-{}", &Uuid::new_v4().simple().to_string()[..12]);
    write_sse_frame(stream, None, &json!({"id":id,"object":"chat.completion.chunk","created":chrono::Utc::now().timestamp(),"model":model,"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}).to_string()).await?;
    let mut buf = String::new();
    let mut decoder = Utf8SseDecoder::default();
    let mut st = AnthropicToOpenAiState {
        stop_reason: "stop".into(),
        ..Default::default()
    };
    while let Ok(Some(bytes)) = resp.chunk().await {
        decoder.push_chunk(&bytes, &mut buf);
        drain_anthropic_events_to_openai(&mut buf, stream, &id, model, &mut st).await?;
    }
    decoder.finish(&mut buf);
    drain_anthropic_events_to_openai(&mut buf, stream, &id, model, &mut st).await?;
    let _ = st.in_tokens;
    let tools = st.tools.clone();
    for (i, (_bi, tid, name, args)) in tools.iter().enumerate() {
        let first = json!({"id":id,"object":"chat.completion.chunk","created":chrono::Utc::now().timestamp(),"model":model,"choices":[{"index":0,"delta":{"tool_calls":[{"index":i,"id":tid,"type":"function","function":{"name":name,"arguments":""}}]},"finish_reason":null}]});
        write_sse_frame(stream, None, &first.to_string()).await?;
        if !args.is_empty() {
            let args_chunk = json!({"id":id,"object":"chat.completion.chunk","created":chrono::Utc::now().timestamp(),"model":model,"choices":[{"index":0,"delta":{"tool_calls":[{"index":i,"function":{"arguments":args}}]},"finish_reason":null}]});
            write_sse_frame(stream, None, &args_chunk.to_string()).await?;
        }
    }
    let finish = if tools.is_empty() {
        st.stop_reason.clone()
    } else {
        "tool_calls".to_string()
    };
    let final_chunk = json!({"id":id,"object":"chat.completion.chunk","created":chrono::Utc::now().timestamp(),"model":model,"choices":[{"index":0,"delta":{},"finish_reason":finish}]});
    write_sse_frame(stream, None, &final_chunk.to_string()).await?;
    write_sse_frame(stream, None, "[DONE]").await
}

#[derive(Default)]
struct ResponsesAccum {
    resp_id: String,
    text: String,
    /// (上游工具序号/块序号, call_id, name, arguments)
    tools: Vec<(i64, String, String, String)>,
    message_started: bool,
    in_tokens: u64,
    out_tokens: u64,
}

fn new_responses_accum() -> ResponsesAccum {
    ResponsesAccum {
        resp_id: Uuid::new_v4().simple().to_string()[..12].to_string(),
        ..Default::default()
    }
}

fn resp_tool_slot(st: &mut ResponsesAccum, idx: i64) -> &mut (i64, String, String, String) {
    if let Some(pos) = st.tools.iter().position(|e| e.0 == idx) {
        &mut st.tools[pos]
    } else {
        st.tools
            .push((idx, String::new(), String::new(), String::new()));
        let last = st.tools.len() - 1;
        &mut st.tools[last]
    }
}

async fn responses_emit_created(
    st: &ResponsesAccum,
    model: &str,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    let in_progress = json!({"id":format!("resp_{}", st.resp_id),"object":"response","status":"in_progress","model":model,"output":[]});
    write_sse_frame(
        stream,
        Some("response.created"),
        &json!({"type":"response.created","response":in_progress}).to_string(),
    )
    .await?;
    write_sse_frame(
        stream,
        Some("response.in_progress"),
        &json!({"type":"response.in_progress","response":in_progress}).to_string(),
    )
    .await
}

async fn responses_ensure_message(
    st: &mut ResponsesAccum,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    if st.message_started {
        return Ok(());
    }
    st.message_started = true;
    let msg_id = format!("msg_{}", st.resp_id);
    write_sse_frame(
        stream,
        Some("response.output_item.added"),
        &json!({"type":"response.output_item.added","output_index":0,"item":{"id":msg_id,"type":"message","status":"in_progress","role":"assistant","content":[]}}).to_string(),
    )
    .await?;
    write_sse_frame(
        stream,
        Some("response.content_part.added"),
        &json!({"type":"response.content_part.added","item_id":msg_id,"output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}).to_string(),
    )
    .await
}

async fn responses_text_delta(
    st: &mut ResponsesAccum,
    model: &str,
    delta: &str,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    if delta.is_empty() {
        return Ok(());
    }
    let _ = model;
    responses_ensure_message(st, stream).await?;
    let msg_id = format!("msg_{}", st.resp_id);
    write_sse_frame(
        stream,
        Some("response.output_text.delta"),
        &json!({"type":"response.output_text.delta","item_id":msg_id,"output_index":0,"content_index":0,"delta":delta}).to_string(),
    )
    .await?;
    st.text.push_str(delta);
    Ok(())
}

async fn responses_finish(
    st: &mut ResponsesAccum,
    model: &str,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    let msg_id = format!("msg_{}", st.resp_id);
    let mut output_index = 0usize;
    if st.message_started {
        write_sse_frame(
            stream,
            Some("response.output_text.done"),
            &json!({"type":"response.output_text.done","item_id":msg_id,"output_index":0,"content_index":0,"text":st.text}).to_string(),
        )
        .await?;
        write_sse_frame(
            stream,
            Some("response.content_part.done"),
            &json!({"type":"response.content_part.done","item_id":msg_id,"output_index":0,"content_index":0,"part":{"type":"output_text","text":st.text,"annotations":[]}}).to_string(),
        )
        .await?;
        write_sse_frame(
            stream,
            Some("response.output_item.done"),
            &json!({"type":"response.output_item.done","output_index":0,"item":{"id":msg_id,"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":st.text,"annotations":[]}]}}).to_string(),
        )
        .await?;
        output_index = 1;
    }
    st.tools.sort_by_key(|e| e.0);
    for (i, (_idx, call_id, name, args)) in st.tools.iter().enumerate() {
        let fc_id = format!("fc_{i}_{}", st.resp_id);
        write_sse_frame(
            stream,
            Some("response.output_item.added"),
            &json!({"type":"response.output_item.added","output_index":output_index,"item":{"id":fc_id,"type":"function_call","status":"in_progress","call_id":call_id,"name":name,"arguments":""}}).to_string(),
        )
        .await?;
        if !args.is_empty() {
            write_sse_frame(
                stream,
                Some("response.function_call_arguments.delta"),
                &json!({"type":"response.function_call_arguments.delta","item_id":fc_id,"output_index":output_index,"delta":args}).to_string(),
            )
            .await?;
        }
        write_sse_frame(
            stream,
            Some("response.function_call_arguments.done"),
            &json!({"type":"response.function_call_arguments.done","item_id":fc_id,"output_index":output_index,"arguments":args}).to_string(),
        )
        .await?;
        write_sse_frame(
            stream,
            Some("response.output_item.done"),
            &json!({"type":"response.output_item.done","output_index":output_index,"item":{"id":fc_id,"type":"function_call","status":"completed","call_id":call_id,"name":name,"arguments":args}}).to_string(),
        )
        .await?;
        output_index += 1;
    }
    let ir = IrResponse {
        id: st.resp_id.clone(),
        model: model.to_string(),
        text: st.text.clone(),
        thinking: None,
        tool_calls: st
            .tools
            .iter()
            .map(|(_i, c, n, a)| (c.clone(), n.clone(), a.clone()))
            .collect(),
        finish_reason: if st.tools.is_empty() {
            "stop".into()
        } else {
            "tool_calls".into()
        },
        usage: IrUsage {
            input_tokens: st.in_tokens,
            output_tokens: st.out_tokens,
        },
    };
    let full = to_openai_responses_response(&ir);
    write_sse_frame(
        stream,
        Some("response.completed"),
        &json!({"type":"response.completed","response":full}).to_string(),
    )
    .await
}

async fn drain_openai_events_to_responses(
    buf: &mut String,
    model: &str,
    stream: &mut TcpStream,
    st: &mut ResponsesAccum,
) -> std::io::Result<()> {
    while let Some(idx) = buf.find("\n\n") {
        let block = buf[..idx].to_string();
        *buf = buf[idx + 2..].to_string();
        for line in block.lines() {
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(t) = v
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
            {
                responses_text_delta(st, model, t, stream).await?;
            }
            if let Some(arr) = v
                .pointer("/choices/0/delta/tool_calls")
                .and_then(Value::as_array)
            {
                for tc in arr {
                    let oi = tc.get("index").and_then(Value::as_i64).unwrap_or(0);
                    let id = tc
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = tc
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let args = tc
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let slot = resp_tool_slot(st, oi);
                    if !id.is_empty() {
                        slot.1 = id;
                    }
                    if !name.is_empty() {
                        slot.2 = name;
                    }
                    slot.3.push_str(&args);
                }
            }
            if let Some(o) = v
                .pointer("/usage/completion_tokens")
                .and_then(Value::as_u64)
            {
                st.out_tokens = o;
            }
            if let Some(i) = v.pointer("/usage/prompt_tokens").and_then(Value::as_u64) {
                st.in_tokens = i;
            }
        }
    }
    Ok(())
}

async fn drain_anthropic_events_to_responses(
    buf: &mut String,
    model: &str,
    stream: &mut TcpStream,
    st: &mut ResponsesAccum,
) -> std::io::Result<()> {
    while let Some(idx) = buf.find("\n\n") {
        let block = buf[..idx].to_string();
        *buf = buf[idx + 2..].to_string();
        for line in block.lines() {
            let Some(data) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            match v.get("type").and_then(Value::as_str).unwrap_or("") {
                "content_block_start" => {
                    if v.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use")
                    {
                        let bi = v.get("index").and_then(Value::as_i64).unwrap_or(0);
                        let idv = v
                            .pointer("/content_block/id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let name = v
                            .pointer("/content_block/name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let slot = resp_tool_slot(st, bi);
                        slot.1 = idv;
                        slot.2 = name;
                    }
                }
                "content_block_delta" => {
                    if let Some(t) = v.pointer("/delta/text").and_then(Value::as_str) {
                        responses_text_delta(st, model, t, stream).await?;
                    }
                    if let Some(pj) = v.pointer("/delta/partial_json").and_then(Value::as_str) {
                        let bi = v.get("index").and_then(Value::as_i64).unwrap_or(0);
                        resp_tool_slot(st, bi).3.push_str(pj);
                    }
                }
                "message_start" => {
                    if let Some(i) = v
                        .pointer("/message/usage/input_tokens")
                        .and_then(Value::as_u64)
                    {
                        st.in_tokens = i;
                    }
                }
                "message_delta" => {
                    if let Some(o) = v.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                        st.out_tokens = o;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// OpenAI Chat 上游 → OpenAI Responses 客户端（文本实时增量，工具末尾整块）。
async fn relay_openai_chat_to_responses(
    mut resp: reqwest::Response,
    model: &str,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    write_sse_head(stream).await?;
    let mut st = new_responses_accum();
    responses_emit_created(&st, model, stream).await?;
    let mut buf = String::new();
    let mut decoder = Utf8SseDecoder::default();
    while let Ok(Some(bytes)) = resp.chunk().await {
        decoder.push_chunk(&bytes, &mut buf);
        drain_openai_events_to_responses(&mut buf, model, stream, &mut st).await?;
    }
    decoder.finish(&mut buf);
    drain_openai_events_to_responses(&mut buf, model, stream, &mut st).await?;
    responses_finish(&mut st, model, stream).await
}

/// Anthropic 上游 → OpenAI Responses 客户端（文本实时增量，工具末尾整块）。
async fn relay_anthropic_to_responses(
    mut resp: reqwest::Response,
    model: &str,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    write_sse_head(stream).await?;
    let mut st = new_responses_accum();
    responses_emit_created(&st, model, stream).await?;
    let mut buf = String::new();
    let mut decoder = Utf8SseDecoder::default();
    while let Ok(Some(bytes)) = resp.chunk().await {
        decoder.push_chunk(&bytes, &mut buf);
        drain_anthropic_events_to_responses(&mut buf, model, stream, &mut st).await?;
    }
    decoder.finish(&mut buf);
    drain_anthropic_events_to_responses(&mut buf, model, stream, &mut st).await?;
    responses_finish(&mut st, model, stream).await
}

/// 将上游 SSE 响应逐块原样透传给客户端（无 Content-Length，Connection: close）。
async fn forward_sse_passthrough(
    mut resp: reqwest::Response,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-cache\r\nConnection: close\r\nX-Accel-Buffering: no\r\n\r\n";
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await?;
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                stream.write_all(&chunk).await?;
                stream.flush().await?;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    Ok(())
}

async fn call_upstream_member(
    member: &PoolMember,
    cred: &CredentialEnvelope,
    ir_req: &IrRequest,
    upstream_model: &str,
) -> Result<(u16, String), String> {
    // 码道 / Trae CN / Qoder CN 统一走各自专用协议通道（返回标准 OpenAI SSE 或 JSON 错误），
    // 非流式调用方由 parse_upstream_response_body 自动聚合 SSE 为完整 IrResponse。
    if matches!(
        member.tool_id.as_str(),
        "codearts" | "trae_cn" | "qoder_cn" | "doubaowork"
    ) {
        let resp = send_upstream_stream(member, cred, ir_req, upstream_model).await?;
        let code = resp.status().as_u16();
        let text = read_bounded_response_text(resp, &member.tool_id).await?;
        return Ok((code, text));
    }

    let client = long_running_http_client();
    let (base_url, token, extra_headers) = resolve_upstream_auth(member, cred)?;

    let (_, bare_upstream_model) = crate::domain::split_scoped_model_id(upstream_model);
    let wire_model = resolve_vendor_wire_model_id(&member.tool_id, bare_upstream_model);
    let protocol = upstream_model_protocol(&member.tool_id, &wire_model);
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
        // 防内存耗尽：声明体积过大的上游响应直接拒绝（流式为逐块转发，不受此限）
        const MAX_UPSTREAM_BODY: u64 = 64 * 1024 * 1024;
        if resp
            .content_length()
            .is_some_and(|len| len > MAX_UPSTREAM_BODY)
        {
            return Err(format!(
                "上游响应体过大（>{} MB）已拒绝",
                MAX_UPSTREAM_BODY / (1024 * 1024)
            ));
        }
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
                    .unwrap_or(0),
                output_tokens: parsed
                    .pointer("/usage/completion_tokens")
                    .or_else(|| parsed.pointer("/usage/output_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
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
                if let Some(v) = chunk
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64)
                {
                    output_tokens = Some(v);
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
            input_tokens: input_tokens.unwrap_or(0),
            output_tokens: output_tokens.unwrap_or(0),
        },
    }
}

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const CONN_READ_TIMEOUT_SECS: u64 = 30;

fn conn_read_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(CONN_READ_TIMEOUT_SECS)
}

fn conn_timeout_err() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::TimedOut, "connection read timeout")
}

/// 读取 `Transfer-Encoding: chunked` 请求体，带总大小上限与逐块读超时。
async fn read_chunked_body(reader: &mut BufReader<&mut TcpStream>) -> std::io::Result<Vec<u8>> {
    let mut body: Vec<u8> = Vec::new();
    loop {
        let mut size_line = String::new();
        let n = tokio::time::timeout(conn_read_timeout(), reader.read_line(&mut size_line))
            .await
            .map_err(|_| conn_timeout_err())??;
        if n == 0 {
            break;
        }
        let size_str = size_line.trim().split(';').next().unwrap_or("0").trim();
        let size = usize::from_str_radix(size_str, 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        if body.len().saturating_add(size) > MAX_BODY_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "chunked body exceeds limit",
            ));
        }
        let mut chunk = vec![0u8; size];
        tokio::time::timeout(conn_read_timeout(), reader.read_exact(&mut chunk))
            .await
            .map_err(|_| conn_timeout_err())??;
        body.extend_from_slice(&chunk);
        let mut crlf = [0u8; 2];
        let _ = tokio::time::timeout(conn_read_timeout(), reader.read_exact(&mut crlf))
            .await
            .map_err(|_| conn_timeout_err())?;
    }
    Ok(body)
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
    let n = tokio::time::timeout(conn_read_timeout(), reader.read_line(&mut request_line))
        .await
        .map_err(|_| conn_timeout_err())??;
    if n == 0 {
        return Ok(());
    }
    if request_line.len() > MAX_REQUEST_LINE_BYTES {
        return write_http_response(
            stream_from_reader(reader),
            "414 URI Too Long",
            "application/json",
            br#"{"error":"Request line too long"}"#,
            &[],
        )
        .await;
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
    let mut header_bytes = request_line.len();
    loop {
        let mut line = String::new();
        let n = tokio::time::timeout(conn_read_timeout(), reader.read_line(&mut line))
            .await
            .map_err(|_| conn_timeout_err())??;
        if n == 0 {
            break;
        }
        header_bytes += n;
        if header_bytes > MAX_HEADER_BYTES {
            return write_http_response(
                stream_from_reader(reader),
                "431 Request Header Fields Too Large",
                "application/json",
                br#"{"error":"Headers too large"}"#,
                &[],
            )
            .await;
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

    let is_chunked = headers
        .get("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    let body_bytes: Vec<u8> = if is_chunked {
        match read_chunked_body(&mut reader).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                return write_http_response(
                    stream_from_reader(reader),
                    "413 Payload Too Large",
                    "application/json",
                    br#"{"error":"Request body too large"}"#,
                    &[],
                )
                .await;
            }
            Err(e) => return Err(e),
        }
    } else {
        if content_len > MAX_BODY_BYTES {
            return write_http_response(
                stream_from_reader(reader),
                "413 Payload Too Large",
                "application/json",
                br#"{"error":"Request body too large"}"#,
                &[],
            )
            .await;
        }
        let mut buf = vec![0u8; content_len];
        if !buf.is_empty() {
            tokio::time::timeout(conn_read_timeout(), reader.read_exact(&mut buf))
                .await
                .map_err(|_| conn_timeout_err())??;
        }
        buf
    };
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
        } else if lower.contains("mimocode") || lower.contains("mimo") {
            Some(TargetAgentId::Mimo)
        } else if lower.contains("zcode") {
            Some(TargetAgentId::Zcode)
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

    // 跨池隔离：仅当本次调用方 Agent 已显式挂载若干池时，才把可路由范围收敛到
    // 这些池，避免“只挂载了池 A 的 Agent 请求到池 B 的同名模型”。
    let stored_bindings = db.get_stored_agent_bindings().unwrap_or_default();
    let mounted_for_caller: Option<Vec<String>> = caller_agent
        .and_then(|a| {
            stored_bindings
                .iter()
                .find(|b| b.agent_id == a)
                .map(|b| b.mounted_pool_ids.clone())
        })
        .filter(|ids| !ids.is_empty());
    let routed_pools: Vec<AccountPool> = match &mounted_for_caller {
        Some(ids) => pools
            .iter()
            .filter(|p| ids.iter().any(|id| id == &p.id))
            .cloned()
            .collect(),
        None => pools.clone(),
    };

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
        match resolve_target_pool_and_model(&routed_pools, caller_agent, &ir_req.model) {
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
                                    timestamp: now_beijing_hms(),
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
                                            RoutingAttemptOutcome::HttpError
                                        },
                                        latency_ms,
                                        detail: format!("HTTP {code} -> {}", own_auth.upstream_url),
                                    }],
                                };
                                log_db_result(
                                    "append_routing_trace",
                                    db.append_routing_trace(&trace),
                                );
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
                    if let Some(found) = routed_pools.iter().find(|p| p.enabled).and_then(|p| {
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

    // P2-9：RoundRobin 用进程内原子游标替代 today_requests 快照，避免并发请求同序惊群
    let mut pool = pool;
    if matches!(pool.strategy, crate::domain::PoolStrategy::RoundRobin) {
        pool.today_requests = runtime.next_round_robin(&pool.id);
    }

    let session_key = headers
        .get("x-conversation-id")
        .or_else(|| headers.get("x-session-id"))
        .cloned();
    let affinity_snap = runtime
        .inner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
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
        let tool_id = infer_trace_tool_id(&requested_model, &plan.skipped_attempts);
        let attempts = if plan.skipped_attempts.is_empty() {
            vec![RoutingAttempt {
                account_label: "号池调度".into(),
                tool_id: tool_id.clone(),
                model: requested_model.clone(),
                tier: None,
                outcome: RoutingAttemptOutcome::HttpError,
                latency_ms: 0,
                detail: msg.clone(),
            }]
        } else {
            plan.skipped_attempts
        };
        let trace = RoutingTraceItem {
            id: format!("tr-{}", &Uuid::new_v4().simple().to_string()[..8]),
            timestamp: now_beijing_hms(),
            agent_id: caller_agent.unwrap_or(TargetAgentId::Opencode),
            pool_id: pool.id.clone(),
            pool_name: pool.name.clone(),
            incoming_protocol: ir_req.incoming_protocol,
            upstream_protocol: WireProtocolKind::OpenaiChat,
            requested_model,
            final_account_label: "无可用候选账号".into(),
            final_tool_id: tool_id,
            final_model: "—".into(),
            status: RoutingTraceStatus::Failed,
            cache_affinity_hit: false,
            ttft_ms: 0,
            total_tokens: 0,
            attempts,
        };
        log_db_result("append_routing_trace", db.append_routing_trace(&trace));
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
    // 收集“凭据失效”这类永久性鉴权失败的账号与文案；若整个池只因它失败，
    // 最终返回 503 + 精确指引，而不是误导性的 502 first byte。
    let mut auth_expired_hits: Vec<(String, String)> = Vec::new();

    for cand in &plan.candidates {
        let Ok((_, mut cred)) = db.get_account_credential(&master_key, &cand.account_id) else {
            continue;
        };
        let t_hop = std::time::Instant::now();
        let upstream_proto = upstream_model_protocol(&cand.tool_id, &plan.effective_model);
        // 流式策略：同协议直接透传；跨协议仅当无 tools（纯文本）时做增量翻译，
        // 携带工具（编码 Agent 常态）仍走缓冲，保证 tool_calls/thinking 语义正确。
        enum StreamKind {
            Passthrough,
            OpenAiClientAnthropicUpstream,
            AnthropicClientOpenAiUpstream,
            ResponsesFromOpenAi,
            ResponsesFromAnthropic,
        }
        let stream_kind = if ir_req.stream && cand.tool_id != "workbuddy" {
            if ir_req.incoming_protocol == upstream_proto {
                Some(StreamKind::Passthrough)
            } else if ir_req.incoming_protocol == WireProtocolKind::OpenaiChat
                && upstream_proto == WireProtocolKind::AnthropicMessages
            {
                Some(StreamKind::OpenAiClientAnthropicUpstream)
            } else if ir_req.incoming_protocol == WireProtocolKind::AnthropicMessages
                && upstream_proto == WireProtocolKind::OpenaiChat
            {
                Some(StreamKind::AnthropicClientOpenAiUpstream)
            } else if runtime.responses_streaming()
                && ir_req.incoming_protocol == WireProtocolKind::OpenaiResponses
            {
                match upstream_proto {
                    WireProtocolKind::OpenaiChat => Some(StreamKind::ResponsesFromOpenAi),
                    WireProtocolKind::AnthropicMessages => Some(StreamKind::ResponsesFromAnthropic),
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(kind) = stream_kind {
            let mut did_auth_refresh = false;
            let mut stream_res =
                send_upstream_stream(cand, &cred, &ir_req, &plan.effective_model).await;
            if let Ok(ref resp) = stream_res {
                if resp.status().as_u16() == 401
                    && pool.auto_refresh_on_401
                    && cand.tool_id != "custom_api_key"
                {
                    if let Some(adapter) = adapters.get(&cand.tool_id) {
                        let lock = runtime.refresh_lock_for(&cand.account_id);
                        let _guard = lock.lock().await;
                        if let Ok((_, fresh)) =
                            db.get_account_credential(&master_key, &cand.account_id)
                        {
                            cred = fresh;
                        }
                        if let Ok(Some(refreshed)) = adapter.force_refresh_credential(&cred).await {
                            did_auth_refresh = true;
                            let _ = db.bind_account_credential(
                                &master_key,
                                &cand.account_id,
                                None,
                                &refreshed,
                            );
                            cred = refreshed;
                            attempts.push(RoutingAttempt {
                                account_label: cand.label.clone(),
                                tool_id: cand.tool_id.clone(),
                                model: plan.effective_model.clone(),
                                tier: Some(cand.tier),
                                outcome: RoutingAttemptOutcome::Refreshed401,
                                latency_ms: t_hop.elapsed().as_millis() as u64,
                                detail: "401 单飞强制刷新凭据并回写加密库".into(),
                            });
                            stream_res =
                                send_upstream_stream(cand, &cred, &ir_req, &plan.effective_model)
                                    .await;
                        }
                    }
                }
            }
            loop {
                match stream_res {
                    Ok(resp) if resp.status().is_success() => {
                        let detail = match kind {
                            StreamKind::Passthrough => "网关实时流式透传成功（首字节后锁定上游）",
                            StreamKind::OpenAiClientAnthropicUpstream => {
                                "跨协议增量流式（Anthropic 上游 → OpenAI 客户端）"
                            }
                            StreamKind::AnthropicClientOpenAiUpstream => {
                                "跨协议增量流式（OpenAI 上游 → Anthropic 客户端）"
                            }
                            StreamKind::ResponsesFromOpenAi => {
                                "跨协议增量流式（OpenAI 上游 → Responses 客户端）"
                            }
                            StreamKind::ResponsesFromAnthropic => {
                                "跨协议增量流式（Anthropic 上游 → Responses 客户端）"
                            }
                        };
                        attempts.push(RoutingAttempt {
                            account_label: cand.label.clone(),
                            tool_id: cand.tool_id.clone(),
                            model: plan.effective_model.clone(),
                            tier: Some(cand.tier),
                            outcome: RoutingAttemptOutcome::Ok,
                            latency_ms: t_hop.elapsed().as_millis() as u64,
                            detail: detail.into(),
                        });
                        let trace = RoutingTraceItem {
                            id: format!("tr-{}", &Uuid::new_v4().simple().to_string()[..8]),
                            timestamp: now_beijing_hms(),
                            agent_id: caller_agent.unwrap_or(TargetAgentId::Opencode),
                            pool_id: pool.id.clone(),
                            pool_name: pool.name.clone(),
                            incoming_protocol: ir_req.incoming_protocol,
                            upstream_protocol: upstream_proto,
                            requested_model: ir_req.model.clone(),
                            final_account_label: cand.label.clone(),
                            final_tool_id: cand.tool_id.clone(),
                            final_model: plan.effective_model.clone(),
                            status: if attempts.len() > 1 {
                                RoutingTraceStatus::FailoverSuccess
                            } else {
                                RoutingTraceStatus::Success
                            },
                            cache_affinity_hit: plan.cache_affinity_hit,
                            ttft_ms: t_hop.elapsed().as_millis() as u64,
                            total_tokens: 0,
                            attempts,
                        };
                        log_db_result("append_routing_trace", db.append_routing_trace(&trace));
                        log_db_result(
                            "increment_pool_stats",
                            db.increment_pool_stats(
                                &pool.id,
                                trace.status == RoutingTraceStatus::FailoverSuccess,
                                plan.cache_affinity_hit,
                            ),
                        );
                        return match kind {
                            StreamKind::Passthrough => forward_sse_passthrough(resp, stream).await,
                            StreamKind::OpenAiClientAnthropicUpstream => {
                                relay_anthropic_to_openai_chat(resp, &ir_req.model, stream).await
                            }
                            StreamKind::AnthropicClientOpenAiUpstream => {
                                relay_openai_chat_to_anthropic(resp, &ir_req.model, stream).await
                            }
                            StreamKind::ResponsesFromOpenAi => {
                                relay_openai_chat_to_responses(resp, &ir_req.model, stream).await
                            }
                            StreamKind::ResponsesFromAnthropic => {
                                relay_anthropic_to_responses(resp, &ir_req.model, stream).await
                            }
                        };
                    }
                    Ok(resp) => {
                        let code = resp.status().as_u16();
                        let body_txt = resp.text().await.unwrap_or_default();
                        if is_concurrency_limit_response(code, &body_txt) {
                            let is_session_cap = code != 429;
                            let cool_sec = if is_session_cap {
                                pool.cooldown_seconds.min(15)
                            } else {
                                pool.cooldown_seconds
                            };
                            runtime.set_member_cooldown(
                                &cand.account_id,
                                chrono::Utc::now().timestamp_millis() + (cool_sec as i64) * 1000,
                            );
                            attempts.push(RoutingAttempt {
                                account_label: cand.label.clone(),
                                tool_id: cand.tool_id.clone(),
                                model: plan.effective_model.clone(),
                                tier: Some(cand.tier),
                                outcome: RoutingAttemptOutcome::Cooldown429,
                                latency_ms: t_hop.elapsed().as_millis() as u64,
                                detail: if is_session_cap {
                                    format!("并发会话数已达上限(3个)，请关闭码道桌面端或其它并发会话后重试（冷却 {cool_sec}s）")
                                } else {
                                    format!(
                                        "HTTP 429 限流，置入 {cool_sec}s 冷却并切换下一账号"
                                    )
                                },
                            });
                            break;
                        } else if is_auth_expired_response(code, &body_txt) {
                            // 部分上游（如华为云 APIG.0602）用 HTTP 400 返回 token 过期；
                            // 首次命中时尝试单飞刷新/本机回吸并重试一次，仍失效再标过期。
                            if !did_auth_refresh
                                && pool.auto_refresh_on_401
                                && cand.tool_id != "custom_api_key"
                            {
                                if let Some(adapter) = adapters.get(&cand.tool_id) {
                                    let lock = runtime.refresh_lock_for(&cand.account_id);
                                    let _guard = lock.lock().await;
                                    if let Ok((_, fresh)) =
                                        db.get_account_credential(&master_key, &cand.account_id)
                                    {
                                        cred = fresh;
                                    }
                                    if let Ok(Some(refreshed)) =
                                        adapter.force_refresh_credential(&cred).await
                                    {
                                        did_auth_refresh = true;
                                        let _ = db.bind_account_credential(
                                            &master_key,
                                            &cand.account_id,
                                            None,
                                            &refreshed,
                                        );
                                        cred = refreshed;
                                        attempts.push(RoutingAttempt {
                                            account_label: cand.label.clone(),
                                            tool_id: cand.tool_id.clone(),
                                            model: plan.effective_model.clone(),
                                            tier: Some(cand.tier),
                                            outcome: RoutingAttemptOutcome::Refreshed401,
                                            latency_ms: t_hop.elapsed().as_millis() as u64,
                                            detail: format!(
                                                "HTTP {code} 凭据过期单飞刷新并回写加密库"
                                            ),
                                        });
                                        stream_res = send_upstream_stream(
                                            cand,
                                            &cred,
                                            &ir_req,
                                            &plan.effective_model,
                                        )
                                        .await;
                                        continue;
                                    }
                                }
                            }
                            let _ = db.update_account_status(
                                &cand.account_id,
                                crate::domain::AccountStatus::Expired,
                            );
                            let reason = auth_expired_reason(&cand.tool_id, &body_txt);
                            auth_expired_hits.push((cand.label.clone(), reason.clone()));
                            attempts.push(RoutingAttempt {
                                account_label: cand.label.clone(),
                                tool_id: cand.tool_id.clone(),
                                model: plan.effective_model.clone(),
                                tier: Some(cand.tier),
                                outcome: RoutingAttemptOutcome::AuthExpired,
                                latency_ms: t_hop.elapsed().as_millis() as u64,
                                detail: format!("凭据失效(HTTP {code})：{reason}"),
                            });
                            break;
                        } else {
                            attempts.push(RoutingAttempt {
                                account_label: cand.label.clone(),
                                tool_id: cand.tool_id.clone(),
                                model: plan.effective_model.clone(),
                                tier: Some(cand.tier),
                                outcome: RoutingAttemptOutcome::HttpError,
                                latency_ms: t_hop.elapsed().as_millis() as u64,
                                detail: format!(
                                    "上游 HTTP {code}（非限流）: {}",
                                    body_txt.chars().take(120).collect::<String>()
                                ),
                            });
                            break;
                        }
                    }
                    Err(e) => {
                        attempts.push(RoutingAttempt {
                            account_label: cand.label.clone(),
                            tool_id: cand.tool_id.clone(),
                            model: plan.effective_model.clone(),
                            tier: Some(cand.tier),
                            outcome: RoutingAttemptOutcome::HttpError,
                            latency_ms: t_hop.elapsed().as_millis() as u64,
                            detail: format!("上游请求未到达/传输失败: {e}"),
                        });
                        break;
                    }
                }
            }
            continue;
        }
        let mut res = call_upstream_member(cand, &cred, &ir_req, &plan.effective_model).await;

        if let Ok((code, ref body_txt)) = res {
            if is_auth_expired_response(code, body_txt)
                && pool.auto_refresh_on_401
                && cand.tool_id != "custom_api_key"
            {
                if let Some(adapter) = adapters.get(&cand.tool_id) {
                    let lock = runtime.refresh_lock_for(&cand.account_id);
                    let _guard = lock.lock().await;
                    if let Ok((_, fresh)) = db.get_account_credential(&master_key, &cand.account_id)
                    {
                        cred = fresh;
                    }
                    if let Ok(Some(refreshed)) = adapter.force_refresh_credential(&cred).await {
                        let _ = db.bind_account_credential(
                            &master_key,
                            &cand.account_id,
                            None,
                            &refreshed,
                        );
                        cred = refreshed;
                        attempts.push(RoutingAttempt {
                            account_label: cand.label.clone(),
                            tool_id: cand.tool_id.clone(),
                            model: plan.effective_model.clone(),
                            tier: Some(cand.tier),
                            outcome: RoutingAttemptOutcome::Refreshed401,
                            latency_ms: t_hop.elapsed().as_millis() as u64,
                            detail: "401 单飞强制刷新凭据并回写加密库".into(),
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
            Ok((code, err_body)) if is_concurrency_limit_response(code, &err_body) => {
                let is_session_cap = code != 429;
                let cool_sec = if is_session_cap {
                    pool.cooldown_seconds.min(15)
                } else {
                    pool.cooldown_seconds
                };
                runtime.set_member_cooldown(
                    &cand.account_id,
                    chrono::Utc::now().timestamp_millis() + (cool_sec as i64) * 1000,
                );
                attempts.push(RoutingAttempt {
                    account_label: cand.label.clone(),
                    tool_id: cand.tool_id.clone(),
                    model: plan.effective_model.clone(),
                    tier: Some(cand.tier),
                    outcome: RoutingAttemptOutcome::Cooldown429,
                    latency_ms: t_hop.elapsed().as_millis() as u64,
                    detail: if is_session_cap {
                        format!("并发会话数已达上限(3个)，请关闭码道桌面端或其它并发会话后重试（冷却 {cool_sec}s）")
                    } else {
                        format!(
                            "HTTP 429 限流，置入 {cool_sec}s 冷却并切换下一账号"
                        )
                    },
                });
            }
            Ok((code, err_body)) if is_auth_expired_response(code, &err_body) => {
                // 永久性鉴权失败（401 / 华为云 APIG.0301）：标记当前账号过期后继续 Failover 下一健康备号。
                let _ = db
                    .update_account_status(&cand.account_id, crate::domain::AccountStatus::Expired);
                let reason = auth_expired_reason(&cand.tool_id, &err_body);
                auth_expired_hits.push((cand.label.clone(), reason.clone()));
                attempts.push(RoutingAttempt {
                    account_label: cand.label.clone(),
                    tool_id: cand.tool_id.clone(),
                    model: plan.effective_model.clone(),
                    tier: Some(cand.tier),
                    outcome: RoutingAttemptOutcome::AuthExpired,
                    latency_ms: t_hop.elapsed().as_millis() as u64,
                    detail: format!("凭据失效(HTTP {code})：{reason}"),
                });
                continue;
            }
            Ok((code, err_body)) => {
                attempts.push(RoutingAttempt {
                    account_label: cand.label.clone(),
                    tool_id: cand.tool_id.clone(),
                    model: plan.effective_model.clone(),
                    tier: Some(cand.tier),
                    outcome: RoutingAttemptOutcome::HttpError,
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
                    outcome: RoutingAttemptOutcome::HttpError,
                    latency_ms: t_hop.elapsed().as_millis() as u64,
                    detail: e,
                });
            }
        }
    }

    let Some((ir_resp, winner)) = final_ir_resp else {
        // 抽一条最具体的失败明细，让客户端直接看到「上游 404 / 协议不匹配」等真实原因，
        // 而不是永远只有一句误导性的 "All pool candidates failed before first byte"。
        let first_detail = attempts
            .iter()
            .find(|a| a.outcome == RoutingAttemptOutcome::HttpError)
            .map(|a| format!("{}@{}: {}", a.tool_id, a.account_label, a.detail.clone()))
            .or_else(|| {
                attempts
                    .iter()
                    .find(|a| a.outcome == RoutingAttemptOutcome::Cooldown429)
                    .map(|a| a.detail.clone())
            })
            .unwrap_or_else(|| "无可用候选账号".to_string());
        let final_tool_id = infer_trace_tool_id(&requested_model, &attempts);
        let trace = RoutingTraceItem {
            id: format!("tr-{}", &Uuid::new_v4().simple().to_string()[..8]),
            timestamp: now_beijing_hms(),
            agent_id: caller_agent.unwrap_or(TargetAgentId::Opencode),
            pool_id: pool.id.clone(),
            pool_name: pool.name.clone(),
            incoming_protocol: ir_req.incoming_protocol,
            upstream_protocol: WireProtocolKind::OpenaiChat,
            requested_model,
            final_account_label: "全部候选均失败".into(),
            final_tool_id,
            final_model: "—".into(),
            status: RoutingTraceStatus::Failed,
            cache_affinity_hit: false,
            ttft_ms: t_start.elapsed().as_millis() as u64,
            total_tokens: 0,
            attempts,
        };
        log_db_result("append_routing_trace", db.append_routing_trace(&trace));
        // 若本次失败全部/主要源于凭据失效，返回可操作的 503 与精确文案，
        // 而不是误导性的 502 "first byte"。
        if !auth_expired_hits.is_empty() {
            let first_label = &auth_expired_hits[0].0;
            let first_reason = &auth_expired_hits[0].1;
            let summary =
                format!("凭据失效（{first_label}）：{first_reason}，请重新登录或导入该账号");
            let payload = serde_json::json!({
                "error": {
                    "message": summary,
                    "type": "auth_expired",
                },
                "message": summary,
                "error_type": "auth_expired",
                "account": first_label,
                "reason": first_reason,
                "hint": "请在对应客户端重新登录以刷新临时凭据，再回 Qwitch 重新导入该账号",
            })
            .to_string();
            return write_http_response(
                stream,
                "503 Service Unavailable",
                "application/json",
                payload.as_bytes(),
                &[],
            )
            .await;
        }
        // 抽一条最具体的失败明细，让客户端直接看到真实原因。
        let summary = format!("全部候选账号均失败：{first_detail}");
        let payload = serde_json::json!({
            "error": {
                "message": summary,
                "type": "all_candidates_failed",
            },
            "message": summary,
            "error_type": "all_candidates_failed",
            "detail": first_detail,
            "hint": "常见原因：上游账号限流/并发会话数已满、凭据过期，或模型映射不匹配。",
        })
        .to_string();
        return write_http_response(
            stream,
            "502 Bad Gateway",
            "application/json",
            payload.as_bytes(),
            &[],
        )
        .await;
    };

    let trace = RoutingTraceItem {
        id: format!("tr-{}", &Uuid::new_v4().simple().to_string()[..8]),
        timestamp: now_beijing_hms(),
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
    log_db_result("append_routing_trace", db.append_routing_trace(&trace));
    log_db_result(
        "increment_pool_stats",
        db.increment_pool_stats(
            &pool.id,
            trace.status == RoutingTraceStatus::FailoverSuccess,
            plan.cache_affinity_hit,
        ),
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

    #[test]
    fn trae_sse_to_openai_emits_text_and_done() {
        // output 事件承载增量；extra_info/metadata/timing_cost 等控制事件即使带 content
        // 也不能当正文（extra_info.content 是全量答案 JSON，必须忽略）。
        let raw = "event: output\ndata: {\"response\":\"你好\"}\n\nevent: output\ndata: {\"content\":\"世界\"}\n\nevent: extra_info\ndata: {\"content\":\"{\\\"reasoning_content\\\":\\\"全量答案\\\"}\"}\n\nevent: metadata\ndata: {\"model\":\"\"}\n\nevent: done\ndata: {\"finish_reason\":\"stop\"}\n\n";
        let sse = trae_sse_to_openai(raw, "trae-model", "trae-model");
        assert!(sse.contains("\"object\":\"chat.completion.chunk\""));
        assert!(sse.contains("你好"));
        assert!(sse.contains("世界"));
        assert!(
            !sse.contains("全量答案"),
            "extra_info.content must be ignored: {sse}"
        );
        assert!(sse.contains("\"finish_reason\":\"stop\""));
        assert!(sse.trim_end().ends_with("data: [DONE]"));
    }

    #[test]
    fn trae_first_error_detects_error_event() {
        let raw = "event: error\ndata: {\"code\":4001,\"message\":\"param is invalid\"}\n\n";
        let (code, msg) = trae_first_error(200, raw).expect("应识别错误事件");
        assert_eq!(code.as_deref(), Some("4001"));
        assert!(msg.contains("param is invalid"));
        // 正常输出不算错误
        let ok = "event: output\ndata: {\"response\":\"hi\"}\n\nevent: done\ndata: {}\n\n";
        assert!(trae_first_error(200, ok).is_none());
    }

    #[test]
    fn qoder_sse_to_openai_unwraps_envelope() {
        // 贴近真实的 Qoder SSE：文本帧 + finish 帧（delta.content 空）+ 仅 usage 的 choices:[] 帧
        // + [DONE] + 末尾 event:finish 元信息帧。
        let raw = concat!(
            "data: {\"statusCodeValue\":200,\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"你好\\\"},\\\"index\\\":0}]}\"}\n\n",
            "data: {\"statusCodeValue\":200,\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"世界\\\"},\\\"index\\\":0}]}\"}\n\n",
            "data: {\"statusCodeValue\":200,\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"\\\"},\\\"finish_reason\\\":\\\"stop\\\",\\\"index\\\":0}]}\"}\n\n",
            "data: {\"statusCodeValue\":200,\"body\":\"{\\\"choices\\\":[],\\\"usage\\\":{\\\"prompt_tokens\\\":10,\\\"completion_tokens\\\":3,\\\"total_tokens\\\":13}}\"}\n\n",
            "data: {\"statusCodeValue\":200,\"body\":\"[DONE]\",\"statusCode\":\"OK\"}\n\n",
            "event:finish\ndata: {\"firstTokenDuration\":356}\n\n",
        );
        let sse = qoder_sse_to_openai(raw, "qoder-model");
        assert!(sse.contains("\"object\":\"chat.completion.chunk\""));
        assert!(sse.contains("你好"));
        assert!(sse.contains("世界"));
        assert!(sse.contains("\"finish_reason\":\"stop\""));
        assert!(sse.contains("\"total_tokens\":13"));
        assert!(!sse.contains("\"tool_calls\":null"));
        assert!(sse.trim_end().ends_with("data: [DONE]"));
    }

    #[test]
    fn qoder_first_error_detects_non_200_frame() {
        // HTTP 200 但帧内 statusCodeValue=400
        let raw = "data: {\"statusCodeValue\":400,\"body\":\"bad model\"}\n\n";
        let (code, msg) = qoder_first_error(200, raw).expect("应识别非 200 帧");
        assert_eq!(code.as_deref(), Some("400"));
        assert!(msg.contains("bad model"));
        // 正常帧不算错误
        let ok = "data: {\"statusCodeValue\":200,\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"x\\\"}}]}\"}\n\n";
        assert!(qoder_first_error(200, ok).is_none());
    }

    #[tokio::test]
    async fn gateway_lobsterai_routes_via_api_proxy_and_streams() {
        // 有道龙虾：上游是 {base}/v1/chat/completions（base=.../api/proxy），OpenAI 兼容 SSE
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/proxy/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(concat!(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"龙虾\"}}]}\n\n",
                        "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}\n\n",
                        "data: [DONE]\n\n"
                    )),
            )
            .mount(&mock)
            .await;

        let (_db, rt, port, root, model_id) = spawn_gateway_with_custom_account(
            "lobsterai",
            "deepseek-flash",
            format!("{}/api/proxy", mock.uri()),
            "lobster",
        )
        .await;
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .bearer_auth("qwitch-bind-opencode")
            .json(&json!({"model": model_id, "stream": true, "messages": [{"role": "user", "content": "hi"}]}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("龙虾"), "body: {body}");
        assert!(body.contains("你好"), "body: {body}");

        // 校验请求带上了 LobsterAI 客户端头
        let received = mock.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(
            received[0]
                .headers
                .get("user-agent")
                .unwrap()
                .to_str()
                .unwrap(),
            "LobsterAI/2026.11.20"
        );
        assert!(received[0]
            .headers
            .get("x-lobsterai-client-capabilities")
            .is_some());
        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn gateway_trae_builds_content_array_and_converts_sse() {
        // Trae CN：POST /api/agent/v3/llm_utils_chat，请求消息 content 必须是数组块，
        // 响应 event:output 承载增量，extra_info 的 content 必须忽略。
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v3/llm_utils_chat"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(concat!(
                        "event: output\ndata: {\"response\":\"\",\"reasoning_content\":\"思考\"}\n\n",
                        "event: output\ndata: {\"response\":\"豆包\",\"reasoning_content\":\"\"}\n\n",
                        "event: output\ndata: {\"response\":\"你好\",\"reasoning_content\":\"\"}\n\n",
                        "event: extra_info\ndata: {\"content\":\"{\\\"reasoning_content\\\":\\\"应为噪声\\\"}\"}\n\n",
                        "event: token_usage\ndata: {\"prompt_tokens\":9,\"completion_tokens\":4,\"total_tokens\":13}\n\n",
                        "event: done\ndata: {\"finish_reason\":\"stop\"}\n\n",
                    )),
            )
            .mount(&mock)
            .await;

        let (_db, rt, port, root, model_id) = spawn_gateway_with_custom_account(
            "trae_cn",
            "Doubao-Seed-2.1-Turbo",
            mock.uri(),
            "trae",
        )
        .await;
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .bearer_auth("qwitch-bind-opencode")
            .json(&json!({"model": model_id, "stream": true, "messages": [{"role": "user", "content": "hi"}]}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("豆包"), "body: {body}");
        assert!(body.contains("你好"), "body: {body}");
        assert!(body.contains("思考"), "reasoning should stream: {body}");
        assert!(
            !body.contains("应为噪声"),
            "extra_info must be ignored: {body}"
        );
        assert!(body.contains("\"finish_reason\":\"stop\""), "body: {body}");
        assert!(body.contains("\"total_tokens\":13"), "body: {body}");
        assert!(body.trim_end().ends_with("data: [DONE]"), "body: {body}");

        // 校验上游请求体：messages[].content 是数组块，function/config_name/model 正确
        let received = mock.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        let req_json: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(req_json["function"], "chat_v3");
        assert_eq!(req_json["config_name"], "Doubao-Seed-2.1-Turbo");
        assert_eq!(req_json["stream"], true);
        assert!(
            req_json["messages"][0]["content"].is_array(),
            "content must be array: {req_json}"
        );
        assert!(received[0].headers.get("authorization").is_some());
        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn gateway_qoder_streams_and_converts_real_sse_shape() {
        // 用 wiremock 模拟 Qoder 的 COSY 端点，返回真实信封格式（文本帧 + usage 帧 + [DONE] + event:finish）
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/algo/api/v2/service/pro/sse/agent_chat_generation",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(concat!(
                        "data:{\"headers\":{\"Content-Type\":[\"application/json\"]},\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"你好\\\",\\\"role\\\":\\\"assistant\\\"},\\\"index\\\":0}],\\\"model\\\":\\\"auto\\\",\\\"object\\\":\\\"chat.completion.chunk\\\"}\",\"statusCodeValue\":200,\"statusCode\":\"OK\"}\n\n",
                        "data:{\"headers\":{\"Content-Type\":[\"application/json\"]},\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"世界\\\"},\\\"index\\\":0}],\\\"model\\\":\\\"auto\\\",\\\"object\\\":\\\"chat.completion.chunk\\\"}\",\"statusCodeValue\":200,\"statusCode\":\"OK\"}\n\n",
                        "data:{\"headers\":{\"Content-Type\":[\"application/json\"]},\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"\\\"},\\\"finish_reason\\\":\\\"stop\\\",\\\"index\\\":0}],\\\"model\\\":\\\"auto\\\",\\\"object\\\":\\\"chat.completion.chunk\\\"}\",\"statusCodeValue\":200,\"statusCode\":\"OK\"}\n\n",
                        "data:{\"headers\":{\"Content-Type\":[\"application/json\"]},\"body\":\"{\\\"choices\\\":[],\\\"usage\\\":{\\\"prompt_tokens\\\":8,\\\"completion_tokens\\\":2,\\\"total_tokens\\\":10},\\\"model\\\":\\\"auto\\\"}\",\"statusCodeValue\":200,\"statusCode\":\"OK\"}\n\n",
                        "data:{\"headers\":{\"Content-Type\":[\"application/json\"]},\"body\":\"[DONE]\",\"statusCodeValue\":200,\"statusCode\":\"OK\"}\n\n",
                        "event:finish\ndata:{\"firstTokenDuration\":100}\n\n",
                    )),
            )
            .mount(&mock)
            .await;

        let (_db, rt, port, root, model_id) =
            spawn_gateway_with_custom_account("qoder_cn", "dfmodel", mock.uri(), "qoder").await;
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .bearer_auth("qwitch-bind-opencode")
            .json(&json!({"model": model_id, "stream": true, "messages": [{"role": "user", "content": "hi"}]}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("chat.completion.chunk"), "body: {body}");
        assert!(body.contains("你好"), "body: {body}");
        assert!(body.contains("世界"), "body: {body}");
        assert!(body.contains("\"finish_reason\":\"stop\""), "body: {body}");
        assert!(body.contains("\"total_tokens\":10"), "body: {body}");
        assert!(body.trim_end().ends_with("data: [DONE]"), "body: {body}");
        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn classifies_auth_expired_responses() {
        // 华为云 APIG.0301 两条真实文案（signature fail / access key not found）
        assert!(is_auth_expired_response(
            401,
            r#"{"error_msg":"Incorrect IAM authentication information: verify ak sk signature fail","error_code":"APIG.0301"}"#
        ));
        assert!(is_auth_expired_response(
            401,
            r#"{"error_msg":"Incorrect IAM authentication information: the access key HSTXXXX is not found","error_code":"APIG.0301"}"#
        ));
        // 裸 401 无体也算
        assert!(is_auth_expired_response(401, ""));
        // 429/500/普通 4xx 不算
        assert!(!is_auth_expired_response(429, "rate limited"));
        assert!(!is_auth_expired_response(500, "boom"));
        assert!(!is_auth_expired_response(400, "bad request"));
    }

    #[test]
    fn auth_expired_reason_extracts_error_code_and_hint() {
        let r = auth_expired_reason(
            "codearts",
            r#"{"error_msg":"the access key HSTXXXX is not found","error_code":"APIG.0301"}"#,
        );
        assert!(r.contains("APIG.0301"));
        assert!(r.contains("access key"));
        assert!(r.contains("重新登录"));
    }

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
    async fn gateway_streams_openai_chat_sse_passthrough() {
        let root = std::env::temp_dir().join(format!("qwitch-gw-stream-{}", Uuid::new_v4()));
        let db = Arc::new(QwitchDatabase::open(&root.join("qwitch.db")).unwrap());
        let key = [9u8; 32];

        let mock_upstream = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
                        "text/event-stream",
                    ),
            )
            .mount(&mock_upstream)
            .await;

        let mut extra = HashMap::new();
        extra.insert("base_url".into(), mock_upstream.uri());
        let acc = db
            .upsert_account(
                &key,
                Some("流式兜底"),
                &CredentialEnvelope {
                    tool_id: "custom_api_key".into(),
                    uid: "sk-stream".into(),
                    display_name: Some("流式兜底".into()),
                    display_email: None,
                    payload: CredentialPayload::Token {
                        access_token: "sk-stream".into(),
                        refresh_token: None,
                        expires_at: None,
                        extra,
                    },
                },
            )
            .unwrap();
        db.create_pool(
            &key,
            &CreatePoolInput {
                id: None,
                name: "Stream Pool".into(),
                description: "".into(),
                scene_tag: "Coding".into(),
                strategy: PoolStrategy::Order,
                affinity_mode: AffinityMode::Off,
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
            rt.inner.lock().unwrap().port = 0;
        }
        let port = rt
            .start_server(Arc::clone(&db), key, Arc::new(HashMap::new()))
            .await
            .expect("start gateway");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .bearer_auth("qwitch-bind-opencode")
            .json(&json!({
                "model": "deepseek-v4-pro",
                "stream": true,
                "messages": [{ "role": "user", "content": "ping" }]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(
            body.contains("data:"),
            "expected SSE passthrough, got: {body}"
        );

        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn accepts_chunked_request_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let root = std::env::temp_dir().join(format!("qwitch-gw-chunked-{}", Uuid::new_v4()));
        let db = Arc::new(QwitchDatabase::open(&root.join("qwitch.db")).unwrap());
        let key = [11u8; 32];
        let rt = Arc::new(GatewayRuntime::new());
        {
            rt.inner.lock().unwrap().port = 0;
        }
        let port = rt
            .start_server(Arc::clone(&db), key, Arc::new(HashMap::new()))
            .await
            .expect("start gateway");

        // 用一个足够大的请求体，验证 chunked 解析（若被忽略则估算会退化为最小值 16）
        let body = format!("{{\"model\":\"x\",\"padding\":\"{}\"}}", "a".repeat(300));
        let chunked = format!("{:x}\r\n{}\r\n0\r\n\r\n", body.len(), body);
        let req = format!(
            "POST /v1/messages/count_tokens HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer qwitch-bind-opencode\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{chunked}"
        );
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await.unwrap();
        let text = String::from_utf8_lossy(&resp);
        assert!(text.starts_with("HTTP/1.1 200"), "resp: {text}");
        assert!(
            !text.contains("\"input_tokens\":16"),
            "chunked body should be parsed (est > 16), resp: {text}"
        );

        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    async fn spawn_gateway_with_custom_account(
        tool_id: &str,
        model_id: &str,
        base_url: String,
        tag: &str,
    ) -> (
        Arc<QwitchDatabase>,
        Arc<GatewayRuntime>,
        u16,
        std::path::PathBuf,
        String,
    ) {
        let root = std::env::temp_dir().join(format!("qwitch-cross-{tag}-{}", Uuid::new_v4()));
        let db = Arc::new(QwitchDatabase::open(&root.join("qwitch.db")).unwrap());
        let key = [21u8; 32];
        let mut extra = HashMap::new();
        extra.insert("base_url".into(), base_url);
        let acc = db
            .upsert_account(
                &key,
                Some("跨协议账号"),
                &CredentialEnvelope {
                    tool_id: tool_id.into(),
                    uid: format!("uid-{tag}"),
                    display_name: Some("跨协议账号".into()),
                    display_email: None,
                    payload: CredentialPayload::Token {
                        access_token: "tok-cross".into(),
                        refresh_token: None,
                        expires_at: None,
                        extra,
                    },
                },
            )
            .unwrap();
        db.create_pool(
            &key,
            &CreatePoolInput {
                id: None,
                name: format!("Cross {tag}"),
                description: String::new(),
                scene_tag: "Coding".into(),
                strategy: PoolStrategy::Order,
                affinity_mode: AffinityMode::Off,
                skip_active_client_account: false,
                cooldown_seconds: 60,
                auto_refresh_on_401: false,
                fallback_api_key_enabled: true,
                members: vec![CreatePoolMemberInput {
                    account_id: acc.id,
                    supported_models: vec![model_id.into()],
                    tier: MemberTier::T1Primary,
                }],
                enabled_model_ids: None,
            },
        )
        .unwrap();
        let rt = Arc::new(GatewayRuntime::new());
        {
            rt.inner.lock().unwrap().port = 0;
        }
        let port = rt
            .start_server(Arc::clone(&db), key, Arc::new(HashMap::new()))
            .await
            .expect("start gateway");
        let actual_model = db
            .list_pools(&key, &HashMap::new())
            .unwrap()
            .into_iter()
            .flat_map(|p| p.model_policies)
            .map(|p| p.model_id)
            .next()
            .unwrap_or_else(|| model_id.to_string());
        (db, rt, port, root, actual_model)
    }

    #[tokio::test]
    async fn cross_protocol_openai_client_from_anthropic_upstream_streams() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(
                        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
                        "text/event-stream",
                    ),
            )
            .mount(&mock)
            .await;
        let (_db, rt, port, root, model_id) =
            spawn_gateway_with_custom_account("minimax_code_cn", "MiniMax-M2.5", mock.uri(), "a")
                .await;
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .bearer_auth("qwitch-bind-opencode")
            .json(&json!({"model": model_id, "stream": true, "messages": [{"role": "user", "content": "hi"}]}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("chat.completion.chunk"), "body: {body}");
        assert!(body.contains("Hello"), "body: {body}");
        assert!(body.contains("[DONE]"), "body: {body}");
        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn cross_protocol_anthropic_client_from_openai_upstream_streams() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                        "text/event-stream",
                    ),
            )
            .mount(&mock)
            .await;
        let (_db, rt, port, root, model_id) =
            spawn_gateway_with_custom_account("custom_api_key", "deepseek-v4-pro", mock.uri(), "b")
                .await;
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/messages"))
            .bearer_auth("qwitch-bind-claude-desktop")
            .json(&json!({"model": model_id, "stream": true, "max_tokens": 64, "messages": [{"role": "user", "content": "hi"}]}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("content_block_delta"), "body: {body}");
        assert!(body.contains("Hi"), "body: {body}");
        assert!(body.contains("message_stop"), "body: {body}");
        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn cross_protocol_tool_calls_openai_client_from_anthropic_upstream() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(
                        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":5}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read_file\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                        "text/event-stream",
                    ),
            )
            .mount(&mock)
            .await;
        let (_db, rt, port, root, model_id) =
            spawn_gateway_with_custom_account("minimax_code_cn", "MiniMax-M2.5", mock.uri(), "cta")
                .await;
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .bearer_auth("qwitch-bind-opencode")
            .json(&json!({
                "model": model_id,
                "stream": true,
                "messages": [{"role": "user", "content": "read a.txt"}],
                "tools": [{"type": "function", "function": {"name": "read_file", "parameters": {"type": "object", "properties": {}}}}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("tool_calls"), "body: {body}");
        assert!(body.contains("read_file"), "body: {body}");
        assert!(body.contains("a.txt"), "body: {body}");
        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn cross_protocol_tool_calls_anthropic_client_from_openai_upstream() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(
                        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}}]}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n",
                        "text/event-stream",
                    ),
            )
            .mount(&mock)
            .await;
        let (_db, rt, port, root, model_id) = spawn_gateway_with_custom_account(
            "custom_api_key",
            "deepseek-v4-pro",
            mock.uri(),
            "ctb",
        )
        .await;
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/messages"))
            .bearer_auth("qwitch-bind-claude-desktop")
            .json(&json!({
                "model": model_id,
                "stream": true,
                "max_tokens": 64,
                "messages": [{"role": "user", "content": "read a.txt"}],
                "tools": [{"name": "read_file", "input_schema": {"type": "object", "properties": {}}}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("tool_use"), "body: {body}");
        assert!(body.contains("read_file"), "body: {body}");
        assert!(body.contains("a.txt"), "body: {body}");
        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn responses_client_streams_from_anthropic_upstream_with_tools() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(
                        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":7}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"read_file\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"b.txt\\\"}\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":4}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                        "text/event-stream",
                    ),
            )
            .mount(&mock)
            .await;
        let (_db, rt, port, root, model_id) =
            spawn_gateway_with_custom_account("minimax_code_cn", "MiniMax-M2.5", mock.uri(), "rsa")
                .await;
        rt.set_responses_streaming(true);
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/responses"))
            .bearer_auth("qwitch-bind-codex-desktop")
            .json(&json!({
                "model": model_id,
                "stream": true,
                "input": "read b.txt",
                "tools": [{"type": "function", "name": "read_file", "parameters": {"type": "object", "properties": {}}}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("response.created"), "body: {body}");
        assert!(body.contains("response.output_text.delta"), "body: {body}");
        assert!(body.contains("Hi"), "body: {body}");
        assert!(body.contains("function_call"), "body: {body}");
        assert!(body.contains("read_file"), "body: {body}");
        assert!(body.contains("response.completed"), "body: {body}");
        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn responses_client_streams_from_openai_upstream_with_tools() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"Hey\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_7\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"c.txt\\\"}\"}}]}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":6,\"completion_tokens\":3}}\n\ndata: [DONE]\n\n",
                        "text/event-stream",
                    ),
            )
            .mount(&mock)
            .await;
        let (_db, rt, port, root, model_id) = spawn_gateway_with_custom_account(
            "custom_api_key",
            "deepseek-v4-pro",
            mock.uri(),
            "rsb",
        )
        .await;
        rt.set_responses_streaming(true);
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/responses"))
            .bearer_auth("qwitch-bind-codex-desktop")
            .json(&json!({
                "model": model_id,
                "stream": true,
                "input": "read c.txt",
                "tools": [{"type": "function", "name": "read_file", "parameters": {"type": "object", "properties": {}}}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("response.output_text.delta"), "body: {body}");
        assert!(body.contains("Hey"), "body: {body}");
        assert!(body.contains("function_call"), "body: {body}");
        assert!(body.contains("read_file"), "body: {body}");
        assert!(body.contains("response.completed"), "body: {body}");
        rt.stop_server();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn cross_protocol_parses_crlf_sse_frames() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(
                        "event: message_start\r\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2}}}\r\n\r\nevent: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\r\n\r\nevent: message_delta\r\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\r\n\r\nevent: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n",
                        "text/event-stream",
                    ),
            )
            .mount(&mock)
            .await;
        let (_db, rt, port, root, model_id) = spawn_gateway_with_custom_account(
            "minimax_code_cn",
            "MiniMax-M2.5",
            mock.uri(),
            "crlf",
        )
        .await;
        let resp = reqwest::Client::new()
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .bearer_auth("qwitch-bind-opencode")
            .json(&json!({"model": model_id, "stream": true, "messages": [{"role": "user", "content": "hi"}]}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body = resp.text().await.unwrap();
        assert!(
            body.contains("Hi"),
            "CRLF SSE should be parsed, body: {body}"
        );
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
            // 先读完客户端发来的 HTTP 请求头，避免 Windows (Winsock) 在接收缓冲区仍有未读数据时
            // 关闭连接发送 TCP RST (WSAECONNRESET / 10054) 导致 reqwest 读响应失败。
            let mut req_buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while !req_buf.windows(4).any(|w| w == b"\r\n\r\n") && req_buf.len() < 8192 {
                let n = s.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                req_buf.extend_from_slice(&chunk[..n]);
            }
            let res = forward_codex_own_signin_request(
                &mut s,
                &auth_override,
                &req_headers,
                br#"{"model":"gpt-6-luna","input":[]}"#,
            )
            .await
            .expect("forward");
            assert_eq!(res.0, 200);
            let _ = s.shutdown().await;
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

    #[test]
    fn utf8_sse_decoder_preserves_split_multibyte_chars_and_crlf() {
        let sample = "data: {\"text\":\"你好世界🚀\"}\r\n\r\ndata: [DONE]\r\n\r\n";
        let raw = sample.as_bytes();

        // 1-byte chunks split every 3-byte CJK char, 4-byte emoji, and \r\n pair.
        let mut decoder1 = Utf8SseDecoder::default();
        let mut out1 = String::new();
        for b in raw.chunks(1) {
            decoder1.push_chunk(b, &mut out1);
        }
        decoder1.finish(&mut out1);
        assert!(!out1.contains('\u{FFFD}'));
        assert_eq!(out1, sample.replace("\r\n", "\n"));

        // 2-byte chunks also split across UTF-8 & CRLF boundaries.
        let mut decoder2 = Utf8SseDecoder::default();
        let mut out2 = String::new();
        for b in raw.chunks(2) {
            decoder2.push_chunk(b, &mut out2);
        }
        decoder2.finish(&mut out2);
        assert!(!out2.contains('\u{FFFD}'));
        assert_eq!(out2, sample.replace("\r\n", "\n"));
    }

    #[test]
    fn is_auth_expired_response_recognizes_codearts_apig_0602() {
        assert!(is_auth_expired_response(
            400,
            r#"{"error_msg":"Bad request: the security token has expired","error_code":"APIG.0602","request_id":"abc"}"#,
        ));
    }

    #[test]
    fn is_concurrency_limit_response_recognizes_codearts_tm_00001041() {
        assert!(is_concurrency_limit_response(
            400,
            r#"{"error_code":"TM.00001041","error_msg":"并发会话数已达上限(3个)，请关闭部分会话后重试。"}"#,
        ));
    }
}
