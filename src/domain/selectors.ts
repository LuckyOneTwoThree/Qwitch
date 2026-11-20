import type {
  AccountSummary,
  AdapterManifest,
  ClaimStatus,
  ClientHostStatus,
  ToolId,
} from "../types/qwitch";
import { getVisibleTools } from "./toolRegistry";

export type AccountFilter = "all" | "active_client" | "claimable" | "attention";
export type ClaimActionState =
  | "claimable"
  | "verify_and_claim"
  | "all_claimed"
  | "no_eligible";

const CST_OFFSET_MS = 8 * 60 * 60 * 1000;

export function toCstDayKey(timestampMs: number): string {
  const d = new Date(timestampMs + CST_OFFSET_MS);
  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, "0");
  const day = String(d.getUTCDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

export function isSameCstDay(iso?: string): boolean {
  if (!iso) return false;
  const ms = Date.parse(iso);
  if (Number.isNaN(ms)) return false;
  return toCstDayKey(ms) === toCstDayKey(Date.now());
}

export function getEffectiveClaimStatus(acc: AccountSummary): ClaimStatus {
  const raw = acc.latestQuota?.claimStatus;
  if (raw === "unsupported") return "unsupported";

  // 与后端 normalize_daily_claim_status 同口径：仅每日重置型工具
  // 以 lastClaimedAt 今日为权威；Qoder 活动按周期，不做自然日覆盖
  const dailyResetTool =
    acc.toolId === "trae_cn" ||
    acc.toolId === "trae" ||
    acc.toolId === "workbuddy" ||
    acc.toolId === "minimax_code_cn";
  if (dailyResetTool && acc.lastClaimedAt && isSameCstDay(acc.lastClaimedAt)) {
    return "claimed";
  }

  if (!acc.latestQuota) {
    return "unknown";
  }

  if (raw === "claimed") {
    return isSameCstDay(acc.latestQuota.fetchedAt) || isSameCstDay(acc.lastClaimedAt)
      ? "claimed"
      : "unknown";
  }

  if (raw === "available") {
    return isSameCstDay(acc.latestQuota.fetchedAt) ? "available" : "unknown";
  }

  return "unknown";
}

export function isAccountNeedingAttention(acc: AccountSummary): boolean {
  if (acc.status === "disabled") return false;
  return (
    acc.status === "expired" ||
    acc.status === "risk_paused" ||
    acc.hasApiToken === false ||
    acc.hasLocalSnapshot === false
  );
}

export function isAccountHealthyReady(acc: AccountSummary): boolean {
  return (
    acc.status === "active" &&
    acc.hasApiToken !== false &&
    acc.hasLocalSnapshot !== false
  );
}

export function isAccountClaimable(
  acc: AccountSummary,
  adapters: AdapterManifest[]
): boolean {
  const adapter = adapters.find((a) => a.id === acc.toolId);
  // 与 AccountCard 同口径：适配器未加载时不计入可领
  const supportsClaim = adapter ? adapter.capabilities.supportsDailyClaim : false;
  return (
    supportsClaim &&
    acc.status === "active" &&
    acc.hasApiToken !== false &&
    getEffectiveClaimStatus(acc) === "available"
  );
}

export function selectScopeAccounts(
  accounts: AccountSummary[],
  activeToolFilter?: ToolId
): AccountSummary[] {
  if (!activeToolFilter) return accounts;
  return accounts.filter((a) => a.toolId === activeToolFilter);
}

export interface ClaimCandidateOptions {
  targetIds?: string[];
  toolId?: ToolId;
  /** 「核实并领取」显式流程允许 unknown 进入队列（后端先查后领，Fail-Closed）；
   *  自动巡检保持 false，仅处理 available */
  includeUnknown?: boolean;
}

export function selectClaimCandidates(
  accounts: AccountSummary[],
  adapters: AdapterManifest[],
  opts: ClaimCandidateOptions = {}
): AccountSummary[] {
  return accounts.filter((acc) => {
    if (opts.targetIds && !opts.targetIds.includes(acc.id)) return false;
    if (!opts.targetIds && opts.toolId && acc.toolId !== opts.toolId) return false;
    const adapter = adapters.find((a) => a.id === acc.toolId);
    if (
      acc.status !== "active" ||
      acc.hasApiToken === false ||
      !adapter?.capabilities.supportsDailyClaim
    ) {
      return false;
    }
    if (opts.targetIds) return true;
    const effectiveClaim = getEffectiveClaimStatus(acc);
    return (
      effectiveClaim === "available" ||
      (opts.includeUnknown === true && effectiveClaim === "unknown")
    );
  });
}

export interface ScopedStats {
  total: number;
  healthyCount: number;
  attentionCount: number;
  disabledCount: number;
  eligibleClaimTotal: number;
  claimableCount: number;
  claimedCount: number;
  unsupportedClaimCount: number;
  unclaimedAuthMissingCount: number;
  unknownClaimCount: number;
  allClaimed: boolean;
  claimActionState: ClaimActionState;
  claimBadgeText: string;
  activeClientsCount: number;
  scopeToolCapacity: number;
  runningClientsCount: number;
  filterCounts: Record<AccountFilter, number>;
}

export function selectScopedStats(
  scopeAccounts: AccountSummary[],
  adapters: AdapterManifest[],
  clientStatuses: Record<string, ClientHostStatus>,
  activeToolFilter?: ToolId
): ScopedStats {
  const visibleTools = getVisibleTools(adapters);
  const targetToolIds = activeToolFilter
    ? [activeToolFilter]
    : visibleTools.map((t) => t.id);

  let healthyCount = 0;
  let attentionCount = 0;
  let disabledCount = 0;
  let eligibleClaimTotal = 0;
  let claimableCount = 0;
  let claimedCount = 0;
  let unsupportedClaimCount = 0;
  let unclaimedAuthMissingCount = 0;
  let unknownClaimCount = 0;
  let activeClientsCount = 0;

  for (const acc of scopeAccounts) {
    if (acc.isActiveOnClient) {
      activeClientsCount += 1;
    }

    if (acc.status === "disabled") {
      disabledCount += 1;
    } else if (isAccountNeedingAttention(acc)) {
      attentionCount += 1;
    } else {
      healthyCount += 1;
    }

    const adapter = adapters.find((a) => a.id === acc.toolId);
    const supportsClaim = adapter ? adapter.capabilities.supportsDailyClaim : false;

    if (supportsClaim && acc.status === "active") {
      if (acc.hasApiToken === false) {
        unclaimedAuthMissingCount += 1;
      } else {
        const effectiveClaim = getEffectiveClaimStatus(acc);
        if (effectiveClaim === "unsupported") {
          unsupportedClaimCount += 1;
        } else {
          eligibleClaimTotal += 1;
          if (effectiveClaim === "available") {
            claimableCount += 1;
          } else if (effectiveClaim === "claimed") {
            claimedCount += 1;
          } else {
            unknownClaimCount += 1;
          }
        }
      }
    }
  }

  const allClaimed =
    claimedCount > 0 && claimableCount === 0 && unknownClaimCount === 0;

  let claimActionState: ClaimActionState = "no_eligible";
  if (claimableCount > 0) {
    claimActionState = "claimable";
  } else if (unknownClaimCount > 0) {
    claimActionState = "verify_and_claim";
  } else if (allClaimed) {
    claimActionState = "all_claimed";
  }

  let claimBadgeText = "无签到项";
  if (claimableCount > 0) {
    claimBadgeText = `${claimableCount} 待领`;
  } else if (unclaimedAuthMissingCount > 0) {
    claimBadgeText = `${unclaimedAuthMissingCount} 待授权`;
  } else if (unknownClaimCount > 0) {
    claimBadgeText = `${unknownClaimCount} 待核实`;
  } else if (claimedCount > 0) {
    claimBadgeText = "已全签";
  }

  const runningClientsCount = targetToolIds.filter(
    (tid) => Boolean(clientStatuses[tid]?.isRunning)
  ).length;

  return {
    total: scopeAccounts.length,
    healthyCount,
    attentionCount,
    disabledCount,
    eligibleClaimTotal,
    claimableCount,
    claimedCount,
    unsupportedClaimCount,
    unclaimedAuthMissingCount,
    unknownClaimCount,
    allClaimed,
    claimActionState,
    claimBadgeText,
    activeClientsCount,
    scopeToolCapacity: targetToolIds.length,
    runningClientsCount,
    filterCounts: {
      all: scopeAccounts.length,
      active_client: activeClientsCount,
      claimable: claimableCount,
      attention: attentionCount,
    },
  };
}
