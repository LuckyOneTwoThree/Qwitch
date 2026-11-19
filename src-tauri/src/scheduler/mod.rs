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
        let total = account_ids.len();
        if total == 0 {
            return Ok(());
        }

        let mut claimed_count: usize = 0;
        let mut skipped_count: usize = 0;
        let mut failed_count: usize = 0;
        let mut device_limit_skipped_count: usize = 0;
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
                    let _ = app.emit("qwitch://account-updated", account_id);
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
                let _ = app.emit("qwitch://account-updated", &summary.id);
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

            let device_key = adapter.daily_claim_device_key();
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
                let _ = app.emit("qwitch://account-updated", &summary.id);
                continue;
            }

            let _ = app.emit(
                "qwitch://batch-progress",
                BatchProgressEvent {
                    action: "claim".into(),
                    running: true,
                    total,
                    completed: idx,
                    current_label: Some(summary.label.clone()),
                    phase: "checking".into(),
                    jitter_seconds: None,
                    summary_text: None,
                },
            );

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
                    let _ = app.emit("qwitch://account-updated", &summary.id);
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
                    let _ = app.emit(
                        "qwitch://batch-progress",
                        BatchProgressEvent {
                            action: "claim".into(),
                            running: true,
                            total,
                            completed: idx,
                            current_label: Some(summary.label.clone()),
                            phase: "claiming".into(),
                            jitter_seconds: None,
                            summary_text: None,
                        },
                    );

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

            let _ = app.emit("qwitch://account-updated", &summary.id);

            // 3. 账号间随机抖动延迟（Jitter），避免突发流量
            if idx + 1 < total {
                let min = jitter.min_ms.min(jitter.max_ms);
                let max = jitter.max_ms.max(min);
                let wait_ms = rand::thread_rng().gen_range(min..=max);
                let _ = app.emit(
                    "qwitch://batch-progress",
                    BatchProgressEvent {
                        action: "claim".into(),
                        running: true,
                        total,
                        completed: idx + 1,
                        current_label: None,
                        phase: "jitter_wait".into(),
                        jitter_seconds: Some((wait_ms as f64) / 1000.0),
                        summary_text: None,
                    },
                );
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
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
            format!("，设备日额度限制跳过 {device_limit_skipped_count} 个 Trae CN 账号")
        };
        let summary_text = format!(
            "串行领取完成：成功 {claimed_count} 个，跳过 {skipped_count} 个，异常 {failed_count} 个{breaker_note}{device_limit_note}"
        );
        let _ = app.emit(
            "qwitch://batch-progress",
            BatchProgressEvent {
                action: "claim".into(),
                running: false,
                total,
                completed: total,
                current_label: None,
                phase: "done".into(),
                jitter_seconds: None,
                summary_text: Some(summary_text),
            },
        );
        let _ = app.emit("qwitch://logs-updated", ());

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
        let total = account_ids.len();
        if total == 0 {
            return Ok(());
        }

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
                    let _ = app.emit("qwitch://account-updated", account_id);
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

            let _ = app.emit(
                "qwitch://batch-progress",
                BatchProgressEvent {
                    action: "refresh".into(),
                    running: true,
                    total,
                    completed: idx,
                    current_label: Some(summary.label.clone()),
                    phase: "checking".into(),
                    jitter_seconds: None,
                    summary_text: None,
                },
            );

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

            let _ = app.emit("qwitch://account-updated", &summary.id);

            if idx + 1 < total {
                let min = jitter.min_ms.min(jitter.max_ms);
                let max = jitter.max_ms.max(min);
                let wait_ms = rand::thread_rng().gen_range(min..=max);
                let _ = app.emit(
                    "qwitch://batch-progress",
                    BatchProgressEvent {
                        action: "refresh".into(),
                        running: true,
                        total,
                        completed: idx + 1,
                        current_label: None,
                        phase: "jitter_wait".into(),
                        jitter_seconds: Some((wait_ms as f64) / 1000.0),
                        summary_text: None,
                    },
                );
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
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
        let _ = app.emit(
            "qwitch://batch-progress",
            BatchProgressEvent {
                action: "refresh".into(),
                running: false,
                total,
                completed: total,
                current_label: None,
                phase: "done".into(),
                jitter_seconds: None,
                summary_text: Some(summary_text),
            },
        );
        let _ = app.emit("qwitch://logs-updated", ());

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
