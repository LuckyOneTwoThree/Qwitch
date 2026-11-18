import type {
  AccountSummary,
  AdapterManifest,
  ClientHostStatus,
  ToolId,
} from "../types/qwitch";
import { getVisibleTools } from "./toolRegistry";

export type AccountFilter = "all" | "active_client" | "claimable" | "attention";

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
  const supportsClaim = adapter ? adapter.capabilities.supportsDailyClaim : true;
  return (
    supportsClaim &&
    acc.status === "active" &&
    acc.hasApiToken !== false &&
    acc.latestQuota?.claimStatus === "available"
  );
}

export function selectScopeAccounts(
  accounts: AccountSummary[],
  activeToolFilter?: ToolId
): AccountSummary[] {
  if (!activeToolFilter) return accounts;
  return accounts.filter((a) => a.toolId === activeToolFilter);
}

export interface ScopedStats {
  total: number;
  healthyCount: number;
  attentionCount: number;
  disabledCount: number;
  claimableCount: number;
  claimedCount: number;
  unclaimedAuthMissingCount: number;
  unknownClaimCount: number;
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
  let claimableCount = 0;
  let claimedCount = 0;
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
    const supportsClaim = adapter ? adapter.capabilities.supportsDailyClaim : true;

    if (supportsClaim && acc.status === "active") {
      if (acc.hasApiToken === false) {
        unclaimedAuthMissingCount += 1;
      } else if (acc.latestQuota?.claimStatus === "available") {
        claimableCount += 1;
      } else if (acc.latestQuota?.claimStatus === "claimed") {
        claimedCount += 1;
      } else if (!acc.latestQuota || acc.latestQuota.claimStatus === "unknown") {
        unknownClaimCount += 1;
      }
    }
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
    claimableCount,
    claimedCount,
    unclaimedAuthMissingCount,
    unknownClaimCount,
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
