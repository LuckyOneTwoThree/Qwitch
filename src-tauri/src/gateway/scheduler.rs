use std::collections::HashMap;

use crate::domain::{
    AccountPool, AffinityMode, MemberTier, PoolMember, PoolStrategy, RoutingAttempt,
    RoutingAttemptOutcome,
};

#[derive(Debug, Clone)]
pub struct SchedulePlan {
    pub effective_model: String,
    pub used_explicit_fallback: bool,
    pub cache_affinity_hit: bool,
    pub candidates: Vec<PoolMember>,
    pub skipped_attempts: Vec<RoutingAttempt>,
    pub diagnostic_reason: Option<String>,
}

fn sort_members_in_tier(
    mut tier_members: Vec<PoolMember>,
    strategy: PoolStrategy,
    today_requests: u64,
) -> Vec<PoolMember> {
    tier_members.sort_by_key(|m| m.priority);
    if tier_members.len() <= 1 {
        return tier_members;
    }

    match strategy {
        PoolStrategy::QuotaFirst => {
            // 跨工具严守编排顺位（不盲比异构百分比），仅同工具内按剩余水位降序择优
            let mut first_prio_by_tool: HashMap<String, usize> = HashMap::new();
            for m in &tier_members {
                first_prio_by_tool
                    .entry(m.tool_id.clone())
                    .or_insert(m.priority);
            }
            tier_members.sort_by(|a, b| {
                if a.tool_id == b.tool_id {
                    b.quota_percent
                        .cmp(&a.quota_percent)
                        .then_with(|| a.priority.cmp(&b.priority))
                } else {
                    let pa = first_prio_by_tool
                        .get(&a.tool_id)
                        .copied()
                        .unwrap_or(a.priority);
                    let pb = first_prio_by_tool
                        .get(&b.tool_id)
                        .copied()
                        .unwrap_or(b.priority);
                    pa.cmp(&pb)
                }
            });
            tier_members
        }
        PoolStrategy::RoundRobin => {
            let offset = (today_requests as usize) % tier_members.len();
            tier_members.rotate_left(offset);
            tier_members
        }
        PoolStrategy::Drain | PoolStrategy::Order => tier_members,
    }
}

fn collect_for_model(
    pool: &AccountPool,
    target_model: &str,
    cooldowns: &HashMap<String, i64>,
    now_ms: i64,
    skipped_attempts: &mut Vec<RoutingAttempt>,
) -> Vec<PoolMember> {
    let mut t1 = Vec::new();
    let mut t2 = Vec::new();
    let mut t3 = Vec::new();
    let (scoped_tid, bare_target) = crate::domain::split_scoped_model_id(target_model);

    for m in &pool.members {
        if let Some(tid) = scoped_tid {
            if m.tool_id != tid {
                continue;
            }
        }
        if !m.supported_models.iter().any(|id| {
            id == target_model || crate::domain::split_scoped_model_id(id).1 == bare_target
        }) {
            continue;
        }
        if !m.enabled {
            continue;
        }
        if m.tool_id == "custom_api_key" && !pool.fallback_api_key_enabled {
            continue;
        }
        if matches!(
            m.account_status.as_deref(),
            Some("risk_paused") | Some("expired") | Some("disabled")
        ) {
            continue;
        }
        // 码道有专用签名上游，只读凭据同样可被调度（不再跳过）
        let runtime_cool = cooldowns
            .get(&m.account_id)
            .copied()
            .or(m.cooldown_until_ms);
        if runtime_cool.is_some_and(|until| until > now_ms) {
            continue;
        }
        // 「跳过桌面端活跃账号」是为切号流程准备的：切号会重启客户端、和网关抢同一份凭据。
        // 码道（STS 签名）、有道龙虾（OAuth Bearer /api/proxy）、Qoder CN（COSY 签名 API）
        // 均支持与桌面端并行请求而不互踢，不能因此把它们唯一的可用账号排掉。
        if pool.skip_active_client_account
            && m.is_active_on_client.unwrap_or(false)
            && !matches!(
                m.tool_id.as_str(),
                "codearts" | "lobsterai" | "qoder_cn" | "doubaowork"
            )
        {
            skipped_attempts.push(RoutingAttempt {
                account_label: m.label.clone(),
                tool_id: m.tool_id.clone(),
                model: target_model.to_string(),
                tier: Some(m.tier),
                outcome: RoutingAttemptOutcome::SkippedActive,
                latency_ms: 1,
                detail: "开启了「跳过桌面端活跃账号」护栏，自动避让以免与官方客户端互踢".into(),
            });
            continue;
        }
        match m.tier {
            MemberTier::T1Primary => t1.push(m.clone()),
            MemberTier::T2Backup => t2.push(m.clone()),
            MemberTier::T3Fallback => t3.push(m.clone()),
        }
    }

    let mut out = sort_members_in_tier(t1, pool.strategy, pool.today_requests);
    out.extend(sort_members_in_tier(t2, pool.strategy, pool.today_requests));
    out.extend(sort_members_in_tier(t3, pool.strategy, pool.today_requests));
    out
}

pub fn select_candidates_for_request(
    pool: &AccountPool,
    requested_model: &str,
    session_key: Option<&str>,
    cooldowns: &HashMap<String, i64>,
    affinity_map: &HashMap<String, String>,
    now_ms: i64,
) -> SchedulePlan {
    let mut skipped_attempts = Vec::new();
    let mut effective_model = requested_model.to_string();
    let mut candidates = collect_for_model(
        pool,
        &effective_model,
        cooldowns,
        now_ms,
        &mut skipped_attempts,
    );
    let policy = pool
        .model_policies
        .iter()
        .find(|p| p.model_id == requested_model);
    let mut used_explicit_fallback = false;

    if candidates.is_empty() {
        if let Some(fb) = policy.and_then(|p| p.fallback_model_id.as_deref()) {
            let mut fb_skipped = Vec::new();
            let fb_candidates = collect_for_model(pool, fb, cooldowns, now_ms, &mut fb_skipped);
            if !fb_candidates.is_empty() {
                used_explicit_fallback = true;
                effective_model = fb.to_string();
                candidates = fb_candidates;
                skipped_attempts.extend(fb_skipped);
            }
        }
    }

    let mut cache_affinity_hit = false;
    if pool.affinity_mode != AffinityMode::Off && !candidates.is_empty() {
        let aff_key = format!(
            "{}::{}::{}",
            pool.id,
            effective_model,
            session_key.unwrap_or("default")
        );
        if let Some(sticky_id) = affinity_map.get(&aff_key) {
            if let Some(idx) = candidates.iter().position(|c| &c.account_id == sticky_id) {
                cache_affinity_hit = true;
                if idx > 0 {
                    let chosen = candidates.remove(idx);
                    candidates.insert(0, chosen);
                }
            }
        }
    }

    let diagnostic_reason = if candidates.is_empty() {
        Some(match policy.and_then(|p| p.fallback_model_id.as_deref()) {
            Some(fb) => {
                format!("主模型「{requested_model}」与显式降级模型「{fb}」当前均无可用账号")
            }
            None => {
                if !skipped_attempts.is_empty()
                    && skipped_attempts
                        .iter()
                        .all(|a| a.outcome == RoutingAttemptOutcome::SkippedActive)
                {
                    format!(
                        "严格锁模「{requested_model}」：可用账号均被「跳过桌面端活跃账号」护栏避让（请在 Agent 调度页关闭该开关或导入第二个账号）"
                    )
                } else {
                    format!(
                        "严格锁模「{requested_model}」：所属账号当前均处于冷却、禁用或未启用 API Key 兜底（网关已拒绝私自换模型）"
                    )
                }
            }
        })
    } else {
        None
    };

    SchedulePlan {
        effective_model,
        used_explicit_fallback,
        cache_affinity_hit,
        candidates,
        skipped_attempts,
        diagnostic_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{PoolModelPolicy, UpstreamChannel};

    fn make_member(
        id: &str,
        tool_id: &str,
        tier: MemberTier,
        priority: usize,
        quota_percent: u8,
        models: &[&str],
    ) -> PoolMember {
        PoolMember {
            id: id.into(),
            account_id: id.into(),
            tool_id: tool_id.into(),
            label: id.into(),
            uid_masked: "u***1".into(),
            supported_models: models.iter().map(|s| (*s).into()).collect(),
            tier,
            channel: UpstreamChannel::OpencodeBridge,
            enabled: true,
            priority,
            quota_percent,
            quota_text: "".into(),
            cooldown_until_ms: None,
            is_active_on_client: Some(false),
            account_status: Some("active".into()),
            code_arts_mode: None,
            last_latency_ms: Some(400),
        }
    }

    #[test]
    fn respects_tier_order_and_compares_quota_only_within_same_tool() {
        let pool = AccountPool {
            id: "p1".into(),
            slug: "p1".into(),
            name: "测试池".into(),
            description: "".into(),
            scene_tag: "Coding".into(),
            enabled: true,
            strategy: PoolStrategy::QuotaFirst,
            affinity_mode: AffinityMode::AutoCache,
            skip_active_client_account: false,
            cooldown_seconds: 60,
            auto_refresh_on_401: true,
            fallback_api_key_enabled: true,
            model_policies: vec![PoolModelPolicy {
                model_id: "claude-sonnet-4.6".into(),
                enabled: true,
                fallback_model_id: None,
                claude_alias_id: None,
            }],
            members: vec![
                // T1 WorkBuddy low quota comes first in priority, Trae high quota in T1, WorkBuddy higher quota #3
                make_member(
                    "wb-low",
                    "workbuddy",
                    MemberTier::T1Primary,
                    1,
                    30,
                    &["claude-sonnet-4.6"],
                ),
                make_member(
                    "trae-high",
                    "trae_cn",
                    MemberTier::T1Primary,
                    2,
                    99,
                    &["claude-sonnet-4.6"],
                ),
                make_member(
                    "wb-high",
                    "workbuddy",
                    MemberTier::T1Primary,
                    3,
                    85,
                    &["claude-sonnet-4.6"],
                ),
                make_member(
                    "api-key",
                    "custom_api_key",
                    MemberTier::T3Fallback,
                    4,
                    100,
                    &["claude-sonnet-4.6"],
                ),
            ],
            today_requests: 0,
            today_failovers: 0,
            cache_hit_rate: 0,
            created_at: "".into(),
        };

        let plan = select_candidates_for_request(
            &pool,
            "claude-sonnet-4.6",
            None,
            &HashMap::new(),
            &HashMap::new(),
            1000,
        );
        let order: Vec<&str> = plan
            .candidates
            .iter()
            .map(|c| c.account_id.as_str())
            .collect();
        // WorkBuddy stays ahead of Trae per tool priority (#1), while inside WorkBuddy wb-high (85%) beats wb-low (30%)!
        // API Key stays strictly at T3!
        assert_eq!(order, vec!["wb-high", "wb-low", "trae-high", "api-key"]);
    }

    #[test]
    fn enforces_strict_model_lock_unless_explicit_fallback_exists() {
        let mut pool = AccountPool {
            id: "p1".into(),
            slug: "p1".into(),
            name: "锁模测试".into(),
            description: "".into(),
            scene_tag: "Coding".into(),
            enabled: true,
            strategy: PoolStrategy::Order,
            affinity_mode: AffinityMode::Off,
            skip_active_client_account: false,
            cooldown_seconds: 60,
            auto_refresh_on_401: true,
            fallback_api_key_enabled: false,
            model_policies: vec![
                PoolModelPolicy {
                    model_id: "claude-opus-4.6".into(),
                    enabled: true,
                    fallback_model_id: None,
                    claude_alias_id: None,
                },
                PoolModelPolicy {
                    model_id: "claude-sonnet-4.6".into(),
                    enabled: true,
                    fallback_model_id: None,
                    claude_alias_id: None,
                },
            ],
            members: vec![
                make_member(
                    "opus-acc",
                    "workbuddy",
                    MemberTier::T1Primary,
                    1,
                    50,
                    &["claude-opus-4.6"],
                ),
                make_member(
                    "sonnet-acc",
                    "workbuddy",
                    MemberTier::T1Primary,
                    2,
                    90,
                    &["claude-sonnet-4.6"],
                ),
            ],
            today_requests: 0,
            today_failovers: 0,
            cache_hit_rate: 0,
            created_at: "".into(),
        };

        let mut cooldowns = HashMap::new();
        cooldowns.insert("opus-acc".into(), 999_999);

        let strict_plan = select_candidates_for_request(
            &pool,
            "claude-opus-4.6",
            None,
            &cooldowns,
            &HashMap::new(),
            1000,
        );
        assert!(strict_plan.candidates.is_empty());
        assert!(strict_plan.diagnostic_reason.unwrap().contains("严格锁模"));

        // 配置显式降级后才允许fallback
        pool.model_policies[0].fallback_model_id = Some("claude-sonnet-4.6".into());
        let fb_plan = select_candidates_for_request(
            &pool,
            "claude-opus-4.6",
            None,
            &cooldowns,
            &HashMap::new(),
            1000,
        );
        assert!(fb_plan.used_explicit_fallback);
        assert_eq!(fb_plan.effective_model, "claude-sonnet-4.6");
        assert_eq!(fb_plan.candidates[0].account_id, "sonnet-acc");
    }

    #[test]
    fn scoped_request_filters_strictly_within_target_tool_accounts() {
        let pool = AccountPool {
            id: "pool-agent-codex_desktop".into(),
            slug: "codex_desktop".into(),
            name: "Codex 专属".into(),
            description: "".into(),
            scene_tag: "Agent".into(),
            enabled: true,
            strategy: PoolStrategy::Order,
            affinity_mode: AffinityMode::AutoCache,
            skip_active_client_account: false,
            cooldown_seconds: 60,
            auto_refresh_on_401: true,
            fallback_api_key_enabled: false,
            model_policies: vec![PoolModelPolicy {
                model_id: "qoder_cn/deepseek-v4-pro".into(),
                enabled: true,
                fallback_model_id: None,
                claude_alias_id: None,
            }],
            members: vec![
                make_member(
                    "wb-1",
                    "workbuddy",
                    MemberTier::T1Primary,
                    1,
                    95,
                    &["deepseek-v4-pro"],
                ),
                make_member(
                    "qd-1",
                    "qoder_cn",
                    MemberTier::T1Primary,
                    2,
                    40,
                    &["deepseek-v4-pro"],
                ),
                make_member(
                    "qd-2",
                    "qoder_cn",
                    MemberTier::T1Primary,
                    3,
                    75,
                    &["deepseek-v4-pro"],
                ),
            ],
            today_requests: 0,
            today_failovers: 0,
            cache_hit_rate: 0,
            created_at: "".into(),
        };

        let plan = select_candidates_for_request(
            &pool,
            "qoder_cn/deepseek-v4-pro",
            None,
            &HashMap::new(),
            &HashMap::new(),
            1000,
        );
        let ids: Vec<&str> = plan
            .candidates
            .iter()
            .map(|c| c.account_id.as_str())
            .collect();
        assert_eq!(ids, vec!["qd-1", "qd-2"]);
    }

    /// 回归：码道只读凭据 + 桌面端活跃，两个历史护栏都不能把它排掉，
    /// 否则请求会直接 503「所属账号当前均处于冷却、禁用」。
    #[test]
    fn codearts_readonly_active_on_client_is_still_schedulable() {
        let mut member = make_member(
            "ca-1",
            "codearts",
            MemberTier::T1Primary,
            1,
            80,
            &["codearts/deepseek-v4.1-flash-1017"],
        );
        member.is_active_on_client = Some(true);
        member.code_arts_mode = Some(crate::domain::CodeArtsAuthMode::ClientReadonly);

        let pool = AccountPool {
            id: "p-codearts".into(),
            slug: "p-codearts".into(),
            name: "码道池".into(),
            description: String::new(),
            scene_tag: "Coding".into(),
            enabled: true,
            strategy: PoolStrategy::Order,
            affinity_mode: AffinityMode::Off,
            skip_active_client_account: true,
            cooldown_seconds: 60,
            auto_refresh_on_401: true,
            fallback_api_key_enabled: false,
            model_policies: Vec::new(),
            members: vec![member],
            today_requests: 0,
            today_failovers: 0,
            cache_hit_rate: 0,
            created_at: String::new(),
        };

        let plan = select_candidates_for_request(
            &pool,
            "codearts/deepseek-v4.1-flash-1017",
            None,
            &HashMap::new(),
            &HashMap::new(),
            1_000,
        );
        assert_eq!(
            plan.candidates.len(),
            1,
            "码道必须可被调度：{:?}",
            plan.diagnostic_reason
        );
        assert_eq!(plan.candidates[0].account_id, "ca-1");
    }

    /// 对照：非码道工具仍然遵守「跳过桌面端活跃账号」护栏。
    #[test]
    fn active_on_client_still_skips_other_tools() {
        let mut member = make_member(
            "wb-1",
            "workbuddy",
            MemberTier::T1Primary,
            1,
            80,
            &["claude-sonnet-4.6"],
        );
        member.is_active_on_client = Some(true);
        let pool = AccountPool {
            id: "p-wb".into(),
            slug: "p-wb".into(),
            name: "WB".into(),
            description: String::new(),
            scene_tag: "Coding".into(),
            enabled: true,
            strategy: PoolStrategy::Order,
            affinity_mode: AffinityMode::Off,
            skip_active_client_account: true,
            cooldown_seconds: 60,
            auto_refresh_on_401: true,
            fallback_api_key_enabled: false,
            model_policies: Vec::new(),
            members: vec![member],
            today_requests: 0,
            today_failovers: 0,
            cache_hit_rate: 0,
            created_at: String::new(),
        };
        let plan = select_candidates_for_request(
            &pool,
            "claude-sonnet-4.6",
            None,
            &HashMap::new(),
            &HashMap::new(),
            1_000,
        );
        assert!(plan.candidates.is_empty(), "桌面端活跃的普通账号仍应被跳过");
        assert!(
            plan.diagnostic_reason
                .as_deref()
                .unwrap_or("")
                .contains("跳过桌面端活跃账号"),
            "被活跃护栏跳过时应给出准确诊断：{:?}",
            plan.diagnostic_reason
        );
    }

    #[test]
    fn active_on_client_does_not_skip_lobsterai_or_qoder_cn() {
        for (tool_id, model_id) in [
            ("lobsterai", "lobsterai/deepseek-v4.1-flash"),
            ("qoder_cn", "qoder_cn/deepseek-v4.1-flash"),
        ] {
            let mut member =
                make_member("acc-1", tool_id, MemberTier::T1Primary, 1, 90, &[model_id]);
            member.is_active_on_client = Some(true);
            let pool = AccountPool {
                id: "p-exempt".into(),
                slug: "p-exempt".into(),
                name: "Exempt".into(),
                description: String::new(),
                scene_tag: "Coding".into(),
                enabled: true,
                strategy: PoolStrategy::Order,
                affinity_mode: AffinityMode::Off,
                skip_active_client_account: true,
                cooldown_seconds: 60,
                auto_refresh_on_401: true,
                fallback_api_key_enabled: false,
                model_policies: Vec::new(),
                members: vec![member],
                today_requests: 0,
                today_failovers: 0,
                cache_hit_rate: 0,
                created_at: String::new(),
            };
            let plan = select_candidates_for_request(
                &pool,
                model_id,
                None,
                &HashMap::new(),
                &HashMap::new(),
                1_000,
            );
            assert_eq!(
                plan.candidates.len(),
                1,
                "{tool_id} 唯一的活跃账号不应被跳过：{:?}",
                plan.diagnostic_reason
            );
        }
    }
}
