use super::*;
use crate::domain::CredentialPayload;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::collections::HashMap;

fn temp_db_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("qwitch_test_{tag}_{}.db", Uuid::new_v4()))
}

#[test]
fn credential_aad_rejects_cross_account_swap_and_migrates_legacy() {
    let db = QwitchDatabase::open(&temp_db_path("aadbind")).expect("open db");
    let mk = [5u8; 32];
    let mk_env = |uid: &str| CredentialEnvelope {
        tool_id: "trae_cn".to_string(),
        uid: uid.to_string(),
        display_name: None,
        display_email: None,
        payload: CredentialPayload::Token {
            access_token: "t".to_string(),
            refresh_token: None,
            expires_at: None,
            extra: HashMap::new(),
        },
    };
    let a = db.upsert_account(&mk, None, &mk_env("u_a")).expect("a");
    let b = db.upsert_account(&mk, None, &mk_env("u_b")).expect("b");
    assert!(db.get_account_credential(&mk, &a.id).is_ok());

    // 把 A 的密文移植到 B 行：AAD 绑定拒绝跨账号解密
    let a_blob: Vec<u8> = {
        let conn = db.conn.lock().expect("lock");
        conn.query_row(
            "SELECT encrypted_credential FROM accounts WHERE id = ?1",
            rusqlite::params![a.id],
            |r| r.get(0),
        )
        .expect("read a blob")
    };
    {
        let conn = db.conn.lock().expect("lock");
        conn.execute(
            "UPDATE accounts SET encrypted_credential = ?1 WHERE id = ?2",
            rusqlite::params![a_blob, b.id],
        )
        .expect("swap");
    }
    assert!(db.get_account_credential(&mk, &b.id).is_err());

    // 旧格式（无 AAD）行：迁移前可读，迁移后变为 QWA1 密封且仍可读
    let c = db.upsert_account(&mk, None, &mk_env("u_c")).expect("c");
    let legacy_blob = {
        use aes_gcm::{
            aead::{Aead, KeyInit},
            Aes256Gcm, Nonce,
        };
        let cipher = Aes256Gcm::new_from_slice(&mk).expect("cipher");
        let mut nonce = [0u8; 12];
        nonce[0] = 9;
        let pt = serde_json::to_vec(&mk_env("u_c")).expect("serde");
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce), pt.as_ref())
            .expect("legacy encrypt");
        let mut v = nonce.to_vec();
        v.extend_from_slice(&ct);
        v
    };
    {
        let conn = db.conn.lock().expect("lock");
        conn.execute(
            "UPDATE accounts SET encrypted_credential = ?1 WHERE id = ?2",
            rusqlite::params![legacy_blob, c.id],
        )
        .expect("downgrade to legacy");
    }
    assert!(
        db.get_account_credential(&mk, &c.id).is_ok(),
        "legacy readable before migration"
    );
    db.migrate_credential_aad(&mk).expect("migrate aad");
    let c_blob: Vec<u8> = {
        let conn = db.conn.lock().expect("lock");
        conn.query_row(
            "SELECT encrypted_credential FROM accounts WHERE id = ?1",
            rusqlite::params![c.id],
            |r| r.get(0),
        )
        .expect("read c blob")
    };
    assert!(crate::storage::crypto::is_aad_sealed(&c_blob));
    assert!(
        db.get_account_credential(&mk, &c.id).is_ok(),
        "legacy re-sealed and readable"
    );
}

#[test]
fn uid_hash_hmac_migration_upgrades_legacy_rows_and_preserves_lookup() {
    let db = QwitchDatabase::open(&temp_db_path("uidhashmig")).expect("open db");
    let mk = [7u8; 32];
    let env = CredentialEnvelope {
        tool_id: "trae_cn".to_string(),
        uid: "u_migrate_123".to_string(),
        display_name: None,
        display_email: None,
        payload: CredentialPayload::Token {
            access_token: "t".to_string(),
            refresh_token: None,
            expires_at: None,
            extra: HashMap::new(),
        },
    };
    let created = db.upsert_account(&mk, None, &env).expect("insert");
    // 模拟旧库：把该行 uid_hash 回退成无盐 SHA-256（迁移标记此时尚未写入）
    let legacy = crate::storage::crypto::compute_uid_hash("trae_cn", "u_migrate_123");
    {
        let conn = db.conn.lock().expect("lock");
        conn.execute(
            "UPDATE accounts SET uid_hash = ?1 WHERE id = ?2",
            rusqlite::params![legacy, created.id],
        )
        .expect("rewrite uid_hash");
    }
    // 迁移前：HMAC 指纹查不到该账号
    assert!(db
        .find_account_id_by_uid(&mk, "trae_cn", "u_migrate_123")
        .expect("find before")
        .is_none());
    // 执行迁移
    db.migrate_uid_hash(&mk).expect("migrate");
    // 迁移后：HMAC 指纹可命中同一账号
    let found = db
        .find_account_id_by_uid(&mk, "trae_cn", "u_migrate_123")
        .expect("find after");
    assert_eq!(found.map(|(id, _)| id), Some(created.id.clone()));
    // 幂等：再次迁移不报错、结果不变
    db.migrate_uid_hash(&mk).expect("migrate idempotent");
    assert_eq!(
        db.get_setting("storage.uid_hash_algo")
            .expect("flag")
            .as_deref(),
        Some("hmac-sha256-v1")
    );
}

#[test]
fn reimport_refreshes_auto_label_to_nickname() {
    let db = QwitchDatabase::open(&temp_db_path("labelrefresh")).expect("open db");
    let mk = [3u8; 32];
    let base = CredentialEnvelope {
        tool_id: "workbuddy".to_string(),
        uid: "wb_12345678".to_string(),
        display_name: None,
        display_email: None,
        payload: CredentialPayload::Token {
            access_token: "t".to_string(),
            refresh_token: None,
            expires_at: None,
            extra: HashMap::new(),
        },
    };
    let created = db.upsert_account(&mk, None, &base).expect("insert");
    assert_eq!(
        created.label,
        crate::storage::crypto::mask_uid("wb_12345678")
    );

    let mut with_name = base.clone();
    with_name.display_name = Some("小明明".to_string());
    let updated = db.upsert_account(&mk, None, &with_name).expect("reimport");
    assert_eq!(updated.label, "小明明");

    // 自定义备注不被覆盖
    let custom = db
        .upsert_account(&mk, Some("我的主力号"), &with_name)
        .expect("custom label");
    assert_eq!(custom.label, "我的主力号");
    let again = db
        .upsert_account(&mk, None, &with_name)
        .expect("keep custom");
    assert_eq!(again.label, "我的主力号");
}

#[test]
fn device_identity_persists_in_account_summary() {
    let db = QwitchDatabase::open(&temp_db_path("device")).expect("open db");
    let master_key = [9u8; 32];
    let mut files = HashMap::new();
    files.insert(
        "trae_device.json".to_string(),
        BASE64.encode(
            serde_json::to_vec(&serde_json::json!({
                "device_id": "93900972767536",
                "device_type": "mac"
            }))
            .unwrap(),
        ),
    );
    let env = CredentialEnvelope {
        tool_id: "trae_cn".to_string(),
        uid: "3704729780097880".to_string(),
        display_name: None,
        display_email: None,
        payload: CredentialPayload::LocalSnapshot { files },
    };
    let created = db
        .upsert_account(&master_key, Some("Trae 设备B"), &env)
        .expect("upsert");
    assert_eq!(created.device_type.as_deref(), Some("mac"));
    assert_eq!(created.device_id_masked.as_deref(), Some("9390****7536"));
    let list = db.list_accounts(None).expect("list");
    assert_eq!(list[0].device_id_masked.as_deref(), Some("9390****7536"));
}

#[test]
fn database_upsert_merge_bind_switch_and_settings() {
    let db_path = temp_db_path("core");
    let db = QwitchDatabase::open(&db_path).expect("open db");
    let master_key = [42u8; 32];

    let mut files = HashMap::new();
    files.insert("auth.v1.dat".to_string(), BASE64.encode(b"v10encrypted"));

    let snap_env = CredentialEnvelope {
        tool_id: "qoder_cn".to_string(),
        uid: "qoder_cn_abcdef123456".to_string(),
        display_name: None,
        display_email: None,
        payload: CredentialPayload::LocalSnapshot { files },
    };

    let created = db
        .upsert_account(&master_key, Some("Qoder 账号1"), &snap_env)
        .expect("insert snap");
    assert!(created.has_local_snapshot);
    assert!(!created.has_api_token);

    let token_env = CredentialEnvelope {
        tool_id: "qoder_cn".to_string(),
        uid: "qoder_cn_abcdef123456".to_string(),
        display_name: Some("Alice".to_string()),
        display_email: Some("alice@example.com".to_string()),
        payload: CredentialPayload::Token {
            access_token: "tok-merged-123".to_string(),
            refresh_token: None,
            expires_at: None,
            extra: HashMap::new(),
        },
    };

    // 同 UID 再次 upsert 自动合并双轨凭据
    let upserted = db
        .upsert_account(&master_key, Some("Qoder 主力"), &token_env)
        .expect("merge upsert");
    assert_eq!(upserted.id, created.id);
    assert!(upserted.has_local_snapshot);
    assert!(upserted.has_api_token);

    // 另一个账号使用 bind_account_credential 从快照升级到双轨
    let mut files2 = HashMap::new();
    files2.insert("auth.v1.dat".to_string(), BASE64.encode(b"v10other"));
    let snap2 = CredentialEnvelope {
        tool_id: "qoder_cn".to_string(),
        uid: "qoder_cn_other999999".to_string(),
        display_name: None,
        display_email: None,
        payload: CredentialPayload::LocalSnapshot { files: files2 },
    };
    let acc2 = db
        .upsert_account(&master_key, Some("Qoder 副号"), &snap2)
        .expect("acc2");
    let bind_tok = CredentialEnvelope {
        tool_id: "qoder_cn".to_string(),
        uid: "real_uid_9999".to_string(),
        display_name: Some("Bob".to_string()),
        display_email: Some("bob@example.com".to_string()),
        payload: CredentialPayload::Token {
            access_token: "tok-bob-456".to_string(),
            refresh_token: None,
            expires_at: None,
            extra: HashMap::new(),
        },
    };
    let bound = db
        .bind_account_credential(&master_key, &acc2.id, None, &bind_tok)
        .expect("bind");
    assert!(bound.has_local_snapshot);
    assert!(bound.has_api_token);
    assert_eq!(bound.label, "Qoder 副号");

    // set_active_on_client 互斥切换
    db.set_active_on_client("qoder_cn", &created.id)
        .expect("set active 1");
    db.set_active_on_client("qoder_cn", &acc2.id)
        .expect("set active 2");
    let all = db.list_accounts(Some("qoder_cn")).expect("list");
    assert_eq!(all.iter().filter(|a| a.is_active_on_client).count(), 1);
    assert!(
        all.iter()
            .find(|a| a.id == acc2.id)
            .unwrap()
            .is_active_on_client
    );

    // app_settings 读写
    db.set_setting("app.settings", r#"{"jitterMinMs":2000}"#)
        .expect("set setting");
    assert_eq!(
        db.get_setting("app.settings")
            .expect("get setting")
            .as_deref(),
        Some(r#"{"jitterMinMs":2000}"#)
    );

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn list_accounts_keeps_creation_order_after_updates() {
    let db_path = temp_db_path("order");
    let db = QwitchDatabase::open(&db_path).expect("open db");
    let master_key = [9u8; 32];

    let mut ids = Vec::new();
    for (uid, label) in [
        ("u_first", "first"),
        ("u_second", "second"),
        ("u_third", "third"),
    ] {
        let env = CredentialEnvelope {
            tool_id: "trae_cn".to_string(),
            uid: uid.to_string(),
            display_name: None,
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: "tok".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        ids.push(
            db.upsert_account(&master_key, Some(label), &env)
                .expect("insert account")
                .id,
        );
    }

    // 模拟 later 账号被频繁更新：创建顺序与更新顺序相反
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
                "UPDATE accounts SET created_at='2026-01-01T00:00:03Z', updated_at='2026-01-03T00:00:00Z' WHERE label='third'",
                [],
            )
            .unwrap();
        conn.execute(
                "UPDATE accounts SET created_at='2026-01-01T00:00:02Z', updated_at='2026-01-02T00:00:00Z' WHERE label='second'",
                [],
            )
            .unwrap();
        conn.execute(
                "UPDATE accounts SET created_at='2026-01-01T00:00:01Z', updated_at='2026-01-01T00:00:00Z' WHERE label='first'",
                [],
            )
            .unwrap();
    }

    let labels: Vec<String> = db
        .list_accounts(Some("trae_cn"))
        .expect("list")
        .into_iter()
        .map(|a| a.label)
        .collect();
    assert_eq!(labels, vec!["first", "second", "third"]);

    // 本机正在使用的账号必须置顶，其余仍按添加时间升序
    db.set_active_on_client("trae_cn", &ids[1])
        .expect("set active");
    let labels_with_active: Vec<String> = db
        .list_accounts(Some("trae_cn"))
        .expect("list")
        .into_iter()
        .map(|a| a.label)
        .collect();
    assert_eq!(labels_with_active, vec!["second", "first", "third"]);

    let all_tools: Vec<String> = db
        .list_accounts(None)
        .expect("list all")
        .into_iter()
        .map(|a| a.label)
        .collect();
    assert_eq!(all_tools, vec!["second", "first", "third"]);

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn action_logs_archive_retention_restore_and_delete() {
    let db_path = temp_db_path("logs");
    let db = QwitchDatabase::open(&db_path).expect("open db");

    db.append_action_log(None, "trae_cn", "query", "success", None, "查额成功 1")
        .unwrap();
    db.append_action_log(None, "trae_cn", "claim", "skipped", None, "今日已签")
        .unwrap();
    db.append_action_log(None, "workbuddy", "claim", "success", None, "签到 +10")
        .unwrap();
    db.append_action_log(
        None,
        "qoder_cn",
        "query",
        "failed",
        Some(&UnifiedErrorCode::AuthExpired),
        "会话过期",
    )
    .unwrap();

    let stats = db.get_action_log_stats().unwrap();
    assert_eq!(stats.active_count, 4);
    assert_eq!(stats.archived_count, 0);
    assert_eq!(stats.failed_active_count, 1);
    assert_eq!(stats.noise_active_count, 2);

    // 仅归档冗余日常日志（query/success + skipped），保留签到成功与异常失败
    let archived_redundant = db
        .archive_action_logs(&ActionLogCleanupRequest {
            redundant_only: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(archived_redundant, 2);

    let active = db.list_action_logs(LogScope::Active, 50, 0, None).unwrap();
    assert_eq!(active.len(), 2);
    let archived = db
        .list_action_logs(LogScope::Archived, 50, 0, None)
        .unwrap();
    assert_eq!(archived.len(), 2);
    assert!(archived.iter().all(|item| item.archived_at.is_some()));

    // 按最大活跃条数策略保留最新 1 条，将较旧的 1 条自动归档
    let retained = db.apply_log_retention(0, 1).unwrap();
    assert_eq!(retained, 1);
    assert_eq!(
        db.list_action_logs(LogScope::Active, 50, 0, None)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        db.list_action_logs(LogScope::Archived, 50, 0, None)
            .unwrap()
            .len(),
        3
    );

    // 从归档恢复单条并彻底清空剩余已归档日志
    let restore_id = archived[0].id;
    let restored = db
        .unarchive_action_logs(&ActionLogCleanupRequest {
            ids: Some(vec![restore_id]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(restored, 1);
    assert_eq!(
        db.list_action_logs(LogScope::Active, 50, 0, None)
            .unwrap()
            .len(),
        2
    );

    let deleted = db
        .delete_action_logs(&ActionLogCleanupRequest {
            scope: Some(LogScope::Archived),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(
        db.list_action_logs(LogScope::Archived, 50, 0, None)
            .unwrap()
            .len(),
        0
    );

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn quota_snapshot_join_retention_and_rename_account() {
    let db_path = temp_db_path("snapshots");
    let db = QwitchDatabase::open(&db_path).expect("open db");
    let master_key = [9u8; 32];

    let env = CredentialEnvelope {
        tool_id: "trae_cn".to_string(),
        uid: "u_snap_123456".to_string(),
        display_name: Some("SnapUser".to_string()),
        display_email: None,
        payload: CredentialPayload::Token {
            access_token: "tok-1".to_string(),
            refresh_token: None,
            expires_at: None,
            extra: HashMap::new(),
        },
    };
    let acc = db
        .upsert_account(&master_key, Some("初始名称"), &env)
        .unwrap();

    let renamed = db.update_account_label(&acc.id, "新备注名").unwrap();
    assert_eq!(renamed.label, "新备注名");

    for i in 0..25 {
        let snap = QuotaSnapshot {
            plan_name: Some(format!("Plan-{i}")),
            claim_status: ClaimStatus::Available,
            buckets: vec![QuotaBucket {
                key: "k".into(),
                label: "额度".into(),
                used: i as f64,
                total: 100.0,
                remaining: (100 - i) as f64,
                unit: "Credits".into(),
                reset_at: None,
            }],
            fetched_at: format!("2026-11-18T12:{i:02}:00Z"),
        };
        db.save_quota_snapshot(&acc.id, &snap).unwrap();
    }

    let listed = db.list_accounts(Some("trae_cn")).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0]
            .latest_quota
            .as_ref()
            .and_then(|q| q.plan_name.as_deref()),
        Some("Plan-24")
    );

    let conn = db.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM quota_snapshots WHERE account_id = ?1",
            params![acc.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 20);
    drop(conn);

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn cross_day_claim_status_resets_stale_snapshot_and_keeps_same_cst_day() {
    let db_path = temp_db_path("cross_day_claim");
    let db = QwitchDatabase::open(&db_path).expect("open db");
    let master_key = [7u8; 32];

    let env = CredentialEnvelope {
        tool_id: "trae_cn".to_string(),
        uid: "u_cross_day_1".to_string(),
        display_name: Some("CrossDayUser".to_string()),
        display_email: None,
        payload: CredentialPayload::Token {
            access_token: "tok-cross".to_string(),
            refresh_token: None,
            expires_at: None,
            extra: HashMap::new(),
        },
    };
    let acc = db
        .upsert_account(&master_key, Some("跨天账号"), &env)
        .unwrap();

    // 保存一条昨天 UTC+8 的已签到快照，经过数据库读取后今日应归零为 Unknown
    let yesterday_snap = QuotaSnapshot {
        plan_name: Some("Pro".into()),
        claim_status: ClaimStatus::Claimed,
        buckets: vec![],
        fetched_at: "2020-01-01T10:00:00+08:00".into(),
    };
    db.save_quota_snapshot(&acc.id, &yesterday_snap).unwrap();
    let loaded_yesterday = db.get_account_summary(&acc.id).unwrap();
    assert_eq!(
        loaded_yesterday
            .latest_quota
            .as_ref()
            .map(|q| &q.claim_status),
        Some(&ClaimStatus::Unknown)
    );

    // 保存一条今日 UTC+8 的已签到快照，应保持 Claimed
    let today_now_rfc3339 = Utc::now().to_rfc3339();
    let today_snap = QuotaSnapshot {
        plan_name: Some("Pro".into()),
        claim_status: ClaimStatus::Claimed,
        buckets: vec![],
        fetched_at: today_now_rfc3339.clone(),
    };
    db.save_quota_snapshot(&acc.id, &today_snap).unwrap();
    let loaded_today = db.get_account_summary(&acc.id).unwrap();
    assert_eq!(
        loaded_today.latest_quota.as_ref().map(|q| &q.claim_status),
        Some(&ClaimStatus::Claimed)
    );

    // 直接测试跨天 boundary（UTC 16:30 即 UTC+8 次日 00:30 vs 同日 15:30 即 UTC+8 当日 23:30）
    let today_cst = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
    assert!(is_same_cst_day("2026-11-17T16:30:00Z", today_cst));
    assert!(!is_same_cst_day("2026-11-17T15:30:00Z", today_cst));
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Claimed,
            "2026-11-17T15:30:00Z",
            Some("2026-11-17T15:00:00Z"),
            today_cst,
            "trae_cn"
        ),
        ClaimStatus::Unknown
    );
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Claimed,
            "2026-11-17T16:30:00Z",
            None,
            today_cst,
            "trae_cn"
        ),
        ClaimStatus::Claimed
    );
    // 每日重置型工具：今日已领 + 服务端缓存仍返回 Available → 权威口径 Claimed
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Available,
            "2026-11-18T02:00:00Z",
            Some("2026-11-18T01:00:00Z"),
            today_cst,
            "workbuddy"
        ),
        ClaimStatus::Claimed
    );
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Available,
            "2026-11-18T02:00:00Z",
            Some("2026-11-18T01:00:00Z"),
            today_cst,
            "minimax_code_cn"
        ),
        ClaimStatus::Claimed
    );
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Unknown,
            "2026-11-18T02:00:00Z",
            Some("2026-11-18T01:00:00Z"),
            today_cst,
            "minimax_code_cn"
        ),
        ClaimStatus::Claimed
    );
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Available,
            "2026-11-18T02:00:00Z",
            Some("2026-11-18T01:00:00Z"),
            today_cst,
            "lobsterai"
        ),
        ClaimStatus::Claimed
    );
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Unknown,
            "2026-11-18T02:00:00Z",
            Some("2026-11-18T01:00:00Z"),
            today_cst,
            "lobsterai"
        ),
        ClaimStatus::Claimed
    );
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Available,
            "2026-11-18T02:00:00Z",
            Some("2026-11-18T01:00:00Z"),
            today_cst,
            "doubaowork"
        ),
        ClaimStatus::Claimed
    );
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Unknown,
            "2026-11-18T02:00:00Z",
            Some("2026-11-18T01:00:00Z"),
            today_cst,
            "doubaowork"
        ),
        ClaimStatus::Claimed
    );
    // Qoder 活动按周期：今日已领不覆盖新出现的 Available 活动
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Available,
            "2026-11-18T02:00:00Z",
            Some("2026-11-18T01:00:00Z"),
            today_cst,
            "qoder_cn"
        ),
        ClaimStatus::Available
    );

    let _ = std::fs::remove_file(&db_path);
}

fn temp_data_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("qwitch_fp_test_{tag}_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn token_envelope(uid: &str) -> CredentialEnvelope {
    CredentialEnvelope {
        tool_id: "qoder_cn".to_string(),
        uid: uid.to_string(),
        display_name: None,
        display_email: None,
        payload: CredentialPayload::Token {
            access_token: "tok-fingerprint-test".to_string(),
            refresh_token: None,
            expires_at: None,
            extra: HashMap::new(),
        },
    }
}

#[test]
fn key_fingerprint_guards_wrong_key_and_never_blesses_it() {
    let dir = temp_data_dir("guard");
    let db_path = dir.join("qwitch.db");
    let db = QwitchDatabase::open(&db_path).expect("open");
    let key_a = [1u8; 32];
    let key_b = [2u8; 32];

    // 空库：首次写入指纹，重复校验通过；空库孤儿 key_id 也允许重初始化
    db.verify_or_init_key_fingerprint(&dir, &key_a)
        .expect("init empty");
    assert!(dir.join("key_id").exists());
    db.verify_or_init_key_fingerprint(&dir, &key_a)
        .expect("same key");
    db.verify_or_init_key_fingerprint(&dir, &key_b)
        .expect("empty db resets orphan key_id");
    assert_eq!(
        std::fs::read_to_string(dir.join("key_id")).unwrap().trim(),
        crypto::key_fingerprint(&key_b)
    );
    db.verify_or_init_key_fingerprint(&dir, &key_a)
        .expect("reset back to key_a");

    // 有加密账号后指纹不匹配 → 拒绝，且解密失败不改写账号状态
    let acc = db
        .upsert_account(&key_a, Some("指纹测试"), &token_envelope("fp_user_1"))
        .expect("insert");
    assert!(db.verify_or_init_key_fingerprint(&dir, &key_b).is_err());
    let err = db
        .get_account_credential(&key_b, &acc.id)
        .expect_err("wrong key must fail");
    assert!(matches!(err, StorageError::Crypto(_)));
    let summary = db.get_account_summary(&acc.id).expect("summary");
    assert_eq!(
        summary.status,
        AccountStatus::Active,
        "解密失败不得改写账号状态"
    );

    let _ = std::fs::remove_dir_all(&dir);

    // 已有账号但无指纹记录：错误密钥不得被"祝福"写入指纹
    let dir = temp_data_dir("nobless");
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");
    db.upsert_account(&key_a, Some("无指纹"), &token_envelope("fp_user_2"))
        .expect("insert");
    assert!(db.verify_or_init_key_fingerprint(&dir, &key_b).is_err());
    assert!(!dir.join("key_id").exists(), "错误密钥不得写入指纹");
    db.verify_or_init_key_fingerprint(&dir, &key_a)
        .expect("correct key blesses");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn key_id_matching_but_sample_undecryptable_is_rejected() {
    let dir = temp_data_dir("fpsplit");
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");
    let key_a = [41u8; 32];
    let key_b = [42u8; 32];
    db.upsert_account(&key_a, Some("数据属于A"), &token_envelope("fp_split_uid"))
        .expect("insert with A");
    // 伪造指向 B 的 key_id（部分备份恢复场景）：指纹匹配 B 但数据属于 A
    std::fs::write(dir.join("key_id"), crypto::key_fingerprint(&key_b)).unwrap();
    assert!(
        db.verify_or_init_key_fingerprint(&dir, &key_b).is_err(),
        "指纹匹配但样本解不开必须拒启"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reimport_preserves_custom_label_and_paused_status_and_rejects_uid_mismatch() {
    let db_path = temp_db_path("protect");
    let db = QwitchDatabase::open(&db_path).expect("open");
    let key = [21u8; 32];

    let env_a = token_envelope("real_uid_aaa111");
    let acc = db
        .upsert_account(&key, Some("主力号"), &env_a)
        .expect("insert");
    assert_eq!(acc.label, "主力号");

    // 显式停用后重新导入（无显式备注）：备注与停用状态均不被覆盖
    db.update_account_status(&acc.id, AccountStatus::Disabled)
        .expect("disable");
    let mut env_a2 = token_envelope("real_uid_aaa111");
    env_a2.display_name = Some("官方昵称".to_string());
    let again = db.upsert_account(&key, None, &env_a2).expect("reimport");
    assert_eq!(again.id, acc.id);
    assert_eq!(again.label, "主力号", "默认展示名不得覆盖用户自定义备注");
    assert_eq!(
        again.status,
        AccountStatus::Disabled,
        "重新导入不得复活已停用账号"
    );

    // 显式备注仍可覆盖
    let renamed = db
        .upsert_account(&key, Some("新备注"), &env_a2)
        .expect("rename via import");
    assert_eq!(renamed.label, "新备注");

    // risk_paused 同样保护
    db.update_account_status(&acc.id, AccountStatus::RiskPaused)
        .expect("pause");
    let paused = db
        .upsert_account(&key, None, &env_a2)
        .expect("reimport paused");
    assert_eq!(paused.status, AccountStatus::RiskPaused);

    // A 卡绑定 B 凭据：双方真实 UID 不一致必须拒绝
    let env_b = token_envelope("real_uid_bbb222");
    let err = db
        .bind_account_credential(&key, &acc.id, None, &env_b)
        .expect_err("must reject");
    assert!(err.to_string().contains("凭据归属不匹配"));
    let summary = db.get_account_summary(&acc.id).expect("summary");
    assert_eq!(
        summary.uid_masked,
        crypto::mask_uid("real_uid_aaa111"),
        "拒绝后原账号身份不变"
    );

    // fallback UID 目标允许被真实 UID 升级
    let fallback_env = token_envelope("wb_manual_temp123");
    let fb = db
        .upsert_account(&key, None, &fallback_env)
        .expect("fallback insert");
    let bound = db
        .bind_account_credential(&key, &fb.id, None, &env_b)
        .expect("fallback bind ok");
    assert_eq!(bound.uid_masked, crypto::mask_uid("real_uid_bbb222"));

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn default_label_never_contains_plain_pii() {
    let mut env = token_envelope("user_plain_uid_99");
    env.display_name = None;
    env.display_email = Some("alice@example.com".to_string());
    assert_eq!(default_account_label(&env), "al***@example.com");

    env.display_email = Some("13812345678".to_string());
    assert_eq!(default_account_label(&env), "138****5678");

    env.display_email = None;
    assert_eq!(
        default_account_label(&env),
        crypto::mask_uid("user_plain_uid_99")
    );
}

#[test]
fn delete_logs_empty_request_defaults_to_active_scope() {
    let db_path = temp_db_path("delscope");
    let db = QwitchDatabase::open(&db_path).expect("open");
    db.append_action_log(None, "trae_cn", "import", "success", None, "活跃日志")
        .unwrap();
    db.append_action_log(None, "trae_cn", "claim", "success", None, "待归档日志")
        .unwrap();
    db.archive_action_logs(&ActionLogCleanupRequest {
        action_type: Some("claim".to_string()),
        ..Default::default()
    })
    .expect("archive");

    let deleted = db
        .delete_action_logs(&ActionLogCleanupRequest::default())
        .expect("delete");
    assert_eq!(deleted, 1, "空请求默认只删除活跃日志，不得清空归档");
    let archived = db
        .list_action_logs(LogScope::Archived, 100, 0, None)
        .expect("list");
    assert_eq!(archived.len(), 1);

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn legacy_pii_migration_masks_labels_and_logs_once() {
    let dir = temp_data_dir("pii");
    let db_path = dir.join("qwitch.db");
    let db = QwitchDatabase::open(&db_path).expect("open");
    let key = [31u8; 32];

    let env = token_envelope("legacy_uid_123456");
    db.upsert_account(&key, Some("alice@example.com"), &env)
        .expect("insert legacy label");
    let env_compound = token_envelope("legacy_uid_compound");
    db.upsert_account(&key, Some("Alice (alice@example.com)"), &env_compound)
        .expect("insert compound label");
    // 绕过写入即脱敏，直接插入旧版明文日志行，真实覆盖迁移路径
    db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO action_logs (account_id, tool_id, action_type, result_status, error_code, message_sanitized, created_at)
                 VALUES (NULL, 'qoder_cn', 'import', 'success', NULL, ?1, ?2)",
                params![
                    "已导入 alice@example.com (13812345678)",
                    Utc::now().to_rfc3339()
                ],
            )
            .expect("insert legacy plaintext log");

    db.migrate_legacy_pii(&key).expect("migrate");

    let accs = db.list_accounts(None).expect("list");
    let labels: Vec<&str> = accs.iter().map(|a| a.label.as_str()).collect();
    assert!(labels.contains(&"al***@example.com"));
    assert!(labels.contains(&"Alice (al***@example.com)"));
    let logs = db
        .list_action_logs(LogScope::All, 100, 0, None)
        .expect("logs");
    assert!(!logs[0].message_sanitized.contains("alice@example.com"));
    assert!(!logs[0].message_sanitized.contains("13812345678"));
    assert!(
        !dir.join("qwitch.db.pre-pii-migration.bak").exists(),
        "迁移提交成功后必须清理含明文 PII 的临时备份"
    );

    // 幂等：标记后再次运行不改动（即使标签又被写成明文）
    db.upsert_account(&key, Some("bob@example.com"), &env)
        .expect("relabel");
    db.migrate_legacy_pii(&key).expect("migrate again");
    let accs = db.list_accounts(None).expect("list");
    let target_acc = accs
        .iter()
        .find(|a| a.uid_masked == crypto::mask_uid("legacy_uid_123456"))
        .expect("target acc exists");
    assert_eq!(
        target_acc.label, "bob@example.com",
        "已标记迁移不得重复执行"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bind_recovers_paused_and_expired_with_token_clean_db_skips_bak_and_tz_snapshot_sort() {
    // 1. 干净空库执行 PII 迁移不生成冗余 .bak，且不误伤 "Work @ Office"
    let dir = temp_data_dir("clean_pii");
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");
    let key = [33u8; 32];
    let mut work_env = token_envelope("u_work_1");
    work_env.display_name = Some("Work @ Office".to_string());
    let acc = db.upsert_account(&key, None, &work_env).expect("insert");
    assert_eq!(acc.label, "Work @ Office");
    db.migrate_legacy_pii(&key).expect("clean migrate");
    assert!(!dir.join("qwitch.db.pre-pii-migration.bak").exists());
    assert_eq!(
        db.get_account_summary(&acc.id).unwrap().label,
        "Work @ Office"
    );
    let _ = std::fs::remove_dir_all(&dir);

    // 2. bind_account_credential 带 Token 恢复 Expired / RiskPaused，保留 Disabled；无 Token 仅快照不恢复
    let db_path = temp_db_path("bind_recover");
    let db = QwitchDatabase::open(&db_path).expect("open");
    let snap_only = CredentialEnvelope {
        tool_id: "qoder_cn".to_string(),
        uid: "qoder_cn_snap999999".to_string(),
        display_name: None,
        display_email: None,
        payload: CredentialPayload::LocalSnapshot {
            files: HashMap::from([("auth.v1.dat".into(), BASE64.encode(b"dat"))]),
        },
    };
    let acc = db
        .upsert_account(&key, Some("暂停号"), &snap_only)
        .expect("insert");
    db.update_account_status(&acc.id, AccountStatus::RiskPaused)
        .expect("pause");
    let still_paused = db
        .bind_account_credential(&key, &acc.id, None, &snap_only)
        .expect("bind snap");
    assert_eq!(still_paused.status, AccountStatus::RiskPaused);

    let tok_env = token_envelope("real_uid_recover");
    let recovered = db
        .bind_account_credential(&key, &acc.id, None, &tok_env)
        .expect("bind token");
    assert_eq!(recovered.status, AccountStatus::Active);

    db.update_account_status(&acc.id, AccountStatus::Disabled)
        .expect("disable");
    let still_disabled = db
        .bind_account_credential(&key, &acc.id, None, &tok_env)
        .expect("bind disabled");
    assert_eq!(still_disabled.status, AccountStatus::Disabled);

    // 3. Unknown + 今日已签在每日重置型工具上返回 Claimed，Qoder 保持 Unknown
    let today_cst = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Unknown,
            "2026-11-18T01:00:00Z",
            Some("2026-11-18T01:00:00Z"),
            today_cst,
            "trae_cn"
        ),
        ClaimStatus::Claimed
    );
    assert_eq!(
        normalize_daily_claim_status_on_date(
            ClaimStatus::Unknown,
            "2026-11-18T01:00:00Z",
            Some("2026-11-18T01:00:00Z"),
            today_cst,
            "qoder_cn"
        ),
        ClaimStatus::Unknown
    );

    // 4. 不同时区时间戳统一归约成 UTC，按真实时间先后排序（含未归一化的历史原始行）
    let older_later_clock = QuotaSnapshot {
        plan_name: Some("Older".into()),
        claim_status: ClaimStatus::Available,
        buckets: vec![],
        fetched_at: "2026-11-18T11:00:00+08:00".into(), // UTC 03:00
    };
    let newer_earlier_clock = QuotaSnapshot {
        plan_name: Some("Newer".into()),
        claim_status: ClaimStatus::Claimed,
        buckets: vec![],
        fetched_at: "2026-11-18T04:00:00Z".into(), // UTC 04:00
    };
    db.save_quota_snapshot(&acc.id, &older_later_clock).unwrap();
    db.save_quota_snapshot(&acc.id, &newer_earlier_clock)
        .unwrap();
    let latest = db.get_account_summary(&acc.id).unwrap();
    assert_eq!(
        latest
            .latest_quota
            .as_ref()
            .and_then(|q| q.plan_name.as_deref()),
        Some("Newer")
    );

    // 直接插入未归一化 +08:00 旧历史行，验证 julianday() 排序不被字符串字典序误导
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
                "INSERT INTO quota_snapshots (id, account_id, plan_name, claim_status, buckets_json, fetched_at)
                 VALUES ('raw-older', ?1, 'LegacyOlder', 'available', '[]', '2026-11-19T11:30:00+08:00')",
                params![acc.id],
            )
            .unwrap();
        conn.execute(
                "INSERT INTO quota_snapshots (id, account_id, plan_name, claim_status, buckets_json, fetched_at)
                 VALUES ('raw-newer', ?1, 'LegacyNewer', 'claimed', '[]', '2026-11-19T05:00:00Z')",
                params![acc.id],
            )
            .unwrap();
    }
    let latest_mixed = db.get_account_summary(&acc.id).unwrap();
    assert_eq!(
        latest_mixed
            .latest_quota
            .as_ref()
            .and_then(|q| q.plan_name.as_deref()),
        Some("LegacyNewer")
    );

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn orphan_key_id_with_empty_db_allows_reinit() {
    let dir = temp_data_dir("orphan_fp");
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open empty db");
    let old_key = [51u8; 32];
    let new_key = [52u8; 32];
    std::fs::write(dir.join("key_id"), crypto::key_fingerprint(&old_key)).unwrap();

    db.verify_or_init_key_fingerprint(&dir, &new_key)
        .expect("empty db must overwrite orphan key_id");
    assert_eq!(
        std::fs::read_to_string(dir.join("key_id")).unwrap().trim(),
        crypto::key_fingerprint(&new_key)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn multi_sample_allows_boot_when_first_row_corrupt_and_second_decrypts() {
    let dir = temp_data_dir("multisample");
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");
    let key_good = [61u8; 32];

    let acc_good = db
        .upsert_account(&key_good, Some("好账号"), &token_envelope("good_uid_1"))
        .expect("insert good");
    let acc_bad = db
        .upsert_account(&key_good, Some("损坏账号"), &token_envelope("bad_uid_2"))
        .expect("insert second");
    {
        // 把最新一条密文写坏（确保 credential_sample_blobs DESC 首位就是坏样本）
        let conn = db.conn.lock().unwrap();
        conn.execute(
                "UPDATE accounts SET encrypted_credential = ?1, updated_at = '2099-01-01T00:00:00Z' WHERE id = ?2",
                params![vec![9u8; 32], acc_bad.id],
            )
            .unwrap();
        conn.execute(
            "UPDATE accounts SET updated_at = '2026-01-01T00:00:00Z' WHERE id = ?1",
            params![acc_good.id],
        )
        .unwrap();
    }

    let samples = db.credential_sample_blobs(5).expect("samples");
    assert_eq!(samples.len(), 2);
    db.verify_or_init_key_fingerprint(&dir, &key_good)
        .expect("multi-sample verification must succeed via second healthy row");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn action_logs_before_id_cursor_avoids_drift_on_insert() {
    let db_path = temp_db_path("log_cursor");
    let db = QwitchDatabase::open(&db_path).expect("open");
    for i in 1..=5 {
        db.append_action_log(
            None,
            "trae_cn",
            "query",
            "success",
            None,
            &format!("条目 {i}"),
        )
        .unwrap();
    }
    let page1 = db.list_action_logs(LogScope::Active, 2, 0, None).unwrap();
    assert_eq!(page1.len(), 2);
    let cursor = page1.last().unwrap().id;

    // 翻页前插入两条新日志；游标分页下第二页不应出现重复或漏项
    db.append_action_log(None, "trae_cn", "query", "success", None, "新条目 6")
        .unwrap();
    db.append_action_log(None, "trae_cn", "query", "success", None, "新条目 7")
        .unwrap();

    let page2 = db
        .list_action_logs(LogScope::Active, 2, 0, Some(cursor))
        .unwrap();
    let page3 = db
        .list_action_logs(LogScope::Active, 2, 0, Some(page2.last().unwrap().id))
        .unwrap();
    assert_eq!(page2.len(), 2);
    assert_eq!(page3.len(), 1);
    let seen_ids: Vec<i64> = page1
        .iter()
        .chain(page2.iter())
        .chain(page3.iter())
        .map(|l| l.id)
        .collect();
    assert!(seen_ids.windows(2).all(|w| w[0] > w[1]));

    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn apply_log_retention_purges_archived_beyond_window() {
    let db = QwitchDatabase::open(&temp_db_path("logpurge")).expect("open db");
    db.append_action_log(None, "trae_cn", "query", "success", None, "查额成功")
        .unwrap();
    // 标记为「已归档且归档时间极早」，模拟超过保留窗口的归档日志
    db.conn
        .lock()
        .expect("lock")
        .execute(
            "UPDATE action_logs SET archived_at = ?1",
            rusqlite::params!["2000-01-01T00:00:00+00:00"],
        )
        .expect("mark archived");
    assert_eq!(
        db.list_action_logs(LogScope::Archived, 50, 0, None)
            .unwrap()
            .len(),
        1
    );
    // 保留窗口 30 天：归档早于窗口 → 自动清底删除
    db.apply_log_retention(30, 0).unwrap();
    assert_eq!(
        db.list_action_logs(LogScope::Archived, 50, 0, None)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(db.get_action_log_stats().unwrap().archived_count, 0);
}

#[test]
fn corrupted_key_id_is_treated_as_missing_instead_of_conflict() {
    let dir = temp_data_dir("corrupt_keyid");
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");
    let key = [7u8; 32];
    let expected = crypto::key_fingerprint(&key);

    // 建立真实账号，使「指纹冲突」分支真正可达（空库会走重置路径，测不到问题）
    db.upsert_account(&key, Some("损坏指纹"), &token_envelope("corrupt_uid"))
        .expect("insert");
    db.verify_or_init_key_fingerprint(&dir, &key)
        .expect("init fingerprint");
    assert_eq!(
        std::fs::read_to_string(dir.join("key_id")).unwrap().trim(),
        expected
    );

    // 模拟写入中途断电留下的各种损坏形态：均应按「指纹缺失」重建，而不是拒绝启动
    for corrupted in [
        "",                                 // 空文件
        &expected[..16],                    // 截断一半
        &expected[..31],                    // 少一个字符
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz", // 非十六进制
        "\0\0\0\0",                         // 二进制垃圾
    ] {
        std::fs::write(dir.join("key_id"), corrupted.as_bytes()).expect("write corrupted");
        db.verify_or_init_key_fingerprint(&dir, &key)
            .unwrap_or_else(|e| panic!("损坏 key_id {corrupted:?} 不应导致拒启: {e}"));
        assert_eq!(
            std::fs::read_to_string(dir.join("key_id")).unwrap().trim(),
            expected,
            "损坏内容 {corrupted:?} 应被原子重建为正确指纹"
        );
    }

    // 重建后仍能正常解密既有凭据（证明未被误判为冲突而丢数据）
    let accounts = db.list_accounts(None).expect("list");
    assert_eq!(accounts.len(), 1);
    db.get_account_credential(&key, &accounts[0].id)
        .expect("credential still decryptable");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn key_id_write_leaves_no_temp_file_and_is_0600() {
    let dir = temp_data_dir("keyid_atomic");
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");
    let key = [9u8; 32];

    db.verify_or_init_key_fingerprint(&dir, &key)
        .expect("init fingerprint");

    // 原子写不得残留临时文件
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .expect("read dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".key_id.tmp-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "原子写不应残留临时文件，实际残留: {leftovers:?}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("key_id"))
            .expect("stat key_id")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "key_id 必须是 0600，实际 {mode:o}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn aad_migration_records_undecryptable_rows_and_retries_next_boot() {
    let dir = temp_data_dir("aad_legacy_track");
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");
    let key = [3u8; 32];

    // 一个正常账号 + 一个用「错误密钥」加密的坏行（模拟历史损坏/密钥错配）
    let good = db
        .upsert_account(&key, Some("正常"), &token_envelope("aad_good"))
        .expect("good");
    let bad = db
        .upsert_account(&key, Some("损坏"), &token_envelope("aad_bad"))
        .expect("bad");

    // 把 bad 的密文替换成无法用当前密钥解密的旧格式字节
    {
        let conn = db.conn.lock().expect("lock");
        conn.execute(
            "UPDATE accounts SET encrypted_credential = ?1 WHERE id = ?2",
            rusqlite::params![
                vec![0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
                &bad.id
            ],
        )
        .expect("corrupt blob");
    }

    db.migrate_credential_aad(&key).expect("migrate");

    // 正常行已升级为 AAD 格式
    {
        let conn = db.conn.lock().expect("lock");
        let blob: Vec<u8> = conn
            .query_row(
                "SELECT encrypted_credential FROM accounts WHERE id = ?1",
                rusqlite::params![&good.id],
                |r| r.get(0),
            )
            .expect("read good");
        assert!(crypto::is_aad_sealed(&blob), "可解密行必须升级为 QWA1");
    }

    // 坏行被显式记录，而不是静默跳过
    let legacy = db.legacy_aad_account_ids();
    assert_eq!(legacy, vec![bad.id.clone()], "无法解密的历史行必须被记录");

    // 再次调用迁移应继续尝试（不因已完成标志而提前返回）
    db.migrate_credential_aad(&key).expect("retry migrate");
    assert_eq!(
        db.legacy_aad_account_ids(),
        vec![bad.id.clone()],
        "仍无法解密的行应继续留在待升级列表"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wal_checkpoint_keeps_wal_bounded_under_many_writes() {
    let dir = temp_data_dir("wal_bound");
    let db_path = dir.join("qwitch.db");
    let db = QwitchDatabase::open(&db_path).expect("open");
    let key = [11u8; 32];

    // 大量写入制造 WAL 增长
    for i in 0..300 {
        db.upsert_account(
            &key,
            Some(&format!("acc{i}")),
            &token_envelope(&format!("wal_uid_{i}")),
        )
        .expect("write");
    }

    let wal_path = std::path::PathBuf::from(format!("{}-wal", db_path.display()));
    let wal_before = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);

    // TRUNCATE checkpoint 后 WAL 应被清空/显著缩小
    assert!(db.checkpoint_wal_truncate(), "checkpoint 应成功");
    let wal_after = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);

    assert!(
        wal_after < wal_before.max(1),
        "TRUNCATE checkpoint 后 WAL 应收缩：before={wal_before} after={wal_after}"
    );
    assert!(
        wal_after <= 4096,
        "TRUNCATE 后 WAL 应接近空（当前 {wal_after} 字节）"
    );

    // 数据未丢失
    assert_eq!(db.list_accounts(None).expect("list").len(), 300);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wal_pragmas_are_applied() {
    let dir = temp_data_dir("wal_pragma");
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");
    let conn = db.conn.lock().expect("lock");

    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .expect("journal_mode");
    assert_eq!(mode.to_lowercase(), "wal");

    let secure: i64 = conn
        .query_row("PRAGMA secure_delete", [], |r| r.get(0))
        .expect("secure_delete");
    assert_eq!(secure, 1, "secure_delete 必须开启，避免删除数据残留空闲页");

    let limit: i64 = conn
        .query_row("PRAGMA journal_size_limit", [], |r| r.get(0))
        .expect("journal_size_limit");
    assert_eq!(limit, 4_194_304);

    let autockpt: i64 = conn
        .query_row("PRAGMA wal_autocheckpoint", [], |r| r.get(0))
        .expect("wal_autocheckpoint");
    assert_eq!(autockpt, 1000);

    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn upsert_snapshot_only_preserves_expired_status() {
    let dir = temp_data_dir("upsert_expired");
    let key = [11u8; 32];
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");

    let mut snap_files = HashMap::new();
    snap_files.insert("doubaowork_profile.zip".to_string(), "AAAA".to_string());
    let mut env_tok = token_envelope("dw_u1");
    env_tok.tool_id = "doubaowork".to_string();
    let acc = db
        .upsert_account(&key, Some("豆包号"), &env_tok)
        .expect("insert");
    db.update_account_status(&acc.id, AccountStatus::Expired)
        .expect("mark expired");

    let env_snap = CredentialEnvelope {
        tool_id: "doubaowork".to_string(),
        uid: "dw_u1".to_string(),
        display_email: None,
        display_name: Some("豆包号".to_string()),
        payload: CredentialPayload::LocalSnapshot { files: snap_files },
    };
    db.upsert_account(&key, None, &env_snap)
        .expect("upsert snapshot only");

    let list = db.list_accounts(Some("doubaowork")).expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].status, AccountStatus::Expired);
    assert!(list[0].has_local_snapshot);
    assert!(list[0].has_api_token);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sync_account_profile_updates_auto_label_and_preserves_custom_remark() {
    let dir = temp_data_dir("sync_profile");
    let key = [19u8; 32];
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");

    let mut env1 = token_envelope("u_auto");
    env1.display_name = Some("旧昵称".to_string());
    let auto_acc = db.upsert_account(&key, None, &env1).expect("insert auto");
    assert_eq!(auto_acc.label, "旧昵称");

    let changed = db
        .sync_account_profile(&key, &auto_acc.id, Some("新昵称"), Some("new@example.com"))
        .expect("sync auto");
    assert!(changed);
    let updated_auto = db.get_account_summary(&auto_acc.id).expect("summary");
    assert_eq!(updated_auto.label, "新昵称");
    assert_eq!(
        updated_auto.email_masked.as_deref(),
        Some("ne***@example.com")
    );

    let mut env2 = token_envelope("u_custom");
    env2.display_name = Some("原昵称".to_string());
    let custom_acc = db
        .upsert_account(&key, Some("我的自定义备注"), &env2)
        .expect("insert custom");
    assert_eq!(custom_acc.label, "我的自定义备注");

    let changed_custom = db
        .sync_account_profile(&key, &custom_acc.id, Some("改名后的昵称"), None)
        .expect("sync custom");
    assert!(changed_custom);
    let updated_custom = db.get_account_summary(&custom_acc.id).expect("summary");
    assert_eq!(updated_custom.label, "我的自定义备注");
    assert_eq!(updated_custom.account_name.as_deref(), Some("改名后的昵称"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn account_alias_preserves_platform_name_and_restores_on_empty() {
    let dir = temp_data_dir("alias_restore");
    let key = [29u8; 32];
    let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open");

    let mut env = token_envelope("u_alias");
    env.display_name = Some("原昵称".to_string());
    let created = db.upsert_account(&key, None, &env).expect("insert");
    assert_eq!(created.label, "原昵称");
    assert_eq!(created.account_name.as_deref(), Some("原昵称"));

    let aliased = db
        .update_account_alias(&key, &created.id, "主力号")
        .expect("set alias");
    assert_eq!(aliased.label, "主力号");
    assert_eq!(aliased.account_name.as_deref(), Some("原昵称"));

    db.sync_account_profile(&key, &created.id, Some("平台新昵称"), None)
        .expect("sync profile");
    let after_sync = db.get_account_summary(&created.id).expect("after sync");
    assert_eq!(after_sync.label, "主力号");
    assert_eq!(after_sync.account_name.as_deref(), Some("平台新昵称"));

    let restored = db
        .update_account_alias(&key, &created.id, "")
        .expect("restore");
    assert_eq!(restored.label, "平台新昵称");
    assert_eq!(restored.account_name.as_deref(), Some("平台新昵称"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// 回归：码道本机快照账号用 OAuth 重新授权时，合并会保留快照形态，
/// 必须把「独立授权」标记补进快照，否则账号仍是只读、无法参与网关，
/// 用户会以为授权没生效（同账号还会被当成两条）。
#[test]
fn codearts_oauth_reauth_marks_existing_local_snapshot_as_isolated() {
    let db = QwitchDatabase::open(&temp_db_path("codearts_reauth")).expect("open db");
    let mk = [7u8; 32];
    let uid = "c02b6539a92f4027bc7b95945aba38f7";

    let mut files = HashMap::new();
    files.insert(
        "decrypted_token.txt".to_string(),
        BASE64.encode(b"client-token"),
    );
    let local = CredentialEnvelope {
        tool_id: "codearts".to_string(),
        uid: uid.to_string(),
        display_name: Some("码道".to_string()),
        display_email: None,
        payload: CredentialPayload::LocalSnapshot { files },
    };
    let acc = db.upsert_account(&mk, None, &local).expect("seed local");

    let mut extra = HashMap::new();
    extra.insert("codearts_mode".to_string(), "isolated_oauth".to_string());
    extra.insert("codearts_code_verifier".to_string(), "v".to_string());
    let oauth = CredentialEnvelope {
        tool_id: "codearts".to_string(),
        uid: uid.to_string(),
        display_name: Some("码道".to_string()),
        display_email: None,
        payload: CredentialPayload::Token {
            access_token: String::new(),
            refresh_token: Some("rt".to_string()),
            expires_at: None,
            extra,
        },
    };
    let merged = db.upsert_account(&mk, None, &oauth).expect("reauth");

    // 同一个账号：不能新增第二条
    assert_eq!(merged.id, acc.id);
    let count: i64 = {
        let conn = db.conn.lock().expect("lock");
        conn.query_row(
            "SELECT COUNT(*) FROM accounts WHERE tool_id = 'codearts'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(count, 1, "重复授权不得产生第二个账号");

    // 快照形态保留，但必须带上独立授权标记
    let (_, env) = db.get_account_credential(&mk, &acc.id).expect("load");
    match env.payload {
        CredentialPayload::LocalSnapshot { files } => {
            assert!(
                files.contains_key(ISOLATED_OAUTH_MARKER_FILE),
                "快照必须补回独立授权标记"
            );
            assert!(files.contains_key("decrypted_token.txt"));
        }
        other => panic!("期望保留快照形态，实际为 {other:?}"),
    }
    assert_eq!(
        db.detect_codearts_auth_mode(&mk, &acc.id),
        crate::domain::CodeArtsAuthMode::IsolatedOauth
    );
}
