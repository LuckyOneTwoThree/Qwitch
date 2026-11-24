use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::domain::{
    default_channel_for_tool, default_model_options_for_tool, format_scoped_model_id,
    normalize_canonical_model_id, split_scoped_model_id, sync_pool_model_policies, AccountPool,
    AccountStatus, AffinityMode, AgentBindingOptions, AgentBindingStatus, CodeArtsAuthMode,
    CreatePoolInput, CreatePoolMemberInput, CredentialPayload, MemberTier, PoolMember,
    PoolModelPolicy, PoolStrategy, RoutingAttempt, RoutingTraceItem, RoutingTraceStatus,
    TargetAgentId, UpdatePoolPatch, UpstreamAccountCandidate, UpstreamModelOption,
    WireProtocolKind,
};
use crate::storage::model_discovery::load_magpie_provider_models;
use crate::storage::{crypto, QwitchDatabase, StorageError};

#[derive(Debug, Clone)]
pub struct StoredAgentBinding {
    pub agent_id: TargetAgentId,
    pub mounted_pool_ids: Vec<String>,
    pub default_model_key: Option<String>,
    pub options: AgentBindingOptions,
    pub status: AgentBindingStatus,
    pub last_hit_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RestoredStashFile {
    pub file_path: String,
    pub original_existed: bool,
    pub original_content: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IsolatedOauthMarker {
    mode: String,
    authorized_at: String,
}

fn slugify_pool_name(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.len() >= 3 {
        trimmed
    } else {
        format!("pool-{}", &Uuid::new_v4().simple().to_string()[..6])
    }
}

fn parse_strategy(raw: &str) -> PoolStrategy {
    match raw {
        "drain" => PoolStrategy::Drain,
        "order" => PoolStrategy::Order,
        "round_robin" => PoolStrategy::RoundRobin,
        _ => PoolStrategy::QuotaFirst,
    }
}

fn strategy_to_str(s: PoolStrategy) -> &'static str {
    match s {
        PoolStrategy::QuotaFirst => "quota_first",
        PoolStrategy::Drain => "drain",
        PoolStrategy::Order => "order",
        PoolStrategy::RoundRobin => "round_robin",
    }
}

fn parse_affinity(raw: &str) -> AffinityMode {
    match raw {
        "session" => AffinityMode::Session,
        "turn" => AffinityMode::Turn,
        "off" => AffinityMode::Off,
        _ => AffinityMode::AutoCache,
    }
}

fn affinity_to_str(a: AffinityMode) -> &'static str {
    match a {
        AffinityMode::AutoCache => "auto_cache",
        AffinityMode::Session => "session",
        AffinityMode::Turn => "turn",
        AffinityMode::Off => "off",
    }
}

fn parse_tier(raw: &str) -> MemberTier {
    match raw {
        "t2_backup" => MemberTier::T2Backup,
        "t3_fallback" => MemberTier::T3Fallback,
        _ => MemberTier::T1Primary,
    }
}

fn parse_binding_status(raw: &str) -> AgentBindingStatus {
    match raw {
        "bound" => AgentBindingStatus::Bound,
        "drifted" => AgentBindingStatus::Drifted,
        "needs_restart" => AgentBindingStatus::NeedsRestart,
        _ => AgentBindingStatus::Unbound,
    }
}

pub fn binding_status_to_str(s: AgentBindingStatus) -> &'static str {
    match s {
        AgentBindingStatus::Bound => "bound",
        AgentBindingStatus::Drifted => "drifted",
        AgentBindingStatus::NeedsRestart => "needs_restart",
        AgentBindingStatus::Unbound => "unbound",
    }
}

fn parse_protocol(raw: &str) -> WireProtocolKind {
    match raw {
        "anthropic_messages" => WireProtocolKind::AnthropicMessages,
        "openai_responses" => WireProtocolKind::OpenaiResponses,
        _ => WireProtocolKind::OpenaiChat,
    }
}

fn protocol_to_str(p: WireProtocolKind) -> &'static str {
    match p {
        WireProtocolKind::OpenaiChat => "openai_chat",
        WireProtocolKind::AnthropicMessages => "anthropic_messages",
        WireProtocolKind::OpenaiResponses => "openai_responses",
    }
}

fn account_status_to_str(status: &AccountStatus) -> &'static str {
    match status {
        AccountStatus::Active => "active",
        AccountStatus::Expired => "expired",
        AccountStatus::RiskPaused => "risk_paused",
        AccountStatus::Disabled => "disabled",
    }
}

fn format_quota_stats(summary: &crate::domain::AccountSummary) -> (u8, String) {
    if summary.tool_id == "custom_api_key" {
        return (100, "按量计费 API Key".to_string());
    }
    if let Some(q) = &summary.latest_quota {
        if let Some(b) = q.buckets.first() {
            let pct = if b.total > 0.0 {
                ((b.remaining / b.total) * 100.0).round().clamp(0.0, 100.0) as u8
            } else {
                0
            };
            let text = if b.total > 0.0 {
                format!("{:.0} / {:.0} {}", b.remaining, b.total, b.unit)
            } else {
                format!("{:.0} {}", b.remaining, b.unit)
            };
            return (pct, text);
        }
    }
    (0, "尚未刷新额度".to_string())
}

fn stash_aad(agent_id: TargetAgentId, file_path: &str) -> Vec<u8> {
    format!("stash:{}:{file_path}", agent_id.as_str()).into_bytes()
}

impl QwitchDatabase {
    pub fn detect_codearts_auth_mode(
        &self,
        master_key: &[u8; 32],
        account_id: &str,
    ) -> CodeArtsAuthMode {
        let Ok((_, env)) = self.get_account_credential(master_key, account_id) else {
            return CodeArtsAuthMode::ClientReadonly;
        };
        match &env.payload {
            CredentialPayload::Token { extra, .. } => {
                if extra.get("codearts_mode").map(String::as_str) == Some("isolated_oauth") {
                    CodeArtsAuthMode::IsolatedOauth
                } else {
                    CodeArtsAuthMode::ClientReadonly
                }
            }
            CredentialPayload::LocalSnapshot { files } => {
                if files.contains_key("isolated_oauth.json") {
                    CodeArtsAuthMode::IsolatedOauth
                } else {
                    CodeArtsAuthMode::ClientReadonly
                }
            }
            CredentialPayload::Cookies { .. } => CodeArtsAuthMode::ClientReadonly,
        }
    }

    pub fn authorize_codearts_isolated(
        &self,
        master_key: &[u8; 32],
        account_id: &str,
    ) -> Result<(), StorageError> {
        let (summary, mut env) = self.get_account_credential(master_key, account_id)?;
        if summary.tool_id != "codearts" {
            return Err(StorageError::Validation(
                "仅「码道 CodeArts」账号需要签发独立 OAuth 隔离授权".into(),
            ));
        }
        match &mut env.payload {
            CredentialPayload::Token { extra, .. } => {
                extra.insert("codearts_mode".into(), "isolated_oauth".into());
            }
            CredentialPayload::LocalSnapshot { files } => {
                let marker = IsolatedOauthMarker {
                    mode: "isolated_oauth".into(),
                    authorized_at: Utc::now().to_rfc3339(),
                };
                let bytes = serde_json::to_vec(&marker)?;
                files.insert("isolated_oauth.json".into(), BASE64.encode(bytes));
            }
            CredentialPayload::Cookies { .. } => {
                return Err(StorageError::Validation(
                    "Cookie 凭据不支持切换为独立 OAuth 隔离".into(),
                ));
            }
        }

        let aad = crypto::credential_aad(&summary.tool_id, account_id);
        let blob = crypto::encrypt_credential(master_key, &aad, &env)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "UPDATE accounts SET encrypted_credential = ?1, is_active_on_client = 0, updated_at = ?2 WHERE id = ?3",
            params![blob, now, account_id],
        )?;
        Ok(())
    }

    pub fn save_cached_account_models(
        &self,
        account_id: &str,
        tool_id: &str,
        models: &[UpstreamModelOption],
    ) -> Result<(), StorageError> {
        let now = Utc::now().to_rfc3339();
        let json = serde_json::to_string(models)?;
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "INSERT INTO account_discovered_models (account_id, tool_id, models_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id) DO UPDATE SET
                tool_id = excluded.tool_id,
                models_json = excluded.models_json,
                updated_at = excluded.updated_at",
            params![account_id, tool_id, json, now],
        )?;
        Ok(())
    }

    pub fn load_cached_models_map(
        &self,
    ) -> Result<HashMap<String, Vec<UpstreamModelOption>>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut stmt =
            conn.prepare("SELECT account_id, models_json FROM account_discovered_models")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut map = HashMap::new();
        for (aid, raw_json) in rows {
            if let Ok(list) = serde_json::from_str::<Vec<UpstreamModelOption>>(&raw_json) {
                if !list.is_empty() {
                    map.insert(aid, list);
                }
            }
        }
        Ok(map)
    }

    pub fn list_upstream_candidates(
        &self,
        master_key: &[u8; 32],
    ) -> Result<Vec<UpstreamAccountCandidate>, StorageError> {
        let accounts = self.list_accounts(None)?;
        let cached_map = self.load_cached_models_map().unwrap_or_default();
        let mut tool_cache: HashMap<String, Vec<UpstreamModelOption>> = HashMap::new();
        let mut out = Vec::with_capacity(accounts.len());
        for acc in accounts {
            let (quota_percent, quota_text) = format_quota_stats(&acc);
            let code_arts_mode = if acc.tool_id == "codearts" {
                Some(self.detect_codearts_auth_mode(master_key, &acc.id))
            } else {
                None
            };
            let default_tier = if acc.tool_id == "custom_api_key" {
                MemberTier::T3Fallback
            } else if code_arts_mode.is_some() {
                MemberTier::T2Backup
            } else {
                MemberTier::T1Primary
            };
            let model_options = if let Some(cached) = cached_map.get(&acc.id) {
                cached.clone()
            } else if let Some(shared) = tool_cache.get(&acc.tool_id) {
                shared.clone()
            } else {
                let resolved = load_magpie_provider_models(&acc.tool_id)
                    .unwrap_or_else(|| default_model_options_for_tool(&acc.tool_id));
                tool_cache.insert(acc.tool_id.clone(), resolved.clone());
                resolved
            };
            let default_models: Vec<String> = model_options.iter().map(|m| m.id.clone()).collect();
            out.push(UpstreamAccountCandidate {
                account_id: acc.id.clone(),
                tool_id: acc.tool_id.clone(),
                label: acc.label,
                uid_masked: acc.uid_masked,
                channel: default_channel_for_tool(&acc.tool_id),
                default_models,
                model_options,
                default_tier,
                quota_percent,
                quota_text,
                is_active_on_client: Some(acc.is_active_on_client),
                account_status: account_status_to_str(&acc.status).to_string(),
                code_arts_mode,
            });
        }
        Ok(out)
    }

    pub fn list_pools(
        &self,
        master_key: &[u8; 32],
        cooldowns: &HashMap<String, i64>,
    ) -> Result<Vec<AccountPool>, StorageError> {
        let candidates = self.list_upstream_candidates(master_key)?;
        let candidate_map: HashMap<String, UpstreamAccountCandidate> = candidates
            .into_iter()
            .map(|c| (c.account_id.clone(), c))
            .collect();

        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut pool_stmt = conn.prepare(
            "SELECT id, slug, name, description, scene_tag, strategy, affinity_mode,
                    skip_active_client_account, cooldown_seconds, auto_refresh_on_401,
                    fallback_api_key_enabled, enabled, today_requests, today_failovers,
                    cache_hit_rate, created_at
             FROM account_pools
             ORDER BY created_at DESC",
        )?;

        struct RawPool {
            id: String,
            slug: String,
            name: String,
            description: String,
            scene_tag: String,
            strategy: PoolStrategy,
            affinity_mode: AffinityMode,
            skip_active_client_account: bool,
            cooldown_seconds: u64,
            auto_refresh_on_401: bool,
            fallback_api_key_enabled: bool,
            enabled: bool,
            today_requests: u64,
            today_failovers: u64,
            cache_hit_rate: u8,
            created_at: String,
        }

        let raw_pools = pool_stmt
            .query_map([], |r| {
                Ok(RawPool {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    name: r.get(2)?,
                    description: r.get(3)?,
                    scene_tag: r.get(4)?,
                    strategy: parse_strategy(&r.get::<_, String>(5)?),
                    affinity_mode: parse_affinity(&r.get::<_, String>(6)?),
                    skip_active_client_account: r.get::<_, i64>(7)? != 0,
                    cooldown_seconds: r.get::<_, i64>(8)?.max(10) as u64,
                    auto_refresh_on_401: r.get::<_, i64>(9)? != 0,
                    fallback_api_key_enabled: r.get::<_, i64>(10)? != 0,
                    enabled: r.get::<_, i64>(11)? != 0,
                    today_requests: r.get::<_, i64>(12)?.max(0) as u64,
                    today_failovers: r.get::<_, i64>(13)?.max(0) as u64,
                    cache_hit_rate: r.get::<_, i64>(14)?.clamp(0, 100) as u8,
                    created_at: r.get(15)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut policies_stmt = conn.prepare(
            "SELECT pool_id, model_id, enabled, fallback_model_id, claude_alias_id
             FROM pool_model_policies",
        )?;
        let mut policies_by_pool: HashMap<String, Vec<PoolModelPolicy>> = HashMap::new();
        let policy_rows = policies_stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                PoolModelPolicy {
                    model_id: r.get(1)?,
                    enabled: r.get::<_, i64>(2)? != 0,
                    fallback_model_id: r.get(3)?,
                    claude_alias_id: r.get(4)?,
                },
            ))
        })?;
        for row in policy_rows {
            let (pid, pol) = row?;
            policies_by_pool.entry(pid).or_default().push(pol);
        }

        let mut members_stmt = conn.prepare(
            "SELECT id, pool_id, account_id, supported_models_json, tier, priority, enabled, last_latency_ms
             FROM pool_members
             ORDER BY priority ASC, id ASC",
        )?;
        let mut members_by_pool: HashMap<String, Vec<PoolMember>> = HashMap::new();
        let member_rows = members_stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(0)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)? != 0,
                r.get::<_, Option<i64>>(7)?,
            ))
        })?;

        for row in member_rows {
            let (pid, mid, account_id, models_json, tier_raw, priority, enabled, last_lat) = row?;
            let Some(cand) = candidate_map.get(&account_id) else {
                continue;
            };
            let raw_models: Vec<String> =
                serde_json::from_str(&models_json).unwrap_or_else(|_| cand.default_models.clone());
            let mut seen_norm = HashSet::new();
            let supported_models: Vec<String> = raw_models
                .into_iter()
                .map(|m| normalize_canonical_model_id(&cand.tool_id, &m))
                .filter(|m| seen_norm.insert(m.clone()))
                .collect();
            let tier = if cand.tool_id == "custom_api_key" {
                MemberTier::T3Fallback
            } else {
                parse_tier(&tier_raw)
            };
            members_by_pool.entry(pid).or_default().push(PoolMember {
                id: mid,
                account_id: cand.account_id.clone(),
                tool_id: cand.tool_id.clone(),
                label: cand.label.clone(),
                uid_masked: cand.uid_masked.clone(),
                supported_models,
                tier,
                channel: cand.channel,
                enabled,
                priority: priority.max(1) as usize,
                quota_percent: cand.quota_percent,
                quota_text: cand.quota_text.clone(),
                cooldown_until_ms: cooldowns.get(&cand.account_id).copied(),
                is_active_on_client: cand.is_active_on_client,
                account_status: Some(cand.account_status.clone()),
                code_arts_mode: cand.code_arts_mode,
                last_latency_ms: last_lat.map(|v| v.max(0) as u64),
            });
        }

        let mut pools = Vec::with_capacity(raw_pools.len());
        for rp in raw_pools {
            let members = members_by_pool.remove(&rp.id).unwrap_or_default();
            let existing_policies = policies_by_pool.remove(&rp.id).unwrap_or_default();
            let model_policies =
                sync_pool_model_policies(&rp.slug, &members, &existing_policies, None);
            pools.push(AccountPool {
                id: rp.id,
                slug: rp.slug,
                name: rp.name,
                description: rp.description,
                scene_tag: rp.scene_tag,
                enabled: rp.enabled,
                strategy: rp.strategy,
                affinity_mode: rp.affinity_mode,
                skip_active_client_account: rp.skip_active_client_account,
                cooldown_seconds: rp.cooldown_seconds,
                auto_refresh_on_401: rp.auto_refresh_on_401,
                fallback_api_key_enabled: rp.fallback_api_key_enabled,
                model_policies,
                members,
                today_requests: rp.today_requests,
                today_failovers: rp.today_failovers,
                cache_hit_rate: rp.cache_hit_rate,
                created_at: rp.created_at,
            });
        }
        Ok(pools)
    }

    pub fn create_pool(
        &self,
        master_key: &[u8; 32],
        input: &CreatePoolInput,
    ) -> Result<AccountPool, StorageError> {
        let candidates = self.list_upstream_candidates(master_key)?;
        let candidate_map: HashMap<String, UpstreamAccountCandidate> = candidates
            .into_iter()
            .map(|c| (c.account_id.clone(), c))
            .collect();

        let id = input
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 64
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("pool-{}", &Uuid::new_v4().simple().to_string()[..10]));
        let mut slug = slugify_pool_name(&input.name);
        let now = Utc::now().to_rfc3339();

        let mut built_members = Vec::new();
        let mut seen_accounts: HashSet<String> = HashSet::new();
        for item in input.members.iter() {
            let Some(cand) = candidate_map.get(&item.account_id) else {
                continue;
            };
            // 同一账号重复入池：仅保留首次出现，避免唯一约束半写入
            if !seen_accounts.insert(cand.account_id.clone()) {
                continue;
            }
            if cand.code_arts_mode == Some(CodeArtsAuthMode::ClientReadonly) {
                return Err(StorageError::Validation(format!(
                    "{} 为只读客户端凭据，请先签发独立 OAuth 后再入池",
                    cand.label
                )));
            }
            let tier = if cand.tool_id == "custom_api_key" {
                MemberTier::T3Fallback
            } else {
                item.tier
            };
            let models = if item.supported_models.is_empty() {
                cand.default_models.clone()
            } else {
                let mut seen = HashSet::new();
                item.supported_models
                    .iter()
                    .map(|m| normalize_canonical_model_id(&cand.tool_id, m))
                    .filter(|m| seen.insert(m.clone()))
                    .collect()
            };
            built_members.push(PoolMember {
                id: format!("pm-{}", &Uuid::new_v4().simple().to_string()[..10]),
                account_id: cand.account_id.clone(),
                tool_id: cand.tool_id.clone(),
                label: cand.label.clone(),
                uid_masked: cand.uid_masked.clone(),
                supported_models: models,
                tier,
                channel: cand.channel,
                enabled: true,
                priority: built_members.len() + 1,
                quota_percent: cand.quota_percent,
                quota_text: cand.quota_text.clone(),
                cooldown_until_ms: None,
                is_active_on_client: cand.is_active_on_client,
                account_status: Some(cand.account_status.clone()),
                code_arts_mode: cand.code_arts_mode,
                last_latency_ms: Some(520 + (built_members.len() as u64) * 65),
            });
        }

        if built_members.is_empty() {
            return Err(StorageError::Validation(
                "号池至少需要 1 个有效可用账号成员".into(),
            ));
        }

        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let slug_exists: bool = conn
            .query_row(
                "SELECT 1 FROM account_pools WHERE slug = ?1",
                params![slug],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if slug_exists {
            slug = format!("{slug}-{}", &Uuid::new_v4().simple().to_string()[..4]);
        }

        let policies = sync_pool_model_policies(
            &slug,
            &built_members,
            &[],
            input.enabled_model_ids.as_deref(),
        );

        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO account_pools (
                id, slug, name, description, scene_tag, strategy, affinity_mode,
                skip_active_client_account, cooldown_seconds, auto_refresh_on_401,
                fallback_api_key_enabled, enabled, today_requests, today_failovers,
                cache_hit_rate, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, 0, 0, 0, ?12, ?12)",
            params![
                id,
                slug,
                input.name.trim(),
                input.description.trim(),
                input.scene_tag.trim(),
                strategy_to_str(input.strategy),
                affinity_to_str(input.affinity_mode),
                input.skip_active_client_account as i64,
                input.cooldown_seconds.clamp(10, 86_400) as i64,
                input.auto_refresh_on_401 as i64,
                input.fallback_api_key_enabled as i64,
                now
            ],
        )?;

        for pol in &policies {
            tx.execute(
                "INSERT INTO pool_model_policies (pool_id, model_id, enabled, fallback_model_id, claude_alias_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    pol.model_id,
                    pol.enabled as i64,
                    pol.fallback_model_id,
                    pol.claude_alias_id
                ],
            )?;
        }

        for m in &built_members {
            let models_json = serde_json::to_string(&m.supported_models)?;
            tx.execute(
                "INSERT INTO pool_members (id, pool_id, account_id, supported_models_json, tier, priority, enabled, last_latency_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
                params![
                    m.id,
                    id,
                    m.account_id,
                    models_json,
                    m.tier.as_str(),
                    m.priority as i64,
                    m.last_latency_ms.map(|v| v as i64)
                ],
            )?;
        }

        tx.commit()?;

        Ok(AccountPool {
            id,
            slug,
            name: input.name.trim().to_string(),
            description: input.description.trim().to_string(),
            scene_tag: input.scene_tag.clone(),
            enabled: true,
            strategy: input.strategy,
            affinity_mode: input.affinity_mode,
            skip_active_client_account: input.skip_active_client_account,
            cooldown_seconds: input.cooldown_seconds.clamp(10, 86_400),
            auto_refresh_on_401: input.auto_refresh_on_401,
            fallback_api_key_enabled: input.fallback_api_key_enabled,
            model_policies: policies,
            members: built_members,
            today_requests: 0,
            today_failovers: 0,
            cache_hit_rate: 0,
            created_at: now,
        })
    }

    #[allow(clippy::type_complexity)]
    pub fn update_pool(&self, pool_id: &str, patch: &UpdatePoolPatch) -> Result<(), StorageError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let current: Option<(String, String, String, String, i64, i64, i64, i64, i64)> = conn
            .query_row(
                "SELECT name, description, strategy, affinity_mode, skip_active_client_account,
                        cooldown_seconds, auto_refresh_on_401, fallback_api_key_enabled, enabled
                 FROM account_pools WHERE id = ?1",
                params![pool_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                    ))
                },
            )
            .optional()?;
        let Some((name, desc, strat, aff, skip, cd, auto401, fb_key, enabled)) = current else {
            return Err(StorageError::NotFound(pool_id.to_string()));
        };

        conn.execute(
            "UPDATE account_pools
             SET name = ?1, description = ?2, strategy = ?3, affinity_mode = ?4,
                 skip_active_client_account = ?5, cooldown_seconds = ?6,
                 auto_refresh_on_401 = ?7, fallback_api_key_enabled = ?8,
                 enabled = ?9, updated_at = ?10
             WHERE id = ?11",
            params![
                patch.name.as_deref().unwrap_or(&name),
                patch.description.as_deref().unwrap_or(&desc),
                patch.strategy.map(strategy_to_str).unwrap_or(&strat),
                patch.affinity_mode.map(affinity_to_str).unwrap_or(&aff),
                patch
                    .skip_active_client_account
                    .map(|v| v as i64)
                    .unwrap_or(skip),
                patch
                    .cooldown_seconds
                    .map(|v| v.max(10) as i64)
                    .unwrap_or(cd),
                patch
                    .auto_refresh_on_401
                    .map(|v| v as i64)
                    .unwrap_or(auto401),
                patch
                    .fallback_api_key_enabled
                    .map(|v| v as i64)
                    .unwrap_or(fb_key),
                patch.enabled.map(|v| v as i64).unwrap_or(enabled),
                now,
                pool_id
            ],
        )?;
        Ok(())
    }

    pub fn delete_pool(&self, pool_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let affected = conn.execute("DELETE FROM account_pools WHERE id = ?1", params![pool_id])?;
        if affected == 0 {
            return Err(StorageError::NotFound(pool_id.to_string()));
        }
        Ok(())
    }

    pub fn toggle_pool_enabled(&self, pool_id: &str) -> Result<bool, StorageError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let current: Option<i64> = conn
            .query_row(
                "SELECT enabled FROM account_pools WHERE id = ?1",
                params![pool_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(curr) = current else {
            return Err(StorageError::NotFound(pool_id.to_string()));
        };
        let next = curr == 0;
        conn.execute(
            "UPDATE account_pools SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
            params![next as i64, now, pool_id],
        )?;
        Ok(next)
    }

    pub fn reorder_pool_member(
        &self,
        pool_id: &str,
        member_id: &str,
        direction: &str,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id FROM pool_members WHERE pool_id = ?1 ORDER BY priority ASC, id ASC",
        )?;
        let mut ids: Vec<String> = stmt
            .query_map(params![pool_id], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let Some(idx) = ids.iter().position(|id| id == member_id) else {
            return Err(StorageError::NotFound(member_id.to_string()));
        };
        let target_idx = if direction == "up" {
            idx.checked_sub(1)
        } else if idx + 1 < ids.len() {
            Some(idx + 1)
        } else {
            None
        };
        if let Some(tidx) = target_idx {
            ids.swap(idx, tidx);
            for (i, id) in ids.iter().enumerate() {
                conn.execute(
                    "UPDATE pool_members SET priority = ?1 WHERE id = ?2",
                    params![(i + 1) as i64, id],
                )?;
            }
        }
        Ok(())
    }

    pub fn toggle_pool_member(&self, pool_id: &str, member_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let current: Option<i64> = conn
            .query_row(
                "SELECT enabled FROM pool_members WHERE id = ?1 AND pool_id = ?2",
                params![member_id, pool_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(curr) = current else {
            return Err(StorageError::NotFound(member_id.to_string()));
        };
        conn.execute(
            "UPDATE pool_members SET enabled = ?1 WHERE id = ?2",
            params![(curr == 0) as i64, member_id],
        )?;
        Ok(())
    }

    fn resync_pool_policies_locked(
        conn: &rusqlite::Connection,
        pool_id: &str,
    ) -> Result<(), StorageError> {
        let slug: String = conn.query_row(
            "SELECT slug FROM account_pools WHERE id = ?1",
            params![pool_id],
            |r| r.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT supported_models_json FROM pool_members WHERE pool_id = ?1 ORDER BY priority ASC",
        )?;
        let rows = stmt.query_map(params![pool_id], |r| r.get::<_, String>(0))?;
        let mut all_models = Vec::new();
        let mut seen = HashSet::new();
        for row in rows {
            let list: Vec<String> = serde_json::from_str(&row?).unwrap_or_default();
            for mid in list {
                if seen.insert(mid.clone()) {
                    all_models.push(mid);
                }
            }
        }

        let mut pstmt =
            conn.prepare("SELECT model_id FROM pool_model_policies WHERE pool_id = ?1")?;
        let existing: HashSet<String> = pstmt
            .query_map(params![pool_id], |r| r.get(0))?
            .collect::<Result<HashSet<_>, _>>()?;

        for old_mid in existing.difference(&seen) {
            conn.execute(
                "DELETE FROM pool_model_policies WHERE pool_id = ?1 AND model_id = ?2",
                params![pool_id, old_mid],
            )?;
        }
        for mid in all_models {
            if !existing.contains(&mid) {
                let alias = crate::domain::build_claude_model_alias(&slug, &mid);
                conn.execute(
                    "INSERT INTO pool_model_policies (pool_id, model_id, enabled, fallback_model_id, claude_alias_id)
                     VALUES (?1, ?2, 1, NULL, ?3)",
                    params![pool_id, mid, alias],
                )?;
            }
        }
        Ok(())
    }

    pub fn toggle_pool_member_model(
        &self,
        pool_id: &str,
        member_id: &str,
        model_id: &str,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT pm.supported_models_json, a.tool_id
                 FROM pool_members pm
                 JOIN accounts a ON a.id = pm.account_id
                 WHERE pm.id = ?1 AND pm.pool_id = ?2",
                params![member_id, pool_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((raw_json, tool_id)) = row else {
            return Err(StorageError::NotFound(member_id.to_string()));
        };
        let norm_id = normalize_canonical_model_id(&tool_id, model_id);
        let mut seen = HashSet::new();
        let mut models: Vec<String> = serde_json::from_str::<Vec<String>>(&raw_json)
            .unwrap_or_default()
            .into_iter()
            .map(|m| normalize_canonical_model_id(&tool_id, &m))
            .filter(|m| seen.insert(m.clone()))
            .collect();
        if let Some(pos) = models.iter().position(|m| m == &norm_id) {
            if models.len() <= 1 {
                return Err(StorageError::Validation(
                    "每个账号成员至少需保留 1 个服务模型；若不再需要请移出该账号".into(),
                ));
            }
            models.remove(pos);
        } else {
            models.push(norm_id);
        }
        conn.execute(
            "UPDATE pool_members SET supported_models_json = ?1 WHERE id = ?2",
            params![serde_json::to_string(&models)?, member_id],
        )?;
        Self::resync_pool_policies_locked(&conn, pool_id)?;
        Ok(())
    }

    pub fn update_pool_member_tier(
        &self,
        pool_id: &str,
        member_id: &str,
        tier: MemberTier,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let tool_id: Option<String> = conn
            .query_row(
                "SELECT a.tool_id FROM pool_members pm
                 JOIN accounts a ON a.id = pm.account_id
                 WHERE pm.id = ?1 AND pm.pool_id = ?2",
                params![member_id, pool_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(tid) = tool_id else {
            return Err(StorageError::NotFound(member_id.to_string()));
        };
        if tid == "custom_api_key" && tier != MemberTier::T3Fallback {
            return Err(StorageError::Validation(
                "按量计费 API Key 已强制锁定在 T3 兜底档，防止误抢免费订阅流量".into(),
            ));
        }
        conn.execute(
            "UPDATE pool_members SET tier = ?1 WHERE id = ?2",
            params![tier.as_str(), member_id],
        )?;
        Ok(())
    }

    pub fn toggle_pool_model_enabled(
        &self,
        pool_id: &str,
        model_id: &str,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let current: Option<i64> = conn
            .query_row(
                "SELECT enabled FROM pool_model_policies WHERE pool_id = ?1 AND model_id = ?2",
                params![pool_id, model_id],
                |r| r.get(0),
            )
            .optional()?;
        match current {
            Some(curr) => {
                conn.execute(
                    "UPDATE pool_model_policies SET enabled = ?1 WHERE pool_id = ?2 AND model_id = ?3",
                    params![(curr == 0) as i64, pool_id, model_id],
                )?;
            }
            None => {
                // 该模型来自成员 supported_models 但尚无显式 policy 行（list_pools 内存合成、
                // 默认视为开启）。首次切换时补一条 disabled 行落库，避免「可见不可切」。
                conn.execute(
                    "INSERT INTO pool_model_policies (pool_id, model_id, enabled, fallback_model_id, claude_alias_id)
                     VALUES (?1, ?2, 0, NULL, NULL)",
                    params![pool_id, model_id],
                )?;
            }
        }
        Ok(())
    }

    pub fn set_pool_model_fallback(
        &self,
        pool_id: &str,
        model_id: &str,
        fallback_model_id: Option<&str>,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let affected = conn.execute(
            "UPDATE pool_model_policies SET fallback_model_id = ?1 WHERE pool_id = ?2 AND model_id = ?3",
            params![fallback_model_id, pool_id, model_id],
        )?;
        if affected == 0 {
            // 影子 policy 尚未落库：插入一行（enabled=1）并写入 fallback，避免 NotFound
            conn.execute(
                "INSERT INTO pool_model_policies (pool_id, model_id, enabled, fallback_model_id, claude_alias_id)
                 VALUES (?1, ?2, 1, ?3, NULL)",
                params![pool_id, model_id, fallback_model_id],
            )?;
        }
        Ok(())
    }

    pub fn ensure_agent_dedicated_pool(
        &self,
        master_key: &[u8; 32],
        agent_id: TargetAgentId,
    ) -> Result<(String, StoredAgentBinding), StorageError> {
        let dedicated_id = format!("pool-agent-{}", agent_id.as_str());
        let pools = self.list_pools(master_key, &HashMap::new())?;
        let stored_opt = self
            .get_stored_agent_bindings()?
            .into_iter()
            .find(|b| b.agent_id == agent_id);

        if pools.iter().any(|p| p.id == dedicated_id) {
            let mut binding = stored_opt.unwrap_or(StoredAgentBinding {
                agent_id,
                mounted_pool_ids: vec![dedicated_id.clone()],
                default_model_key: None,
                options: Default::default(),
                status: AgentBindingStatus::Bound,
                last_hit_at: None,
            });
            if binding.mounted_pool_ids != vec![dedicated_id.clone()] {
                binding.mounted_pool_ids = vec![dedicated_id.clone()];
                self.upsert_agent_binding(&binding)?;
            }
            return Ok((dedicated_id, binding));
        }

        let agent_name = match agent_id {
            TargetAgentId::ClaudeDesktop => "Claude 桌面版",
            TargetAgentId::Opencode => "OpenCode",
            TargetAgentId::Dsh => "DeepSeek Harness",
            TargetAgentId::CodexDesktop => "Codex 桌面版",
            TargetAgentId::Mimo => "MiMo Code",
            TargetAgentId::Zcode => "ZCode",
        };
        let now = Utc::now().to_rfc3339();
        let candidates = self.list_upstream_candidates(master_key)?;
        let candidate_map: HashMap<String, UpstreamAccountCandidate> = candidates
            .iter()
            .map(|c| (c.account_id.clone(), c.clone()))
            .collect();

        let source_pool = stored_opt
            .as_ref()
            .and_then(|b| b.mounted_pool_ids.first())
            .and_then(|pid| pools.iter().find(|p| &p.id == pid));

        let strategy = source_pool
            .map(|p| p.strategy)
            .unwrap_or(PoolStrategy::Order);
        let affinity_mode = source_pool
            .map(|p| p.affinity_mode)
            .unwrap_or(AffinityMode::AutoCache);
        let skip_active = source_pool
            .map(|p| p.skip_active_client_account)
            .unwrap_or(true);
        let cooldown_seconds = source_pool.map(|p| p.cooldown_seconds).unwrap_or(60);
        let auto_401 = source_pool.map(|p| p.auto_refresh_on_401).unwrap_or(true);
        let fallback_key = source_pool
            .map(|p| p.fallback_api_key_enabled)
            .unwrap_or(true);

        let mut built_members: Vec<PoolMember> = Vec::new();
        let mut seen_acc = HashSet::new();
        if let Some(src) = source_pool {
            let enabled_policies: HashSet<&str> = src
                .model_policies
                .iter()
                .filter(|mp| mp.enabled)
                .map(|mp| mp.model_id.as_str())
                .collect();
            for m in &src.members {
                if !seen_acc.insert(m.account_id.clone()) {
                    continue;
                }
                let scoped_models: Vec<String> = m
                    .supported_models
                    .iter()
                    .filter(|mid| {
                        enabled_policies.is_empty()
                            || enabled_policies.contains(mid.as_str())
                            || enabled_policies
                                .contains(format_scoped_model_id(&m.tool_id, mid).as_str())
                    })
                    .map(|mid| {
                        format_scoped_model_id(
                            &m.tool_id,
                            &normalize_canonical_model_id(&m.tool_id, split_scoped_model_id(mid).1),
                        )
                    })
                    .collect();
                let mut dedup_seen = HashSet::new();
                let scoped_models: Vec<String> = scoped_models
                    .into_iter()
                    .filter(|s| dedup_seen.insert(s.clone()))
                    .collect();
                let mut cloned = m.clone();
                cloned.id = format!("pm-{}", &Uuid::new_v4().simple().to_string()[..10]);
                cloned.priority = built_members.len() + 1;
                cloned.supported_models = scoped_models;
                built_members.push(cloned);
            }
        }

        // 补齐其他已授权可用账号，初始开放为空（待用户按工具勾选），确保同工具切主备无需先加池
        for cand in &candidates {
            if !seen_acc.insert(cand.account_id.clone()) {
                continue;
            }
            if cand.code_arts_mode == Some(CodeArtsAuthMode::ClientReadonly) {
                continue;
            }
            built_members.push(PoolMember {
                id: format!("pm-{}", &Uuid::new_v4().simple().to_string()[..10]),
                account_id: cand.account_id.clone(),
                tool_id: cand.tool_id.clone(),
                label: cand.label.clone(),
                uid_masked: cand.uid_masked.clone(),
                supported_models: Vec::new(),
                tier: if cand.tool_id == "custom_api_key" {
                    MemberTier::T3Fallback
                } else {
                    MemberTier::T1Primary
                },
                channel: cand.channel,
                enabled: true,
                priority: built_members.len() + 1,
                quota_percent: cand.quota_percent,
                quota_text: cand.quota_text.clone(),
                cooldown_until_ms: None,
                is_active_on_client: cand.is_active_on_client,
                account_status: Some(cand.account_status.clone()),
                code_arts_mode: cand.code_arts_mode,
                last_latency_ms: Some(520),
            });
        }

        let slug = agent_id.as_str().to_string();
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let slug_in_use: bool = conn
            .query_row(
                "SELECT 1 FROM account_pools WHERE slug = ?1 AND id != ?2",
                params![slug, dedicated_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        let final_slug = if slug_in_use {
            format!("{slug}-{}", &Uuid::new_v4().simple().to_string()[..4])
        } else {
            slug
        };

        conn.execute(
            "INSERT INTO account_pools (
                id, slug, name, description, scene_tag, strategy, affinity_mode,
                skip_active_client_account, cooldown_seconds, auto_refresh_on_401,
                fallback_api_key_enabled, enabled, today_requests, today_failovers,
                cache_hit_rate, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, 0, 0, 0, ?12, ?12)",
            params![
                dedicated_id,
                final_slug,
                format!("{agent_name} 专属路由"),
                format!("直接为 {agent_name} 配置订阅工具、开放模型与同工具主备调度"),
                "Agent 专属",
                strategy_to_str(strategy),
                affinity_to_str(affinity_mode),
                skip_active as i64,
                cooldown_seconds.clamp(10, 86_400) as i64,
                auto_401 as i64,
                fallback_key as i64,
                now
            ],
        )?;

        for m in &built_members {
            if !candidate_map.contains_key(&m.account_id) {
                continue;
            }
            conn.execute(
                "INSERT INTO pool_members (id, pool_id, account_id, supported_models_json, tier, priority, enabled, last_latency_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    m.id,
                    dedicated_id,
                    m.account_id,
                    serde_json::to_string(&m.supported_models)?,
                    m.tier.as_str(),
                    m.priority as i64,
                    m.enabled as i64,
                    m.last_latency_ms.map(|v| v as i64)
                ],
            )?;
        }
        Self::resync_pool_policies_locked(&conn, &dedicated_id)?;
        drop(conn);

        let mut next_default_key = stored_opt
            .as_ref()
            .and_then(|b| b.default_model_key.clone());
        if let Some(dk) = next_default_key.as_deref() {
            if let Some((_, raw_mid)) = dk.split_once("::") {
                if let Some(src) = source_pool {
                    let (_, bare) = split_scoped_model_id(raw_mid);
                    if let Some(m) = src.members.iter().find(|mb| {
                        mb.supported_models
                            .iter()
                            .any(|s| s == raw_mid || split_scoped_model_id(s).1 == bare)
                    }) {
                        next_default_key = Some(format!(
                            "{dedicated_id}::{}",
                            format_scoped_model_id(&m.tool_id, bare)
                        ));
                    }
                }
            }
        }

        let binding = StoredAgentBinding {
            agent_id,
            mounted_pool_ids: vec![dedicated_id.clone()],
            default_model_key: next_default_key,
            options: stored_opt.map(|b| b.options).unwrap_or_default(),
            status: AgentBindingStatus::Bound,
            last_hit_at: None,
        };
        self.upsert_agent_binding(&binding)?;
        Ok((dedicated_id, binding))
    }

    pub fn set_pool_tool_models(
        &self,
        master_key: &[u8; 32],
        pool_id: &str,
        tool_id: &str,
        model_ids: &[String],
    ) -> Result<(), StorageError> {
        let candidates = self.list_upstream_candidates(master_key)?;
        let mut seen = HashSet::new();
        let scoped_models: Vec<String> = model_ids
            .iter()
            .map(|m| {
                let (_, bare) = split_scoped_model_id(m);
                let canonical = normalize_canonical_model_id(tool_id, bare);
                format_scoped_model_id(tool_id, &canonical)
            })
            .filter(|m| seen.insert(m.clone()))
            .collect();
        let models_json = serde_json::to_string(&scoped_models)?;

        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let max_prio: i64 = conn.query_row(
            "SELECT COALESCE(MAX(priority), 0) FROM pool_members WHERE pool_id = ?1",
            params![pool_id],
            |r| r.get(0),
        )?;
        let mut next_prio = max_prio + 1;

        for cand in candidates.iter().filter(|c| {
            c.tool_id == tool_id && c.code_arts_mode != Some(CodeArtsAuthMode::ClientReadonly)
        }) {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT id FROM pool_members WHERE pool_id = ?1 AND account_id = ?2",
                    params![pool_id, cand.account_id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(mid) = existing {
                conn.execute(
                    "UPDATE pool_members SET supported_models_json = ?1 WHERE id = ?2",
                    params![models_json, mid],
                )?;
            } else {
                let mid = format!("pm-{}", &Uuid::new_v4().simple().to_string()[..10]);
                let tier = if tool_id == "custom_api_key" {
                    MemberTier::T3Fallback
                } else {
                    MemberTier::T1Primary
                };
                conn.execute(
                    "INSERT INTO pool_members (id, pool_id, account_id, supported_models_json, tier, priority, enabled)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
                    params![mid, pool_id, cand.account_id, models_json, tier.as_str(), next_prio],
                )?;
                next_prio += 1;
            }
        }

        Self::resync_pool_policies_locked(&conn, pool_id)?;
        for mid in &scoped_models {
            conn.execute(
                "UPDATE pool_model_policies SET enabled = 1 WHERE pool_id = ?1 AND model_id = ?2",
                params![pool_id, mid],
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_pool_tool_account_schedule(
        &self,
        master_key: &[u8; 32],
        pool_id: &str,
        tool_id: &str,
        ordered_account_ids: &[String],
        enabled_account_ids: &[String],
        strategy: Option<PoolStrategy>,
        skip_active_client_account: Option<bool>,
        cooldown_seconds: Option<u64>,
    ) -> Result<(), StorageError> {
        let patch = UpdatePoolPatch {
            strategy,
            skip_active_client_account,
            cooldown_seconds,
            ..Default::default()
        };
        if strategy.is_some() || skip_active_client_account.is_some() || cooldown_seconds.is_some()
        {
            self.update_pool(pool_id, &patch)?;
        }

        let candidates = self.list_upstream_candidates(master_key)?;
        let enabled_set: HashSet<&str> = enabled_account_ids.iter().map(String::as_str).collect();
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;

        let mut stmt = conn.prepare(
            "SELECT pm.id, pm.account_id, a.tool_id, pm.priority
             FROM pool_members pm
             JOIN accounts a ON a.id = pm.account_id
             WHERE pm.pool_id = ?1
             ORDER BY pm.priority ASC, pm.id ASC",
        )?;
        let all_rows: Vec<(String, String, String, i64)> = stmt
            .query_map(params![pool_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut tool_member_by_acc: HashMap<String, String> = HashMap::new();
        let mut tool_slots: Vec<i64> = Vec::new();
        for (mid, acc_id, tid, prio) in &all_rows {
            if tid == tool_id {
                tool_member_by_acc.insert(acc_id.clone(), mid.clone());
                tool_slots.push(*prio);
            }
        }
        let mut next_prio = all_rows.iter().map(|r| r.3).max().unwrap_or(0) + 1;

        for cand in candidates.iter().filter(|c| {
            c.tool_id == tool_id && c.code_arts_mode != Some(CodeArtsAuthMode::ClientReadonly)
        }) {
            if !tool_member_by_acc.contains_key(&cand.account_id) {
                let mid = format!("pm-{}", &Uuid::new_v4().simple().to_string()[..10]);
                let tier = if tool_id == "custom_api_key" {
                    MemberTier::T3Fallback
                } else {
                    MemberTier::T1Primary
                };
                conn.execute(
                    "INSERT INTO pool_members (id, pool_id, account_id, supported_models_json, tier, priority, enabled)
                     VALUES (?1, ?2, ?3, '[]', ?4, ?5, 1)",
                    params![mid, pool_id, cand.account_id, tier.as_str(), next_prio],
                )?;
                tool_member_by_acc.insert(cand.account_id.clone(), mid);
                tool_slots.push(next_prio);
                next_prio += 1;
            }
        }
        tool_slots.sort_unstable();

        let mut remaining_accs: Vec<(String, i64)> = tool_member_by_acc
            .iter()
            .map(|(aid, mid)| {
                let prio = conn
                    .query_row(
                        "SELECT priority FROM pool_members WHERE id = ?1",
                        params![mid],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()
                    .ok()
                    .flatten()
                    .unwrap_or(i64::MAX);
                (aid.clone(), prio)
            })
            .collect();
        remaining_accs.sort_by_key(|(_, prio)| *prio);
        let mut final_acc_order: Vec<String> = Vec::new();
        for aid in ordered_account_ids {
            if tool_member_by_acc.contains_key(aid) && !final_acc_order.contains(aid) {
                final_acc_order.push(aid.clone());
            }
        }
        for (aid, _) in remaining_accs {
            if !final_acc_order.contains(&aid) {
                final_acc_order.push(aid);
            }
        }

        for (idx, aid) in final_acc_order.iter().enumerate() {
            if let Some(mid) = tool_member_by_acc.get(aid) {
                let prio = tool_slots.get(idx).copied().unwrap_or((idx + 1) as i64);
                let is_en = enabled_set.contains(aid.as_str()) as i64;
                conn.execute(
                    "UPDATE pool_members SET priority = ?1, enabled = ?2 WHERE id = ?3",
                    params![prio, is_en, mid],
                )?;
            }
        }
        Ok(())
    }

    pub fn add_members_to_pool(
        &self,
        master_key: &[u8; 32],
        pool_id: &str,
        items: &[CreatePoolMemberInput],
    ) -> Result<(), StorageError> {
        let candidates = self.list_upstream_candidates(master_key)?;
        let candidate_map: HashMap<String, UpstreamAccountCandidate> = candidates
            .into_iter()
            .map(|c| (c.account_id.clone(), c))
            .collect();

        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let max_prio: i64 = conn.query_row(
            "SELECT COALESCE(MAX(priority), 0) FROM pool_members WHERE pool_id = ?1",
            params![pool_id],
            |r| r.get(0),
        )?;
        let mut next_prio = max_prio + 1;

        for item in items {
            let Some(cand) = candidate_map.get(&item.account_id) else {
                continue;
            };
            if cand.code_arts_mode == Some(CodeArtsAuthMode::ClientReadonly) {
                return Err(StorageError::Validation(format!(
                    "{} 为只读客户端凭据，请先签发独立 OAuth 授权",
                    cand.label
                )));
            }
            let incoming_models: Vec<String> = if item.supported_models.is_empty() {
                cand.default_models.clone()
            } else {
                let mut seen = HashSet::new();
                item.supported_models
                    .iter()
                    .map(|m| normalize_canonical_model_id(&cand.tool_id, m))
                    .filter(|m| seen.insert(m.clone()))
                    .collect()
            };

            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT id, supported_models_json FROM pool_members WHERE pool_id = ?1 AND account_id = ?2",
                    params![pool_id, item.account_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;

            if let Some((mid, raw_json)) = existing {
                let mut merged: Vec<String> = serde_json::from_str(&raw_json).unwrap_or_default();
                for m in incoming_models {
                    if !merged.contains(&m) {
                        merged.push(m);
                    }
                }
                conn.execute(
                    "UPDATE pool_members SET supported_models_json = ?1 WHERE id = ?2",
                    params![serde_json::to_string(&merged)?, mid],
                )?;
            } else {
                let mid = format!("pm-{}", &Uuid::new_v4().simple().to_string()[..10]);
                let tier = if cand.tool_id == "custom_api_key" {
                    MemberTier::T3Fallback
                } else {
                    item.tier
                };
                conn.execute(
                    "INSERT INTO pool_members (id, pool_id, account_id, supported_models_json, tier, priority, enabled)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
                    params![
                        mid,
                        pool_id,
                        item.account_id,
                        serde_json::to_string(&incoming_models)?,
                        tier.as_str(),
                        next_prio
                    ],
                )?;
                next_prio += 1;
            }
        }

        Self::resync_pool_policies_locked(&conn, pool_id)?;
        Ok(())
    }

    pub fn remove_pool_member(&self, pool_id: &str, member_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let affected = conn.execute(
            "DELETE FROM pool_members WHERE id = ?1 AND pool_id = ?2",
            params![member_id, pool_id],
        )?;
        if affected == 0 {
            return Err(StorageError::NotFound(member_id.to_string()));
        }
        let mut stmt = conn.prepare(
            "SELECT id FROM pool_members WHERE pool_id = ?1 ORDER BY priority ASC, id ASC",
        )?;
        let remaining: Vec<String> = stmt
            .query_map(params![pool_id], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for (i, id) in remaining.iter().enumerate() {
            conn.execute(
                "UPDATE pool_members SET priority = ?1 WHERE id = ?2",
                params![(i + 1) as i64, id],
            )?;
        }
        // 池内已无成员时自动禁用，避免 Agent 挂载一个永不可能成功的空池
        if remaining.is_empty() {
            conn.execute(
                "UPDATE account_pools SET enabled = 0 WHERE id = ?1",
                params![pool_id],
            )?;
        }
        Self::resync_pool_policies_locked(&conn, pool_id)?;
        Ok(())
    }

    pub fn increment_pool_stats(
        &self,
        pool_id: &str,
        failover_occurred: bool,
        cache_hit: bool,
    ) -> Result<(), StorageError> {
        let now = Utc::now().to_rfc3339();
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let row: Option<(i64, i64, i64, Option<String>)> = conn
            .query_row(
                "SELECT today_requests, today_failovers, cache_hit_rate, stats_date FROM account_pools WHERE id = ?1",
                params![pool_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some((reqs, fails, old_rate, stats_date)) = row {
            // 跨日重置「today」统计，避免永久累计失真
            let same_day = stats_date.as_deref() == Some(today.as_str());
            let (base_reqs, base_fails, base_rate) = if same_day {
                (reqs, fails, old_rate)
            } else {
                (0, 0, 0)
            };
            let new_reqs = base_reqs + 1;
            let new_fails = if failover_occurred {
                base_fails + 1
            } else {
                base_fails
            };
            let hit_sample = if cache_hit { 100 } else { 0 };
            let new_rate = if base_reqs == 0 {
                hit_sample
            } else {
                ((base_rate * 4 + hit_sample) / 5).clamp(0, 100)
            };
            conn.execute(
                "UPDATE account_pools SET today_requests = ?1, today_failovers = ?2, cache_hit_rate = ?3, stats_date = ?4, updated_at = ?5 WHERE id = ?6",
                params![new_reqs, new_fails, new_rate, today, now, pool_id],
            )?;
        }
        Ok(())
    }

    pub fn get_stored_agent_bindings(&self) -> Result<Vec<StoredAgentBinding>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT agent_id, mounted_pool_ids_json, default_model_key, options_json, status, last_hit_at
             FROM agent_bindings",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut out = Vec::new();
        for (aid, pools_json, def_key, opts_json, status_raw, last_hit) in rows {
            let Some(agent_id) = TargetAgentId::from_id(&aid) else {
                continue;
            };
            let mounted_pool_ids: Vec<String> =
                serde_json::from_str(&pools_json).unwrap_or_default();
            let options: AgentBindingOptions = serde_json::from_str(&opts_json).unwrap_or_default();
            out.push(StoredAgentBinding {
                agent_id,
                mounted_pool_ids,
                default_model_key: def_key,
                options,
                status: parse_binding_status(&status_raw),
                last_hit_at: last_hit,
            });
        }
        Ok(out)
    }

    pub fn upsert_agent_binding(&self, binding: &StoredAgentBinding) -> Result<(), StorageError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "INSERT INTO agent_bindings (
                agent_id, mounted_pool_ids_json, default_model_key, binding_token_hash,
                options_json, status, last_hit_at, last_checked_at, updated_at
             ) VALUES (?1, ?2, ?3, '', ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(agent_id) DO UPDATE SET
                mounted_pool_ids_json = excluded.mounted_pool_ids_json,
                default_model_key = excluded.default_model_key,
                options_json = excluded.options_json,
                status = excluded.status,
                last_hit_at = excluded.last_hit_at,
                last_checked_at = excluded.last_checked_at,
                updated_at = excluded.updated_at",
            params![
                binding.agent_id.as_str(),
                serde_json::to_string(&binding.mounted_pool_ids)?,
                binding.default_model_key,
                serde_json::to_string(&binding.options)?,
                binding_status_to_str(binding.status),
                binding.last_hit_at,
                now
            ],
        )?;
        Ok(())
    }

    pub fn save_agent_stash(
        &self,
        master_key: &[u8; 32],
        agent_id: TargetAgentId,
        file_path: &str,
        original_existed: bool,
        original_content: Option<&[u8]>,
    ) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        // 首次备份后不再覆盖原值 Stash，确保多次切换后仍能还原最初始配置
        let already: bool = conn
            .query_row(
                "SELECT 1 FROM agent_config_stashes WHERE agent_id = ?1 AND file_path = ?2",
                params![agent_id.as_str(), file_path],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if already {
            return Ok(());
        }

        let encrypted: Option<Vec<u8>> = match original_content {
            Some(bytes) => Some(crypto::encrypt_bytes_with_aad(
                master_key,
                &stash_aad(agent_id, file_path),
                bytes,
            )?),
            None => None,
        };
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO agent_config_stashes (
                agent_id, file_path, original_existed, original_content_encrypted, captured_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                agent_id.as_str(),
                file_path,
                original_existed as i64,
                encrypted,
                now
            ],
        )?;
        Ok(())
    }

    pub fn get_agent_stashes(
        &self,
        master_key: &[u8; 32],
        agent_id: TargetAgentId,
    ) -> Result<Vec<RestoredStashFile>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT file_path, original_existed, original_content_encrypted
             FROM agent_config_stashes WHERE agent_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![agent_id.as_str()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)? != 0,
                    r.get::<_, Option<Vec<u8>>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut out = Vec::with_capacity(rows.len());
        for (file_path, original_existed, blob_opt) in rows {
            let original_content = match blob_opt {
                Some(blob) => Some(crypto::decrypt_bytes_with_aad(
                    master_key,
                    &stash_aad(agent_id, &file_path),
                    &blob,
                )?),
                None => None,
            };
            out.push(RestoredStashFile {
                file_path,
                original_existed,
                original_content,
            });
        }
        Ok(out)
    }

    pub fn delete_agent_stashes(&self, agent_id: TargetAgentId) -> Result<(), StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "DELETE FROM agent_config_stashes WHERE agent_id = ?1",
            params![agent_id.as_str()],
        )?;
        Ok(())
    }

    pub fn append_routing_trace(&self, trace: &RoutingTraceItem) -> Result<(), StorageError> {
        let now = Utc::now().to_rfc3339();
        let status_str = match trace.status {
            RoutingTraceStatus::Success => "success",
            RoutingTraceStatus::FailoverSuccess => "failover_success",
            RoutingTraceStatus::Failed => "failed",
        };
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        conn.execute(
            "INSERT INTO gateway_request_logs (
                trace_id, agent_id, pool_id, pool_name, incoming_protocol,
                upstream_protocol, requested_model, final_account_label,
                final_tool_id, final_model, status, cache_affinity_hit,
                ttft_ms, total_tokens, attempts_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                trace.id,
                trace.agent_id.as_str(),
                trace.pool_id,
                trace.pool_name,
                protocol_to_str(trace.incoming_protocol),
                protocol_to_str(trace.upstream_protocol),
                trace.requested_model,
                trace.final_account_label,
                trace.final_tool_id,
                trace.final_model,
                status_str,
                trace.cache_affinity_hit as i64,
                trace.ttft_ms as i64,
                trace.total_tokens as i64,
                serde_json::to_string(&trace.attempts)?,
                now
            ],
        )?;
        let _ = conn.execute(
            "DELETE FROM gateway_request_logs
             WHERE id NOT IN (SELECT id FROM gateway_request_logs ORDER BY id DESC LIMIT 2000)",
            [],
        );
        Ok(())
    }

    pub fn list_routing_traces(&self, limit: usize) -> Result<Vec<RoutingTraceItem>, StorageError> {
        let conn = self.conn.lock().map_err(|_| StorageError::LockPoisoned)?;
        let mut stmt = conn.prepare(
            "SELECT trace_id, created_at, agent_id, pool_id, pool_name,
                    incoming_protocol, upstream_protocol, requested_model,
                    final_account_label, final_tool_id, final_model, status,
                    cache_affinity_hit, ttft_ms, total_tokens, attempts_json
             FROM gateway_request_logs
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit.max(1) as i64], |r| {
                let agent_raw: String = r.get(2)?;
                let agent_id =
                    TargetAgentId::from_id(&agent_raw).unwrap_or(TargetAgentId::Opencode);
                let created_at: String = r.get(1)?;
                let timestamp = created_at
                    .get(11..19)
                    .map(str::to_string)
                    .unwrap_or(created_at);
                let status = match r.get::<_, String>(11)?.as_str() {
                    "failover_success" => RoutingTraceStatus::FailoverSuccess,
                    "failed" => RoutingTraceStatus::Failed,
                    _ => RoutingTraceStatus::Success,
                };
                let attempts: Vec<RoutingAttempt> =
                    serde_json::from_str(&r.get::<_, String>(15)?).unwrap_or_default();
                Ok(RoutingTraceItem {
                    id: r.get(0)?,
                    timestamp,
                    agent_id,
                    pool_id: r.get(3)?,
                    pool_name: r.get(4)?,
                    incoming_protocol: parse_protocol(&r.get::<_, String>(5)?),
                    upstream_protocol: parse_protocol(&r.get::<_, String>(6)?),
                    requested_model: r.get(7)?,
                    final_account_label: r.get(8)?,
                    final_tool_id: r.get(9)?,
                    final_model: r.get(10)?,
                    status,
                    cache_affinity_hit: r.get::<_, i64>(12)? != 0,
                    ttft_ms: r.get::<_, i64>(13)?.max(0) as u64,
                    total_tokens: r.get::<_, i64>(14)?.max(0) as u64,
                    attempts,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CredentialEnvelope;

    fn temp_db(name: &str) -> (QwitchDatabase, [u8; 32], std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("qwitch-pool-db-{name}-{}", Uuid::new_v4()));
        let db = QwitchDatabase::open(&dir.join("qwitch.db")).expect("open temp db");
        (db, [9u8; 32], dir)
    }

    fn seed_account(
        db: &QwitchDatabase,
        key: &[u8; 32],
        tool_id: &str,
        uid: &str,
        label: &str,
    ) -> String {
        let env = CredentialEnvelope {
            tool_id: tool_id.to_string(),
            uid: uid.to_string(),
            display_name: Some(label.to_string()),
            display_email: None,
            payload: CredentialPayload::Token {
                access_token: format!("tok-{uid}"),
                refresh_token: Some(format!("rt-{uid}")),
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        db.upsert_account(key, Some(label), &env)
            .expect("seed account")
            .id
    }

    #[test]
    fn pool_crud_merges_models_enforces_api_key_tier_and_guards_codearts() {
        let (db, key, dir) = temp_db("crud");
        let wb_id = seed_account(&db, &key, "workbuddy", "wb_uid_1", "WB主号");
        let ca_id = seed_account(&db, &key, "codearts", "ca_uid_1", "码道主号");
        let ak_id = seed_account(&db, &key, "custom_api_key", "sk_uid_1", "API兜底");

        // client_readonly CodeArts 不允许直接入池
        let bad = db.create_pool(
            &key,
            &CreatePoolInput {
                id: None,
                name: "Bad".into(),
                description: "".into(),
                scene_tag: "测试".into(),
                strategy: PoolStrategy::QuotaFirst,
                affinity_mode: AffinityMode::AutoCache,
                skip_active_client_account: false,
                cooldown_seconds: 60,
                auto_refresh_on_401: true,
                fallback_api_key_enabled: true,
                members: vec![CreatePoolMemberInput {
                    account_id: ca_id.clone(),
                    supported_models: vec!["openPangu-Coder-Pro".into()],
                    tier: MemberTier::T1Primary,
                }],
                enabled_model_ids: None,
            },
        );
        assert!(bad.is_err());

        // 签发独立 OAuth 后解锁
        db.authorize_codearts_isolated(&key, &ca_id)
            .expect("unlock");

        let pool = db
            .create_pool(
                &key,
                &CreatePoolInput {
                    id: None,
                    name: "主力多模型池".into(),
                    description: "全量测试".into(),
                    scene_tag: "Coding".into(),
                    strategy: PoolStrategy::QuotaFirst,
                    affinity_mode: AffinityMode::AutoCache,
                    skip_active_client_account: false,
                    cooldown_seconds: 60,
                    auto_refresh_on_401: true,
                    fallback_api_key_enabled: true,
                    members: vec![
                        CreatePoolMemberInput {
                            account_id: wb_id.clone(),
                            supported_models: vec!["deepseek-v4-pro".into()],
                            tier: MemberTier::T1Primary,
                        },
                        CreatePoolMemberInput {
                            account_id: ak_id.clone(),
                            supported_models: vec!["deepseek-v4-pro".into()],
                            tier: MemberTier::T1Primary, // 强锁 T3
                        },
                    ],
                    enabled_model_ids: None,
                },
            )
            .expect("create pool");

        assert_eq!(pool.members[1].tier, MemberTier::T3Fallback);

        // 追加同一账号的第二个模型 -> 合并而不是重复插行
        db.add_members_to_pool(
            &key,
            &pool.id,
            &[
                CreatePoolMemberInput {
                    account_id: wb_id.clone(),
                    supported_models: vec!["glm-5.3".into()],
                    tier: MemberTier::T1Primary,
                },
                CreatePoolMemberInput {
                    account_id: ca_id,
                    supported_models: vec!["openPangu-Coder-Pro".into()],
                    tier: MemberTier::T2Backup,
                },
            ],
        )
        .expect("merge & add");

        let pools = db.list_pools(&key, &HashMap::new()).expect("list pools");
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].members.len(), 3);
        let wb_member = pools[0]
            .members
            .iter()
            .find(|m| m.account_id == wb_id)
            .unwrap();
        assert_eq!(
            wb_member.supported_models,
            vec!["deepseek-v4-pro".to_string(), "glm-5.3".to_string()]
        );
        let ca_member = pools[0]
            .members
            .iter()
            .find(|m| m.tool_id == "codearts")
            .unwrap();
        assert_eq!(
            ca_member.supported_models,
            vec!["openpangu-coder-pro".to_string()]
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn encrypted_stash_roundtrip_preserves_first_snapshot() {
        let (db, key, dir) = temp_db("stash");
        db.save_agent_stash(
            &key,
            TargetAgentId::Dsh,
            "/tmp/test-dsh/.env",
            true,
            Some(b"DEEPSEEK_API_KEY=sk-orig\n"),
        )
        .expect("save stash");
        // 再次保存同一路径不应覆盖原值
        db.save_agent_stash(
            &key,
            TargetAgentId::Dsh,
            "/tmp/test-dsh/.env",
            true,
            Some(b"OVERWRITTEN"),
        )
        .expect("ignore second");

        let restored = db
            .get_agent_stashes(&key, TargetAgentId::Dsh)
            .expect("get stashes");
        assert_eq!(restored.len(), 1);
        assert!(restored[0].original_existed);
        assert_eq!(
            restored[0].original_content.as_deref(),
            Some(b"DEEPSEEK_API_KEY=sk-orig\n".as_slice())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn agent_dedicated_pool_supports_atomic_tool_models_clear_and_primary_promotion() {
        let (db, key, dir) = temp_db("agent_atomic");
        let wb1 = seed_account(&db, &key, "workbuddy", "wb_u1", "WB主号");
        let wb2 = seed_account(&db, &key, "workbuddy", "wb_u2", "WB副号");
        let qd1 = seed_account(&db, &key, "qoder_cn", "qd_u1", "Qoder主号");

        let (pool_id, _) = db
            .ensure_agent_dedicated_pool(&key, TargetAgentId::CodexDesktop)
            .expect("ensure pool");
        assert_eq!(pool_id, "pool-agent-codex_desktop");

        db.set_pool_tool_models(
            &key,
            &pool_id,
            "workbuddy",
            &["deepseek-v4-pro".into(), "glm-5.3".into()],
        )
        .expect("set workbuddy models");
        db.set_pool_tool_models(&key, &pool_id, "qoder_cn", &["qwen4-max".into()])
            .expect("set qoder models");

        db.update_pool_tool_account_schedule(
            &key,
            &pool_id,
            "workbuddy",
            &[wb2.clone(), wb1.clone()],
            std::slice::from_ref(&wb2),
            Some(PoolStrategy::Order),
            Some(true),
            Some(90),
        )
        .expect("promote wb2 to primary and disable wb1");

        let pools = db.list_pools(&key, &HashMap::new()).expect("list");
        let pool = pools.iter().find(|p| p.id == pool_id).unwrap();
        assert_eq!(pool.strategy, PoolStrategy::Order);
        assert_eq!(pool.cooldown_seconds, 90);
        let wb_members: Vec<&PoolMember> = pool
            .members
            .iter()
            .filter(|m| m.tool_id == "workbuddy")
            .collect();
        assert_eq!(wb_members[0].account_id, wb2);
        assert!(wb_members[0].enabled);
        assert_eq!(wb_members[1].account_id, wb1);
        assert!(!wb_members[1].enabled);

        // 清空 workbuddy 模型，不影响 qoder_cn 模型和 wb 账号队列顺序
        db.set_pool_tool_models(&key, &pool_id, "workbuddy", &[])
            .expect("clear workbuddy models");
        let pools_after = db
            .list_pools(&key, &HashMap::new())
            .expect("list after clear");
        let pool_after = pools_after.iter().find(|p| p.id == pool_id).unwrap();
        let policy_ids: Vec<&str> = pool_after
            .model_policies
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.model_id.as_str())
            .collect();
        assert_eq!(policy_ids, vec!["qoder_cn/qwen4-max"]);
        assert!(pool_after.members.iter().any(|m| m.account_id == qd1));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn create_pool_dedupes_duplicate_members_without_partial_rows() {
        let (db, key, dir) = temp_db("dedupe");
        let wb = seed_account(&db, &key, "workbuddy", "wb_dedupe", "WB");
        let input = CreatePoolInput {
            id: None,
            name: "Dedupe Pool".into(),
            description: String::new(),
            scene_tag: "Coding".into(),
            strategy: PoolStrategy::Order,
            affinity_mode: AffinityMode::AutoCache,
            skip_active_client_account: false,
            cooldown_seconds: 60,
            auto_refresh_on_401: false,
            fallback_api_key_enabled: false,
            members: vec![
                CreatePoolMemberInput {
                    account_id: wb.clone(),
                    supported_models: vec!["deepseek-v4-pro".into()],
                    tier: MemberTier::T1Primary,
                },
                CreatePoolMemberInput {
                    account_id: wb.clone(),
                    supported_models: vec!["deepseek-v4-pro".into()],
                    tier: MemberTier::T1Primary,
                },
            ],
            enabled_model_ids: None,
        };
        let pool = db
            .create_pool(&key, &input)
            .expect("create pool with duplicate member");
        assert_eq!(pool.members.len(), 1, "duplicate account must be deduped");
        let pools = db.list_pools(&key, &HashMap::new()).expect("list pools");
        let stored = pools
            .iter()
            .find(|p| p.id == pool.id)
            .expect("pool present");
        assert_eq!(
            stored.members.len(),
            1,
            "no partial duplicate row persisted"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn increment_pool_stats_resets_on_new_day() {
        let (db, key, dir) = temp_db("statsday");
        let wb = seed_account(&db, &key, "workbuddy", "wb_stats", "WB");
        let pool = db
            .create_pool(
                &key,
                &CreatePoolInput {
                    id: None,
                    name: "Stats Pool".into(),
                    description: String::new(),
                    scene_tag: "Coding".into(),
                    strategy: PoolStrategy::Order,
                    affinity_mode: AffinityMode::AutoCache,
                    skip_active_client_account: false,
                    cooldown_seconds: 60,
                    auto_refresh_on_401: false,
                    fallback_api_key_enabled: false,
                    members: vec![CreatePoolMemberInput {
                        account_id: wb,
                        supported_models: vec!["deepseek-v4-pro".into()],
                        tier: MemberTier::T1Primary,
                    }],
                    enabled_model_ids: None,
                },
            )
            .unwrap();
        // 模拟「昨天」已累计 99 次
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE account_pools SET today_requests = 99, stats_date = '2000-01-01' WHERE id = ?1",
                params![pool.id],
            )
            .unwrap();
        }
        db.increment_pool_stats(&pool.id, false, true).unwrap();
        let pools = db.list_pools(&key, &HashMap::new()).unwrap();
        let p = pools.iter().find(|p| p.id == pool.id).unwrap();
        assert_eq!(
            p.today_requests, 1,
            "stale stats_date must reset before increment"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
