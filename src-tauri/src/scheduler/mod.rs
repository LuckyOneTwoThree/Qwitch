use chrono::Utc;
use rand::Rng;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::adapter::ToolAdapter;
use crate::domain::{AccountStatus, ClaimStatus, UnifiedErrorCode};
use crate::storage::QwitchDatabase;

#[derive(Debug, Clone)]
pub struct JitterConfig {
    pub min_ms: u64,
    pub max_ms: u64,
}

impl Default for JitterConfig {
    fn default() -> Self {
        Self {
            min_ms: 1500,
            max_ms: 3500,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchProgressEvent {
    pub action: String, // "claim" | "refresh"
    pub running: bool,
    pub total: usize,
    pub completed: usize,
    pub current_label: Option<String>,
    pub phase: String, // "checking" | "claiming" | "jitter_wait" | "done"
    pub jitter_seconds: Option<f64>,
    pub summary_text: Option<String>,
}

pub trait SchedulerEventSink: Send + Sync {
    fn emit_batch_progress(&self, event: BatchProgressEvent);
    fn emit_account_updated(&self, account_id: &str);
    fn emit_logs_updated(&self);
}

impl SchedulerEventSink for AppHandle {
    fn emit_batch_progress(&self, event: BatchProgressEvent) {
        let _ = self.emit("qwitch://batch-progress", event);
    }

    fn emit_account_updated(&self, account_id: &str) {
        let _ = self.emit("qwitch://account-updated", account_id);
    }

    fn emit_logs_updated(&self) {
        let _ = self.emit("qwitch://logs-updated", ());
    }
}

pub struct AntiRiskScheduler {
    queue_lock: Arc<Mutex<()>>,
}

fn device_claim_marker_key(tool_id: &str, device_key: &str) -> String {
    format!("daily_claim_device_limit:{tool_id}:{device_key}")
}

fn device_claim_limit_used_today(db: &QwitchDatabase, marker_key: &str) -> bool {
    db.get_setting(marker_key)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<String>(&json).ok())
        .is_some_and(|claimed_at| crate::storage::is_today_cst(&claimed_at))
}

fn remember_device_claim_limit(db: &QwitchDatabase, marker_key: &str) {
    if let Ok(value_json) = serde_json::to_string(&Utc::now().to_rfc3339()) {
        let _ = db.set_setting(marker_key, &value_json);
    }
}

impl AntiRiskScheduler {
    pub fn new() -> Self {
        Self {
            queue_lock: Arc::new(Mutex::new(())),
        }
    }

    /// 单队列串行锁。批量调用方必须先 try_lock_owned 成功（重入即拒绝），
    /// 单账号请求用 lock_owned 排队等待，保证所有平台请求严格串行。
    pub fn queue_lock(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.queue_lock)
    }

    /// 调用方必须已持有 queue_lock（见 queue_lock()）
    pub async fn run_daily_claim_batch(
        &self,
        app: &AppHandle,
        db: &Arc<QwitchDatabase>,
        master_key: &[u8; 32],
        adapters: &HashMap<String, Arc<dyn ToolAdapter>>,
        account_ids: Vec<String>,
        jitter: JitterConfig,
    ) -> Result<(), String> {
        self.run_daily_claim_batch_with_sink(
            app,
            db,
            master_key,
            adapters,
            account_ids,
            jitter,
            (1200, 2600),
        )
        .await
    }

    /// 可注入事件接收端的每日领取批量引擎，供集成与行为回归测试调用
    #[allow(clippy::too_many_arguments)]
    pub async fn run_daily_claim_batch_with_sink<S: SchedulerEventSink + ?Sized>(
        &self,
        sink: &S,
        db: &Arc<QwitchDatabase>,
        master_key: &[u8; 32],
        adapters: &HashMap<String, Arc<dyn ToolAdapter>>,
        account_ids: Vec<String>,
        jitter: JitterConfig,
        cross_device_cooldown_ms: (u64, u64),
    ) -> Result<(), String> {
        let total = account_ids.len();
        if total == 0 {
            return Ok(());
        }

        sink.emit_batch_progress(BatchProgressEvent {
            action: "claim".into(),
            running: true,
            total,
            completed: 0,
            current_label: None,
            phase: "checking".into(),
            jitter_seconds: None,
            summary_text: Some("正在初始化批量队列…".into()),
        });

        let mut claimed_count: usize = 0;
        let mut skipped_count: usize = 0;
        let mut failed_count: usize = 0;
        let mut device_limit_skipped_count: usize = 0;
        // 跨设备节流：记录上一个处理过的设备作用域，切换设备身份时额外冷却
        let mut last_device_scope: Option<String> = None;
        // 工具级熔断：同一工具出现风控/限流后，中止该工具剩余账号
        let mut risk_broken_tools: HashMap<String, ()> = HashMap::new();
        // 设备级每日额度：收到成功回包或 9095 后，阻止本机对同设备继续发领取请求。
        let mut device_limit_reached: HashMap<String, ()> = HashMap::new();

        for (idx, account_id) in account_ids.iter().enumerate() {
            let (summary, cred) = match db.get_account_credential(master_key, account_id) {
                Ok(pair) => pair,
                Err(e) => {
                    failed_count += 1;
                    let (tid, acc_label) = match db.get_account_summary(account_id) {
                        Ok(s) => (s.tool_id, Some(s.label)),
                        Err(_) => ("unknown".to_string(), None),
                    };
                    let msg = match acc_label {
                        Some(lbl) => format!("{lbl} 读取账号凭据失败: {e}"),
                        None => format!("读取账号凭据失败: {e}"),
                    };
                    let _ = db.append_action_log(
                        Some(account_id),
                        &tid,
                        "claim",
                        "failed",
                        Some(&UnifiedErrorCode::InternalError),
                        &msg,
                    );
                    sink.emit_account_updated(account_id);
                    continue;
                }
            };

            if summary.status != AccountStatus::Active {
                skipped_count += 1;
                continue;
            }

            if !summary.has_api_token {
                skipped_count += 1;
                let _ = db.append_action_log(
                    Some(&summary.id),
                    &summary.tool_id,
                    "claim",
                    "skipped",
                    None,
                    &format!("{} 尚未完成在线授权，已自动跳过签到", summary.label),
                );
                sink.emit_account_updated(&summary.id);
                continue;
            }

            let Some(adapter) = adapters.get(&summary.tool_id) else {
                skipped_count += 1;
                continue;
            };

            if !adapter.manifest().capabilities.supports_daily_claim {
                skipped_count += 1;
                continue;
            }

            if risk_broken_tools.contains_key(&summary.tool_id) {
                skipped_count += 1;
                let _ = db.append_action_log(
                    Some(&summary.id),
                    &summary.tool_id,
                    "claim",
                    "skipped",
                    None,
                    &format!("{} 所属工具已触发风控熔断，本批次自动跳过", summary.label),
                );
                continue;
            }

            // 请求前按需刷新 OAuth 凭据（如 MiniMax accessToken），避免过期后整批 401
            let cred = match adapter.refresh_credential(&cred).await {
                Ok(Some(updated)) => {
                    if let Err(e) =
                        db.bind_account_credential(master_key, &summary.id, None, &updated)
                    {
                        let _ = db.append_action_log(
                            Some(&summary.id),
                            &summary.tool_id,
                            "claim",
                            "failed",
                            Some(&UnifiedErrorCode::InternalError),
                            &format!(
                                "{} 刷新后的凭据落盘失败（后续操作仍将继续）：{e}",
                                summary.label
                            ),
                        );
                    }
                    updated
                }
                Ok(None) => cred,
                Err(err) => {
                    failed_count += 1;
                    let code = handle_adapter_failure(
                        db,
                        &summary.id,
                        &summary.tool_id,
                        &summary.label,
                        "claim",
                        err,
                    );
                    if matches!(
                        code,
                        UnifiedErrorCode::RiskTriggered | UnifiedErrorCode::RateLimited
                    ) {
                        risk_broken_tools.insert(summary.tool_id.clone(), ());
                    }
                    sink.emit_account_updated(&summary.id);
                    continue;
                }
            };

            let device_key = adapter.daily_claim_device_key_for(&cred);
            let device_scope = device_key
                .as_deref()
                .map(|key| format!("{}:{key}", summary.tool_id));
            let device_marker_key = device_key
                .as_deref()
                .map(|key| device_claim_marker_key(&summary.tool_id, key));
            let batch_device_limit = device_scope
                .as_deref()
                .is_some_and(|scope| device_limit_reached.contains_key(scope));
            let persisted_device_limit = !batch_device_limit
                && device_marker_key
                    .as_deref()
                    .is_some_and(|key| device_claim_limit_used_today(db, key));
            if persisted_device_limit || batch_device_limit {
                if let Some(scope) = device_scope.as_ref() {
                    device_limit_reached.insert(scope.clone(), ());
                }
                skipped_count += 1;
                device_limit_skipped_count += 1;
                sink.emit_account_updated(&summary.id);
                continue;
            }

            // 同出口 IP 多设备节流：切换到不同设备身份时额外冷却，避免短时间内
            // 以多个 device_id 集中领取而被关联风控。
            if let (Some(prev), Some(cur)) = (last_device_scope.as_deref(), device_scope.as_deref())
            {
                if prev != cur {
                    let min_c = cross_device_cooldown_ms.0.min(cross_device_cooldown_ms.1);
                    let max_c = cross_device_cooldown_ms.1.max(min_c);
                    let extra_ms = if max_c == 0 {
                        0
                    } else {
                        rand::thread_rng().gen_range(min_c..=max_c)
                    };
                    sink.emit_batch_progress(BatchProgressEvent {
                        action: "claim".into(),
                        running: true,
                        total,
                        completed: idx,
                        current_label: Some(summary.label.clone()),
                        phase: "jitter_wait".into(),
                        jitter_seconds: Some((extra_ms as f64) / 1000.0),
                        summary_text: Some("切换设备身份，冷却中…".into()),
                    });
                    if extra_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(extra_ms)).await;
                    }
                }
            }
            if let Some(cur) = device_scope.as_ref() {
                last_device_scope = Some(cur.clone());
            }

            sink.emit_batch_progress(BatchProgressEvent {
                action: "claim".into(),
                running: true,
                total,
                completed: idx,
                current_label: Some(summary.label.clone()),
                phase: "checking".into(),
                jitter_seconds: None,
                summary_text: None,
            });

            // 1. 幂等前置校验：先查状态再决定是否领取（Fail-Closed）
            let quota = match adapter.inspect_quota(&cred).await {
                Ok(q) => {
                    let _ = db.save_quota_snapshot(&summary.id, &q);
                    q
                }
                Err(err) => {
                    failed_count += 1;
                    let code = handle_adapter_failure(
                        db,
                        &summary.id,
                        &summary.tool_id,
                        &summary.label,
                        "claim",
                        err,
                    );
                    if matches!(
                        code,
                        UnifiedErrorCode::RiskTriggered | UnifiedErrorCode::RateLimited
                    ) {
                        risk_broken_tools.insert(summary.tool_id.clone(), ());
                    }
                    sink.emit_account_updated(&summary.id);
                    continue;
                }
            };

            match quota.claim_status {
                ClaimStatus::Claimed => {
                    skipped_count += 1;
                    let _ = db.append_action_log(
                        Some(&summary.id),
                        &summary.tool_id,
                        "claim",
                        "skipped",
                        None,
                        &format!("{} 今日已领取，幂等校验自动跳过", summary.label),
                    );
                }
                ClaimStatus::Unsupported => {
                    skipped_count += 1;
                    let _ = db.append_action_log(
                        Some(&summary.id),
                        &summary.tool_id,
                        "claim",
                        "skipped",
                        None,
                        &format!("{} 当前无可用签到活动，已自动跳过", summary.label),
                    );
                }
                ClaimStatus::Unknown => {
                    skipped_count += 1;
                    let _ = db.append_action_log(
                        Some(&summary.id),
                        &summary.tool_id,
                        "claim",
                        "skipped",
                        None,
                        &format!("{} 签到状态未知，为防重复请求已跳过", summary.label),
                    );
                }
                ClaimStatus::Available => {
                    sink.emit_batch_progress(BatchProgressEvent {
                        action: "claim".into(),
                        running: true,
                        total,
                        completed: idx,
                        current_label: Some(summary.label.clone()),
                        phase: "claiming".into(),
                        jitter_seconds: None,
                        summary_text: None,
                    });

                    // 2. 执行手动领取（复用前置快照，避免适配器重复查状态）
                    match adapter.claim_daily(&cred, Some(&quota)).await {
                        Ok(res) => {
                            let is_confirmed_claimed = res
                                .updated_quota
                                .as_ref()
                                .is_some_and(|q| q.claim_status == ClaimStatus::Claimed);
                            if let Some(updated_q) = res.updated_quota {
                                let _ = db.save_quota_snapshot(&summary.id, &updated_q);
                            }
                            if res.claimed {
                                claimed_count += 1;
                                if let (Some(scope), Some(marker_key)) =
                                    (device_scope.as_ref(), device_marker_key.as_deref())
                                {
                                    device_limit_reached.insert(scope.clone(), ());
                                    remember_device_claim_limit(db, marker_key);
                                }
                                let _ =
                                    db.mark_account_claimed(&summary.id, &Utc::now().to_rfc3339());
                                let _ = db.append_action_log(
                                    Some(&summary.id),
                                    &summary.tool_id,
                                    "claim",
                                    "success",
                                    None,
                                    &format!(
                                        "{} ({}) {}",
                                        summary.label, summary.uid_masked, res.reward_summary
                                    ),
                                );
                            } else {
                                skipped_count += 1;
                                if is_confirmed_claimed
                                    && !summary
                                        .last_claimed_at
                                        .as_deref()
                                        .is_some_and(crate::storage::is_today_cst)
                                {
                                    let _ = db.mark_account_claimed(
                                        &summary.id,
                                        &Utc::now().to_rfc3339(),
                                    );
                                }
                                let _ = db.append_action_log(
                                    Some(&summary.id),
                                    &summary.tool_id,
                                    "claim",
                                    "skipped",
                                    None,
                                    &format!("{} {}", summary.label, res.reward_summary),
                                );
                            }
                        }
                        Err(crate::adapter::AdapterError::Unified {
                            code: UnifiedErrorCode::DeviceDailyLimitReached,
                            message,
                        }) => {
                            skipped_count += 1;
                            device_limit_skipped_count += 1;
                            if let (Some(scope), Some(marker_key)) =
                                (device_scope.as_ref(), device_marker_key.as_deref())
                            {
                                device_limit_reached.insert(scope.clone(), ());
                                remember_device_claim_limit(db, marker_key);
                            }
                            let _ = db.append_action_log(
                                Some(&summary.id),
                                &summary.tool_id,
                                "claim",
                                "skipped",
                                Some(&UnifiedErrorCode::DeviceDailyLimitReached),
                                &format!("{}：{message}", summary.label),
                            );
                        }
                        Err(err) => {
                            failed_count += 1;
                            let code = handle_adapter_failure(
                                db,
                                &summary.id,
                                &summary.tool_id,
                                &summary.label,
                                "claim",
                                err,
                            );
                            if matches!(
                                code,
                                UnifiedErrorCode::RiskTriggered | UnifiedErrorCode::RateLimited
                            ) {
                                risk_broken_tools.insert(summary.tool_id.clone(), ());
                            }
                        }
                    }
                }
            }

            sink.emit_account_updated(&summary.id);

            // 3. 账号间随机抖动延迟（Jitter），避免突发流量
            if idx + 1 < total {
                let min = jitter.min_ms.min(jitter.max_ms);
                let max = jitter.max_ms.max(min);
                let wait_ms = if max == 0 {
                    0
                } else {
                    rand::thread_rng().gen_range(min..=max)
                };
                if wait_ms > 0 {
                    sink.emit_batch_progress(BatchProgressEvent {
                        action: "claim".into(),
                        running: true,
                        total,
                        completed: idx + 1,
                        current_label: None,
                        phase: "jitter_wait".into(),
                        jitter_seconds: Some((wait_ms as f64) / 1000.0),
                        summary_text: None,
                    });
                    tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                }
            }
        }

        let breaker_note = if risk_broken_tools.is_empty() {
            String::new()
        } else {
            format!("，{} 个工具因风控熔断中止", risk_broken_tools.len())
        };
        let device_limit_note = if device_limit_skipped_count == 0 {
            String::new()
        } else {
            format!("，设备日额度限制跳过 {device_limit_skipped_count} 个账号")
        };
        let summary_text = format!(
            "串行领取完成：成功 {claimed_count} 个，跳过 {skipped_count} 个，异常 {failed_count} 个{breaker_note}{device_limit_note}"
        );
        sink.emit_batch_progress(BatchProgressEvent {
            action: "claim".into(),
            running: false,
            total,
            completed: total,
            current_label: None,
            phase: "done".into(),
            jitter_seconds: None,
            summary_text: Some(summary_text),
        });
        sink.emit_logs_updated();

        Ok(())
    }

    /// 调用方必须已持有 queue_lock（见 queue_lock()）
    pub async fn run_quota_refresh_batch(
        &self,
        app: &AppHandle,
        db: &Arc<QwitchDatabase>,
        master_key: &[u8; 32],
        adapters: &HashMap<String, Arc<dyn ToolAdapter>>,
        account_ids: Vec<String>,
        jitter: JitterConfig,
    ) -> Result<(), String> {
        self.run_quota_refresh_batch_with_sink(app, db, master_key, adapters, account_ids, jitter)
            .await
    }

    /// 可注入事件接收端的批量查额引擎，供集成与行为回归测试调用
    pub async fn run_quota_refresh_batch_with_sink<S: SchedulerEventSink + ?Sized>(
        &self,
        sink: &S,
        db: &Arc<QwitchDatabase>,
        master_key: &[u8; 32],
        adapters: &HashMap<String, Arc<dyn ToolAdapter>>,
        account_ids: Vec<String>,
        jitter: JitterConfig,
    ) -> Result<(), String> {
        let total = account_ids.len();
        if total == 0 {
            return Ok(());
        }

        sink.emit_batch_progress(BatchProgressEvent {
            action: "refresh".into(),
            running: true,
            total,
            completed: 0,
            current_label: None,
            phase: "checking".into(),
            jitter_seconds: None,
            summary_text: Some("正在初始化批量队列…".into()),
        });

        let mut refreshed_count: usize = 0;
        let mut skipped_count: usize = 0;
        let mut failed_count: usize = 0;
        let mut risk_broken_tools: HashMap<String, ()> = HashMap::new();

        for (idx, account_id) in account_ids.iter().enumerate() {
            let (summary, cred) = match db.get_account_credential(master_key, account_id) {
                Ok(pair) => pair,
                Err(e) => {
                    failed_count += 1;
                    let (tid, acc_label) = match db.get_account_summary(account_id) {
                        Ok(s) => (s.tool_id, Some(s.label)),
                        Err(_) => ("unknown".to_string(), None),
                    };
                    let msg = match acc_label {
                        Some(lbl) => format!("{lbl} 读取账号凭据失败: {e}"),
                        None => format!("读取账号凭据失败: {e}"),
                    };
                    let _ = db.append_action_log(
                        Some(account_id),
                        &tid,
                        "query",
                        "failed",
                        Some(&UnifiedErrorCode::InternalError),
                        &msg,
                    );
                    sink.emit_account_updated(account_id);
                    continue;
                }
            };

            if summary.status != AccountStatus::Active || !summary.has_api_token {
                skipped_count += 1;
                continue;
            }

            let Some(adapter) = adapters.get(&summary.tool_id) else {
                skipped_count += 1;
                continue;
            };

            if !adapter.manifest().capabilities.supports_quota_query {
                skipped_count += 1;
                continue;
            }

            if risk_broken_tools.contains_key(&summary.tool_id) {
                skipped_count += 1;
                let _ = db.append_action_log(
                    Some(&summary.id),
                    &summary.tool_id,
                    "query",
                    "skipped",
                    None,
                    &format!("{} 所属工具已触发风控熔断，本批次自动跳过", summary.label),
                );
                continue;
            }

            // 请求前按需刷新 OAuth 凭据（如 MiniMax accessToken）
            let cred = match adapter.refresh_credential(&cred).await {
                Ok(Some(updated)) => {
                    if let Err(e) =
                        db.bind_account_credential(master_key, &summary.id, None, &updated)
                    {
                        let _ = db.append_action_log(
                            Some(&summary.id),
                            &summary.tool_id,
                            "query",
                            "failed",
                            Some(&UnifiedErrorCode::InternalError),
                            &format!(
                                "{} 刷新后的凭据落盘失败（后续操作仍将继续）：{e}",
                                summary.label
                            ),
                        );
                    }
                    updated
                }
                Ok(None) => cred,
                Err(err) => {
                    failed_count += 1;
                    let code = handle_adapter_failure(
                        db,
                        &summary.id,
                        &summary.tool_id,
                        &summary.label,
                        "query",
                        err,
                    );
                    if matches!(
                        code,
                        UnifiedErrorCode::RiskTriggered | UnifiedErrorCode::RateLimited
                    ) {
                        risk_broken_tools.insert(summary.tool_id.clone(), ());
                    }
                    sink.emit_account_updated(&summary.id);
                    continue;
                }
            };

            sink.emit_batch_progress(BatchProgressEvent {
                action: "refresh".into(),
                running: true,
                total,
                completed: idx,
                current_label: Some(summary.label.clone()),
                phase: "checking".into(),
                jitter_seconds: None,
                summary_text: None,
            });

            match adapter.inspect_quota(&cred).await {
                Ok(snapshot) => {
                    refreshed_count += 1;
                    let _ = db.save_quota_snapshot(&summary.id, &snapshot);
                    let _ = db.append_action_log(
                        Some(&summary.id),
                        &summary.tool_id,
                        "query",
                        "success",
                        None,
                        &format!(
                            "已刷新 {} ({}) 的最新额度",
                            summary.label, summary.uid_masked
                        ),
                    );
                }
                Err(err) => {
                    failed_count += 1;
                    let code = handle_adapter_failure(
                        db,
                        &summary.id,
                        &summary.tool_id,
                        &summary.label,
                        "query",
                        err,
                    );
                    if matches!(
                        code,
                        UnifiedErrorCode::RiskTriggered | UnifiedErrorCode::RateLimited
                    ) {
                        risk_broken_tools.insert(summary.tool_id.clone(), ());
                    }
                }
            }

            sink.emit_account_updated(&summary.id);

            if idx + 1 < total {
                let min = jitter.min_ms.min(jitter.max_ms);
                let max = jitter.max_ms.max(min);
                let wait_ms = if max == 0 {
                    0
                } else {
                    rand::thread_rng().gen_range(min..=max)
                };
                if wait_ms > 0 {
                    sink.emit_batch_progress(BatchProgressEvent {
                        action: "refresh".into(),
                        running: true,
                        total,
                        completed: idx + 1,
                        current_label: None,
                        phase: "jitter_wait".into(),
                        jitter_seconds: Some((wait_ms as f64) / 1000.0),
                        summary_text: None,
                    });
                    tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                }
            }
        }

        let breaker_note = if risk_broken_tools.is_empty() {
            String::new()
        } else {
            format!("，{} 个工具因风控熔断中止", risk_broken_tools.len())
        };
        let summary_text = format!(
            "额度刷新完成：成功 {refreshed_count} 个，跳过 {skipped_count} 个，异常 {failed_count} 个{breaker_note}"
        );
        sink.emit_batch_progress(BatchProgressEvent {
            action: "refresh".into(),
            running: false,
            total,
            completed: total,
            current_label: None,
            phase: "done".into(),
            jitter_seconds: None,
            summary_text: Some(summary_text),
        });
        sink.emit_logs_updated();

        Ok(())
    }
}

impl Default for AntiRiskScheduler {
    fn default() -> Self {
        Self::new()
    }
}

fn handle_adapter_failure(
    db: &Arc<QwitchDatabase>,
    account_id: &str,
    tool_id: &str,
    label: &str,
    action_type: &str,
    err: crate::adapter::AdapterError,
) -> UnifiedErrorCode {
    let crate::adapter::AdapterError::Unified { code, message } = err;
    match code {
        UnifiedErrorCode::AuthExpired => {
            let _ = db.update_account_status(account_id, AccountStatus::Expired);
        }
        UnifiedErrorCode::RiskTriggered | UnifiedErrorCode::RateLimited => {
            let _ = db.update_account_status(account_id, AccountStatus::RiskPaused);
        }
        _ => {}
    }
    let _ = db.append_action_log(
        Some(account_id),
        tool_id,
        action_type,
        "failed",
        Some(&code),
        &format!("{label} 操作中止：{message}"),
    );
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{AdapterCapabilities, AdapterError, AdapterManifest, ClaimResult};
    use crate::domain::{AuthMethod, CredentialEnvelope, CredentialPayload, QuotaSnapshot};
    use crate::platform::PlatformHost;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct MockEventSink {
        progress: StdMutex<Vec<BatchProgressEvent>>,
        updated: StdMutex<Vec<String>>,
        logs_updates: AtomicUsize,
    }

    impl SchedulerEventSink for MockEventSink {
        fn emit_batch_progress(&self, event: BatchProgressEvent) {
            self.progress.lock().unwrap().push(event);
        }

        fn emit_account_updated(&self, account_id: &str) {
            self.updated.lock().unwrap().push(account_id.to_string());
        }

        fn emit_logs_updated(&self) {
            self.logs_updates.fetch_add(1, Ordering::SeqCst);
        }
    }

    type InspectFn =
        Box<dyn FnMut(&CredentialEnvelope) -> Result<QuotaSnapshot, AdapterError> + Send>;
    type ClaimFn = Box<
        dyn FnMut(&CredentialEnvelope, Option<&QuotaSnapshot>) -> Result<ClaimResult, AdapterError>
            + Send,
    >;
    type RefreshFn = Box<
        dyn FnMut(&CredentialEnvelope) -> Result<Option<CredentialEnvelope>, AdapterError> + Send,
    >;
    type DeviceKeyFn = Box<dyn Fn(&CredentialEnvelope) -> Option<String> + Send + Sync>;

    struct MockAdapter {
        manifest: AdapterManifest,
        inspect_fn: StdMutex<InspectFn>,
        claim_fn: StdMutex<ClaimFn>,
        refresh_fn: StdMutex<RefreshFn>,
        device_key_fn: DeviceKeyFn,
    }

    impl MockAdapter {
        fn new(tool_id: &str) -> Self {
            Self {
                manifest: AdapterManifest {
                    id: tool_id.to_string(),
                    name: tool_id.to_string(),
                    version: "0.1.0".to_string(),
                    is_builtin: true,
                    capabilities: AdapterCapabilities {
                        auth_methods: vec![AuthMethod::TokenPaste],
                        supports_quota_query: true,
                        supports_daily_claim: true,
                        supports_client_switch: false,
                        supports_multi_instance: false,
                    },
                },
                inspect_fn: StdMutex::new(Box::new(|_| Ok(snapshot(ClaimStatus::Available)))),
                claim_fn: StdMutex::new(Box::new(|_, _| {
                    Ok(ClaimResult {
                        claimed: true,
                        reward_summary: "+100 Credits".into(),
                        updated_quota: Some(snapshot(ClaimStatus::Claimed)),
                    })
                })),
                refresh_fn: StdMutex::new(Box::new(|_| Ok(None))),
                device_key_fn: Box::new(|_| None),
            }
        }
    }

    #[async_trait]
    impl ToolAdapter for MockAdapter {
        fn manifest(&self) -> &AdapterManifest {
            &self.manifest
        }

        fn daily_claim_device_key_for(&self, cred: &CredentialEnvelope) -> Option<String> {
            (self.device_key_fn)(cred)
        }

        async fn import_from_local(
            &self,
            _host: &dyn PlatformHost,
        ) -> Result<CredentialEnvelope, AdapterError> {
            unreachable!()
        }

        async fn parse_manual_token(
            &self,
            _raw_input: &str,
        ) -> Result<CredentialEnvelope, AdapterError> {
            unreachable!()
        }

        async fn inspect_quota(
            &self,
            cred: &CredentialEnvelope,
        ) -> Result<QuotaSnapshot, AdapterError> {
            (self.inspect_fn.lock().unwrap())(cred)
        }

        async fn claim_daily(
            &self,
            cred: &CredentialEnvelope,
            precheck: Option<&QuotaSnapshot>,
        ) -> Result<ClaimResult, AdapterError> {
            (self.claim_fn.lock().unwrap())(cred, precheck)
        }

        async fn refresh_credential(
            &self,
            cred: &CredentialEnvelope,
        ) -> Result<Option<CredentialEnvelope>, AdapterError> {
            (self.refresh_fn.lock().unwrap())(cred)
        }

        async fn apply_session(
            &self,
            _host: &dyn PlatformHost,
            _cred: &CredentialEnvelope,
        ) -> Result<(), AdapterError> {
            unreachable!()
        }
    }

    fn snapshot(status: ClaimStatus) -> QuotaSnapshot {
        QuotaSnapshot {
            plan_name: Some("Free".into()),
            claim_status: status,
            buckets: vec![],
            fetched_at: Utc::now().to_rfc3339(),
        }
    }

    fn token_env(tool_id: &str, uid: &str, tok: &str) -> CredentialEnvelope {
        CredentialEnvelope {
            tool_id: tool_id.to_string(),
            uid: uid.to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: tok.to_string(),
                refresh_token: None,
                expires_at: None,
                extra: Default::default(),
            },
        }
    }

    fn local_only_env(tool_id: &str, uid: &str) -> CredentialEnvelope {
        let mut files = HashMap::new();
        files.insert("storage.json".to_string(), "e30=".to_string());
        CredentialEnvelope {
            tool_id: tool_id.to_string(),
            uid: uid.to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::LocalSnapshot { files },
        }
    }

    fn open_test_db() -> (std::path::PathBuf, Arc<QwitchDatabase>, [u8; 32]) {
        let db_path =
            std::env::temp_dir().join(format!("qwitch_sched_{}.db", uuid::Uuid::new_v4()));
        let db = Arc::new(QwitchDatabase::open(&db_path).expect("open db"));
        (db_path, db, [9u8; 32])
    }

    #[tokio::test]
    async fn claim_batch_happy_path_idempotent_skip_and_fail_closed() {
        let (db_path, db, key) = open_test_db();
        let a_avail = db
            .upsert_account(&key, Some("avail"), &token_env("qoder_cn", "avail", "t1"))
            .unwrap()
            .id;
        let a_claimed = db
            .upsert_account(
                &key,
                Some("claimed"),
                &token_env("qoder_cn", "claimed", "t2"),
            )
            .unwrap()
            .id;
        let a_unknown = db
            .upsert_account(
                &key,
                Some("unknown"),
                &token_env("qoder_cn", "unknown", "t3"),
            )
            .unwrap()
            .id;
        let a_unsup = db
            .upsert_account(&key, Some("unsup"), &token_env("qoder_cn", "unsup", "t4"))
            .unwrap()
            .id;
        let a_disabled = db
            .upsert_account(&key, Some("disabled"), &token_env("qoder_cn", "dis", "t5"))
            .unwrap()
            .id;
        db.update_account_status(&a_disabled, AccountStatus::Disabled)
            .unwrap();
        let a_notoken = db
            .upsert_account(&key, Some("notok"), &local_only_env("qoder_cn", "notok"))
            .unwrap()
            .id;

        let claim_calls = Arc::new(StdMutex::new(Vec::<String>::new()));
        let claim_calls_inner = Arc::clone(&claim_calls);

        let mut adapter = MockAdapter::new("qoder_cn");
        adapter.inspect_fn = StdMutex::new(Box::new(|cred| {
            Ok(match cred.uid.as_str() {
                "avail" => snapshot(ClaimStatus::Available),
                "claimed" => snapshot(ClaimStatus::Claimed),
                "unknown" => snapshot(ClaimStatus::Unknown),
                "unsup" => snapshot(ClaimStatus::Unsupported),
                other => panic!("unexpected inspect for {other}"),
            })
        }));
        adapter.claim_fn = StdMutex::new(Box::new(move |cred, precheck| {
            assert_eq!(
                precheck.map(|q| &q.claim_status),
                Some(&ClaimStatus::Available)
            );
            claim_calls_inner.lock().unwrap().push(cred.uid.clone());
            Ok(ClaimResult {
                claimed: true,
                reward_summary: "成功领取 +100".into(),
                updated_quota: Some(snapshot(ClaimStatus::Claimed)),
            })
        }));

        let mut adapters: HashMap<String, Arc<dyn ToolAdapter>> = HashMap::new();
        adapters.insert("qoder_cn".to_string(), Arc::new(adapter));

        let scheduler = AntiRiskScheduler::new();
        let sink = MockEventSink::default();
        scheduler
            .run_daily_claim_batch_with_sink(
                &sink,
                &db,
                &key,
                &adapters,
                vec![
                    a_avail.clone(),
                    a_claimed.clone(),
                    a_unknown.clone(),
                    a_unsup.clone(),
                    a_disabled.clone(),
                    a_notoken.clone(),
                ],
                JitterConfig {
                    min_ms: 0,
                    max_ms: 0,
                },
                (0, 0),
            )
            .await
            .unwrap();

        assert_eq!(*claim_calls.lock().unwrap(), vec!["avail".to_string()]);
        let avail_sum = db.get_account_summary(&a_avail).unwrap();
        assert!(avail_sum.last_claimed_at.is_some());
        assert_eq!(
            avail_sum.latest_quota.as_ref().map(|q| &q.claim_status),
            Some(&ClaimStatus::Claimed)
        );
        let events = sink.progress.lock().unwrap();
        assert!(events.iter().any(|e| e.phase == "claiming"));
        let done = events.last().expect("done event");
        assert!(!done.running);
        assert_eq!(done.phase, "done");
        assert!(done
            .summary_text
            .as_deref()
            .unwrap()
            .contains("成功 1 个，跳过 5 个，异常 0 个"));
        assert!(sink.logs_updates.load(Ordering::SeqCst) >= 1);

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn claim_batch_circuit_breaker_isolates_tool_and_keeps_auth_expired_scoped() {
        let (db_path, db, key) = open_test_db();
        let a1 = db
            .upsert_account(&key, Some("a1"), &token_env("tool_a", "a1", "t1"))
            .unwrap()
            .id;
        let a2 = db
            .upsert_account(&key, Some("a2"), &token_env("tool_a", "a2", "t2"))
            .unwrap()
            .id;
        let b1 = db
            .upsert_account(&key, Some("b1"), &token_env("tool_b", "b1", "t3"))
            .unwrap()
            .id;

        let a_inspected = Arc::new(StdMutex::new(Vec::<String>::new()));
        let a_inspected_inner = Arc::clone(&a_inspected);
        let mut adapter_a = MockAdapter::new("tool_a");
        adapter_a.inspect_fn = StdMutex::new(Box::new(move |cred| {
            a_inspected_inner.lock().unwrap().push(cred.uid.clone());
            Err(AdapterError::Unified {
                code: UnifiedErrorCode::RiskTriggered,
                message: "403 challenge".into(),
            })
        }));

        let adapter_b = MockAdapter::new("tool_b");
        let mut adapters: HashMap<String, Arc<dyn ToolAdapter>> = HashMap::new();
        adapters.insert("tool_a".to_string(), Arc::new(adapter_a));
        adapters.insert("tool_b".to_string(), Arc::new(adapter_b));

        let scheduler = AntiRiskScheduler::new();
        let sink = MockEventSink::default();
        scheduler
            .run_daily_claim_batch_with_sink(
                &sink,
                &db,
                &key,
                &adapters,
                vec![a1.clone(), a2.clone(), b1.clone()],
                JitterConfig {
                    min_ms: 0,
                    max_ms: 0,
                },
                (0, 0),
            )
            .await
            .unwrap();

        assert_eq!(*a_inspected.lock().unwrap(), vec!["a1".to_string()]);
        assert_eq!(
            db.get_account_summary(&a1).unwrap().status,
            AccountStatus::RiskPaused
        );
        assert_eq!(
            db.get_account_summary(&a2).unwrap().status,
            AccountStatus::Active
        );
        assert!(db
            .get_account_summary(&b1)
            .unwrap()
            .last_claimed_at
            .is_some());

        // 单独验证 401 AuthExpired 仅标记 Expired，不熔断同工具后续账号
        let c1 = db
            .upsert_account(&key, Some("c1"), &token_env("tool_c", "c1", "t4"))
            .unwrap()
            .id;
        let c2 = db
            .upsert_account(&key, Some("c2"), &token_env("tool_c", "c2", "t5"))
            .unwrap()
            .id;
        let mut adapter_c = MockAdapter::new("tool_c");
        adapter_c.inspect_fn = StdMutex::new(Box::new(|cred| {
            if cred.uid == "c1" {
                Err(AdapterError::Unified {
                    code: UnifiedErrorCode::AuthExpired,
                    message: "401 expired".into(),
                })
            } else {
                Ok(snapshot(ClaimStatus::Available))
            }
        }));
        adapters.insert("tool_c".to_string(), Arc::new(adapter_c));
        scheduler
            .run_daily_claim_batch_with_sink(
                &MockEventSink::default(),
                &db,
                &key,
                &adapters,
                vec![c1.clone(), c2.clone()],
                JitterConfig {
                    min_ms: 0,
                    max_ms: 0,
                },
                (0, 0),
            )
            .await
            .unwrap();
        assert_eq!(
            db.get_account_summary(&c1).unwrap().status,
            AccountStatus::Expired
        );
        assert!(db
            .get_account_summary(&c2)
            .unwrap()
            .last_claimed_at
            .is_some());

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn claim_batch_device_limit_blocks_same_device_and_cools_down_across_devices() {
        let (db_path, db, key) = open_test_db();
        let d1_a = db
            .upsert_account(&key, Some("d1_a"), &token_env("tool_dev", "d1_a", "dev-1"))
            .unwrap()
            .id;
        let d1_b = db
            .upsert_account(&key, Some("d1_b"), &token_env("tool_dev", "d1_b", "dev-1"))
            .unwrap()
            .id;
        let d2_a = db
            .upsert_account(&key, Some("d2_a"), &token_env("tool_dev", "d2_a", "dev-2"))
            .unwrap()
            .id;

        let inspected = Arc::new(StdMutex::new(Vec::<String>::new()));
        let inspected_inner = Arc::clone(&inspected);
        let mut adapter = MockAdapter::new("tool_dev");
        adapter.device_key_fn = Box::new(|cred| match &cred.payload {
            CredentialPayload::Token { access_token, .. } => Some(access_token.clone()),
            _ => None,
        });
        adapter.inspect_fn = StdMutex::new(Box::new(move |cred| {
            inspected_inner.lock().unwrap().push(cred.uid.clone());
            Ok(snapshot(ClaimStatus::Available))
        }));

        let mut adapters: HashMap<String, Arc<dyn ToolAdapter>> = HashMap::new();
        adapters.insert("tool_dev".to_string(), Arc::new(adapter));

        let scheduler = AntiRiskScheduler::new();
        let sink = MockEventSink::default();
        scheduler
            .run_daily_claim_batch_with_sink(
                &sink,
                &db,
                &key,
                &adapters,
                vec![d1_a.clone(), d1_b.clone(), d2_a.clone()],
                JitterConfig {
                    min_ms: 0,
                    max_ms: 0,
                },
                (0, 0),
            )
            .await
            .unwrap();

        assert_eq!(
            *inspected.lock().unwrap(),
            vec!["d1_a".to_string(), "d2_a".to_string()]
        );
        assert!(db
            .get_account_summary(&d1_a)
            .unwrap()
            .last_claimed_at
            .is_some());
        assert!(db
            .get_account_summary(&d1_b)
            .unwrap()
            .last_claimed_at
            .is_none());
        assert!(db
            .get_account_summary(&d2_a)
            .unwrap()
            .last_claimed_at
            .is_some());

        let events = sink.progress.lock().unwrap();
        assert!(events.iter().any(|e| {
            e.phase == "jitter_wait"
                && e.summary_text
                    .as_deref()
                    .is_some_and(|s| s.contains("切换设备身份，冷却中"))
        }));

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn refresh_batch_persists_rotated_credential_and_breaks_on_rate_limit() {
        let (db_path, db, key) = open_test_db();
        let a1 = db
            .upsert_account(&key, Some("a1"), &token_env("tool_r", "u1", "old_tok"))
            .unwrap()
            .id;
        let a2 = db
            .upsert_account(&key, Some("a2"), &token_env("tool_r", "u2", "tok_2"))
            .unwrap()
            .id;
        let a3 = db
            .upsert_account(&key, Some("a3"), &token_env("tool_r", "u3", "tok_3"))
            .unwrap()
            .id;

        let seen_tokens = Arc::new(StdMutex::new(Vec::<String>::new()));
        let seen_tokens_inner = Arc::clone(&seen_tokens);

        let mut adapter = MockAdapter::new("tool_r");
        adapter.refresh_fn = StdMutex::new(Box::new(|cred| {
            if cred.uid == "u1" {
                Ok(Some(token_env("tool_r", "u1", "rotated_tok")))
            } else {
                Ok(None)
            }
        }));
        adapter.inspect_fn = StdMutex::new(Box::new(move |cred| {
            let tok = match &cred.payload {
                CredentialPayload::Token { access_token, .. } => access_token.clone(),
                _ => String::new(),
            };
            seen_tokens_inner.lock().unwrap().push(tok);
            if cred.uid == "u2" {
                Err(AdapterError::Unified {
                    code: UnifiedErrorCode::RateLimited,
                    message: "429 too fast".into(),
                })
            } else {
                Ok(snapshot(ClaimStatus::Unsupported))
            }
        }));

        let mut adapters: HashMap<String, Arc<dyn ToolAdapter>> = HashMap::new();
        adapters.insert("tool_r".to_string(), Arc::new(adapter));

        let scheduler = AntiRiskScheduler::new();
        let sink = MockEventSink::default();
        scheduler
            .run_quota_refresh_batch_with_sink(
                &sink,
                &db,
                &key,
                &adapters,
                vec![a1.clone(), a2.clone(), a3.clone()],
                JitterConfig {
                    min_ms: 0,
                    max_ms: 0,
                },
            )
            .await
            .unwrap();

        assert_eq!(
            *seen_tokens.lock().unwrap(),
            vec!["rotated_tok".to_string(), "tok_2".to_string()]
        );
        let (_, saved_a1) = db.get_account_credential(&key, &a1).unwrap();
        match saved_a1.payload {
            CredentialPayload::Token { access_token, .. } => {
                assert_eq!(access_token, "rotated_tok");
            }
            _ => panic!("expected token payload"),
        }
        assert_eq!(
            db.get_account_summary(&a2).unwrap().status,
            AccountStatus::RiskPaused
        );
        assert_eq!(
            db.get_account_summary(&a3).unwrap().status,
            AccountStatus::Active
        );

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn queue_lock_rejects_concurrent_reentry() {
        let scheduler = AntiRiskScheduler::new();
        let guard = scheduler
            .queue_lock()
            .try_lock_owned()
            .expect("first lock succeeds");
        assert!(scheduler.queue_lock().try_lock_owned().is_err());
        drop(guard);
        assert!(scheduler.queue_lock().try_lock_owned().is_ok());
    }
}
