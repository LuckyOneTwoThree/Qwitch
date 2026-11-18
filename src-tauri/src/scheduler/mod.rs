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
            max_ms: 4000,
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
    queue_lock: Mutex<()>,
}

impl AntiRiskScheduler {
    pub fn new() -> Self {
        Self {
            queue_lock: Mutex::new(()),
        }
    }

    pub async fn run_daily_claim_batch(
        &self,
        app: &AppHandle,
        db: &Arc<QwitchDatabase>,
        master_key: &[u8; 32],
        adapters: &HashMap<String, Arc<dyn ToolAdapter>>,
        account_ids: Vec<String>,
        jitter: JitterConfig,
    ) -> Result<(), String> {
        // 严格单队列串行锁，防止并发触发平台风控
        let _guard = self.queue_lock.lock().await;

        let total = account_ids.len();
        if total == 0 {
            return Ok(());
        }

        let mut claimed_count: usize = 0;
        let mut skipped_count: usize = 0;
        let mut failed_count: usize = 0;

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
                    handle_adapter_failure(
                        db,
                        &summary.id,
                        &summary.tool_id,
                        &summary.label,
                        "claim",
                        err,
                    );
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

                    // 2. 执行手动领取
                    match adapter.claim_daily(&cred).await {
                        Ok(res) => {
                            if let Some(updated_q) = res.updated_quota {
                                let _ = db.save_quota_snapshot(&summary.id, &updated_q);
                            }
                            if res.claimed {
                                claimed_count += 1;
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
                        Err(err) => {
                            failed_count += 1;
                            handle_adapter_failure(
                                db,
                                &summary.id,
                                &summary.tool_id,
                                &summary.label,
                                "claim",
                                err,
                            );
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

        let summary_text = format!(
            "串行领取完成：成功 {claimed_count} 个，跳过 {skipped_count} 个，异常 {failed_count} 个"
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

        Ok(())
    }

    pub async fn run_quota_refresh_batch(
        &self,
        app: &AppHandle,
        db: &Arc<QwitchDatabase>,
        master_key: &[u8; 32],
        adapters: &HashMap<String, Arc<dyn ToolAdapter>>,
        account_ids: Vec<String>,
        jitter: JitterConfig,
    ) -> Result<(), String> {
        // 与批量领取共用单队列串行锁，防止并发请求触发平台限流与风控
        let _guard = self.queue_lock.lock().await;

        let total = account_ids.len();
        if total == 0 {
            return Ok(());
        }

        let mut refreshed_count: usize = 0;
        let mut skipped_count: usize = 0;
        let mut failed_count: usize = 0;

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
                        &format!("已刷新 {} ({}) 的最新额度", summary.label, summary.uid_masked),
                    );
                }
                Err(err) => {
                    failed_count += 1;
                    handle_adapter_failure(
                        db,
                        &summary.id,
                        &summary.tool_id,
                        &summary.label,
                        "query",
                        err,
                    );
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

        let summary_text = format!(
            "额度刷新完成：成功 {refreshed_count} 个，跳过 {skipped_count} 个，异常 {failed_count} 个"
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

        Ok(())
    }
}

fn handle_adapter_failure(
    db: &Arc<QwitchDatabase>,
    account_id: &str,
    tool_id: &str,
    label: &str,
    action_type: &str,
    err: crate::adapter::AdapterError,
) {
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
}
