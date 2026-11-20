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

/**
 * 平铺视图的排序优先级。
 *
 * 数值越小越靠前。顺序体现「额度管理器」的核心意图：**先处理需要我动手的**。
 */
export const FLAT_SORT_TIER = {
  /** 过期 / 风控暂停 / 未完成授权 —— 不处理就无法使用 */
  attention: 0,
  /** 今日可领取 —— 该去点一下 */
  claimable: 1,
  /** 本机客户端当前挂载 —— 正在使用中 */
  activeOnClient: 2,
  /** 其余正常账号 */
  normal: 3,
} as const;

/**
 * 平铺视图排序：语义优先，工具顺序与分组视图保持一致。
 *
 * 背景：平铺视图此前**不做任何排序**，直接继承后端
 * `ORDER BY tool_id ASC, is_active_on_client DESC, created_at ASC`
 * 的顺序。这带来两个实际问题：
 *
 * 1. 工具顺序是 `tool_id` 字典序（minimax → qoder → trae → workbuddy），
 *    与分组视图的 `BUILTIN_TOOL_ORDER`（trae → workbuddy → qoder → minimax）
 *    恰好相反。用户在两个视图间切换时所有卡片位置全变，观感混乱。
 * 2. `is_active_on_client` 的优先级在 `tool_id` 之后，导致「当前挂载的账号」
 *    会被字典序更靠前的工具挤到后面 —— 用户最该第一眼看到的信息被淹没。
 *
 * 本函数用**已存在的字段**（status / claimStatus / isActiveOnClient / toolId /
 * label）重新排序，不依赖后端改动，也不扩展 DTO：
 *
 * - 分层：待处理 → 可领取 → 挂载中 → 其余
 * - 层内：按 `BUILTIN_TOOL_ORDER` 排工具，与分组视图一致
 * - 工具内：保持传入顺序（后端已按 created_at 稳定排序，等于「先导入的在前」）
 *
 * 排序是**稳定**的：同层同工具内不重排，避免每次刷新卡片跳动。
 */
/**
 * `isAccountClaimable` 的适配器直传版本（内部使用，避免重复线性查找）。
 */
function isAccountClaimableWithAdapter(
  acc: AccountSummary,
  adapter: AdapterManifest | undefined
): boolean {
  const supportsClaim = adapter ? adapter.capabilities.supportsDailyClaim : false;
  return (
    supportsClaim &&
    acc.status === "active" &&
    acc.hasApiToken !== false &&
    getEffectiveClaimStatus(acc) === "available"
  );
}

export function sortAccountsForFlatView(
  accounts: AccountSummary[],
  adapters: AdapterManifest[]
): AccountSummary[] {
  const toolOrder = new Map<string, number>();
  getVisibleTools(adapters).forEach((tool, index) => {
    toolOrder.set(tool.id, index);
  });

  // 预建 toolId → adapter 索引：isAccountClaimable 内部会按 toolId 查找适配器，
  // 若直接传入 adapters 数组，每个账号都要线性扫描一次。账号量虽小，但排序
  // 在每次过滤/刷新时都会执行，索引化后开销从 O(n·m) 降为 O(n)。
  const adapterById = new Map(adapters.map((a) => [a.id, a]));
  const claimableCache = new Map<string, boolean>();

  const tierOf = (acc: AccountSummary): number => {
    // 已停用是用户主动关闭的，不需要提醒处理，归入 normal
    if (acc.status !== "disabled" && isAccountNeedingAttention(acc)) {
      return FLAT_SORT_TIER.attention;
    }
    // 同一账号的 claimable 判定在本次排序内只算一次
    let claimable = claimableCache.get(acc.id);
    if (claimable === undefined) {
      const adapter = adapterById.get(acc.toolId);
      claimable = isAccountClaimableWithAdapter(acc, adapter);
      claimableCache.set(acc.id, claimable);
    }
    if (claimable) {
      return FLAT_SORT_TIER.claimable;
    }
    if (acc.isActiveOnClient) {
      return FLAT_SORT_TIER.activeOnClient;
    }
    return FLAT_SORT_TIER.normal;
  };

  // 先记录原始下标，作为同层同工具内的稳定兜底
  const decorated = accounts.map((acc, index) => ({
    acc,
    index,
    tier: tierOf(acc),
    toolRank: toolOrder.get(acc.toolId) ?? Number.MAX_SAFE_INTEGER,
  }));

  decorated.sort((a, b) => {
    if (a.tier !== b.tier) return a.tier - b.tier;
    if (a.toolRank !== b.toolRank) return a.toolRank - b.toolRank;
    return a.index - b.index;
  });

  return decorated.map((d) => d.acc);
}
