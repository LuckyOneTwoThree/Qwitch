import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ToastTone } from "../types/qwitch";
import type {
  AccountPool,
  AgentBinding,
  AgentBindingOptions,
  GatewayRuntimeState,
  MemberTier,
  PoolMember,
  RoutingAttempt,
  RoutingTraceItem,
  RunPoolSimulationInput,
  RunPoolSimulationResult,
  TargetAgentId,
  UpstreamAccountCandidate,
  WireProtocolKind,
} from "../types/pool";
import {
  AVAILABLE_UPSTREAM_CANDIDATES,
  INITIAL_GATEWAY_STATE,
  INITIAL_POOLS,
  INITIAL_ROUTING_TRACES,
  MEMBER_TIER_META,
  TARGET_AGENT_ORDER,
  TOOL_SHORT_LABELS,
  UPSTREAM_MODEL_OPTIONS,
  buildAgentConfigSnippets,
  buildClaudeModelAlias,
  computeAgentExposedModels,
  createInitialAgentBindings,
  formatScopedModelId,
  getCatalogModelsForTool,
  makeDefaultModelKey,
  splitScopedModelId,
  syncPoolModelPolicies,
  type CatalogModelEntry,
} from "../data/mockPoolData";

export interface CreatePoolMemberInput {
  candidate: UpstreamAccountCandidate;
  supportedModels: string[];
  tier: MemberTier;
}

export interface CreatePoolInput {
  id?: string;
  name: string;
  description: string;
  sceneTag: string;
  strategy: AccountPool["strategy"];
  affinityMode: AccountPool["affinityMode"];
  skipActiveClientAccount: boolean;
  cooldownSeconds: number;
  autoRefreshOn401: boolean;
  fallbackApiKeyEnabled: boolean;
  members: CreatePoolMemberInput[];
  enabledModelIds?: string[];
}

export interface PoolPrototypeContextValue {
  isTauriRuntime: boolean;
  gateway: GatewayRuntimeState;
  pools: AccountPool[];
  selectedPoolId: string;
  setSelectedPoolId: (poolId: string) => void;
  candidates: UpstreamAccountCandidate[];
  agentBindings: AgentBinding[];
  traces: RoutingTraceItem[];
  nowMs: number;
  isRefreshingModels: boolean;
  refreshUpstreamModels: (accountId?: string) => Promise<void>;
  toggleGateway: () => void;
  updateGatewayPort: (port: number) => void;
  cycleBridgeRuntime: () => void;
  toggleProxySimulation: () => void;
  createPool: (input: CreatePoolInput) => AccountPool;
  updatePool: (poolId: string, patch: Partial<AccountPool>) => void;
  deletePool: (poolId: string) => void;
  togglePoolEnabled: (poolId: string) => void;
  reorderPoolMember: (poolId: string, memberId: string, direction: "up" | "down") => void;
  togglePoolMember: (poolId: string, memberId: string) => void;
  togglePoolMemberModel: (poolId: string, memberId: string, modelId: string) => void;
  updatePoolMemberTier: (poolId: string, memberId: string, tier: MemberTier) => void;
  togglePoolModelEnabled: (poolId: string, modelId: string) => void;
  setPoolModelFallback: (poolId: string, modelId: string, fallbackModelId: string | null) => void;
  addMembersToPool: (poolId: string, members: CreatePoolMemberInput[]) => void;
  removePoolMember: (poolId: string, memberId: string) => void;
  injectMemberCooldown: (poolId: string, memberId: string) => void;
  clearMemberCooldown: (poolId: string, memberId: string) => void;
  authorizeCodeArtsIsolated: (accountId: string) => void;
  updateAgentMountedPools: (
    agentId: TargetAgentId,
    mountedPoolIds: string[],
    nextDefaultModelKey?: string | null
  ) => void;
  setAgentDefaultModel: (agentId: TargetAgentId, modelKey: string) => void;
  mountPoolToAgents: (poolId: string, agentIds: TargetAgentId[]) => void;
  unmountPoolFromAgent: (agentId: TargetAgentId, poolId: string) => void;
  unbindAgent: (agentId: TargetAgentId) => void;
  repairAgentDrift: (agentId: TargetAgentId) => void;
  restartAgentClient: (agentId: TargetAgentId) => void;
  toggleAgentOption: (agentId: TargetAgentId, key: keyof AgentBindingOptions) => void;
  detectAgentEnvironment: () => void;
  runPoolSimulation: (input: RunPoolSimulationInput) => RunPoolSimulationResult;
  getAgentToolModelOptions: (toolId: string) => CatalogModelEntry[];
  getAgentToolEnabledModelIds: (agentId: TargetAgentId, toolId: string) => string[];
  getAgentToolAccounts: (agentId: TargetAgentId, toolId: string) => PoolMember[];
  getAgentPrimaryPool: (agentId: TargetAgentId) => AccountPool | null;
  setAgentToolModels: (agentId: TargetAgentId, toolId: string, modelIds: string[]) => void;
  selectAllAgentToolModels: (
    agentId: TargetAgentId,
    toolId: string,
    filterModelIds?: string[]
  ) => void;
  selectRecommendedAgentToolModels: (agentId: TargetAgentId, toolId: string) => void;
  selectLowRateAgentToolModels: (agentId: TargetAgentId, toolId: string) => void;
  invertAgentToolModels: (
    agentId: TargetAgentId,
    toolId: string,
    scopeModelIds?: string[]
  ) => void;
  clearAgentToolModels: (agentId: TargetAgentId, toolId: string) => void;
  setAgentDefaultToolModel: (agentId: TargetAgentId, toolId: string, modelId: string) => void;
  promoteAgentToolPrimaryAccount: (
    agentId: TargetAgentId,
    toolId: string,
    accountId: string
  ) => void;
  reorderAgentToolAccount: (
    agentId: TargetAgentId,
    toolId: string,
    accountId: string,
    direction: "up" | "down"
  ) => void;
  toggleAgentToolAccountEnabled: (
    agentId: TargetAgentId,
    toolId: string,
    accountId: string
  ) => void;
  setAllAgentToolAccountsEnabled: (
    agentId: TargetAgentId,
    toolId: string,
    onlyPrimary: boolean
  ) => void;
  sortAgentToolAccountsByQuota: (agentId: TargetAgentId, toolId: string) => void;
  updateAgentToolRoutingOptions: (
    agentId: TargetAgentId,
    toolId: string,
    patch: {
      strategy?: AccountPool["strategy"];
      skipActiveClientAccount?: boolean;
      cooldownSeconds?: number;
    }
  ) => void;
  syncToolScheduleToAllAgents: (sourceAgentId: TargetAgentId, toolId: string) => void;
}

interface PoolWorkspaceState {
  gateway: GatewayRuntimeState;
  pools: AccountPool[];
  candidates: UpstreamAccountCandidate[];
  agentBindings: AgentBinding[];
  traces: RoutingTraceItem[];
}

const detectTauriRuntime = () =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const DEFAULT_FALLBACK_VALUE: PoolPrototypeContextValue = {
  isTauriRuntime: false,
  gateway: INITIAL_GATEWAY_STATE,
  pools: [],
  selectedPoolId: "",
  setSelectedPoolId: () => {},
  candidates: [],
  agentBindings: createInitialAgentBindings([], INITIAL_GATEWAY_STATE),
  traces: [],
  nowMs: Date.now(),
  isRefreshingModels: false,
  refreshUpstreamModels: async () => {},
  toggleGateway: () => {},
  updateGatewayPort: () => {},
  cycleBridgeRuntime: () => {},
  toggleProxySimulation: () => {},
  createPool: () => INITIAL_POOLS[0],
  updatePool: () => {},
  deletePool: () => {},
  togglePoolEnabled: () => {},
  reorderPoolMember: () => {},
  togglePoolMember: () => {},
  togglePoolMemberModel: () => {},
  updatePoolMemberTier: () => {},
  togglePoolModelEnabled: () => {},
  setPoolModelFallback: () => {},
  addMembersToPool: () => {},
  removePoolMember: () => {},
  injectMemberCooldown: () => {},
  clearMemberCooldown: () => {},
  authorizeCodeArtsIsolated: () => {},
  updateAgentMountedPools: () => {},
  setAgentDefaultModel: () => {},
  mountPoolToAgents: () => {},
  unmountPoolFromAgent: () => {},
  unbindAgent: () => {},
  repairAgentDrift: () => {},
  restartAgentClient: () => {},
  toggleAgentOption: () => {},
  detectAgentEnvironment: () => {},
  runPoolSimulation: () => ({
    trace: INITIAL_ROUTING_TRACES[0],
  }),
  getAgentToolModelOptions: (toolId: string) =>
    getCatalogModelsForTool(toolId, AVAILABLE_UPSTREAM_CANDIDATES),
  getAgentToolEnabledModelIds: () => [],
  getAgentToolAccounts: () => [],
  getAgentPrimaryPool: () => null,
  setAgentToolModels: () => {},
  selectAllAgentToolModels: () => {},
  selectRecommendedAgentToolModels: () => {},
  selectLowRateAgentToolModels: () => {},
  invertAgentToolModels: () => {},
  clearAgentToolModels: () => {},
  setAgentDefaultToolModel: () => {},
  promoteAgentToolPrimaryAccount: () => {},
  reorderAgentToolAccount: () => {},
  toggleAgentToolAccountEnabled: () => {},
  setAllAgentToolAccountsEnabled: () => {},
  sortAgentToolAccountsByQuota: () => {},
  updateAgentToolRoutingOptions: () => {},
  syncToolScheduleToAllAgents: () => {},
};

const PoolPrototypeContext = createContext<PoolPrototypeContextValue | null>(null);

function slugifyPoolName(name: string): string {
  const ascii = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (ascii.length >= 3) return ascii;
  return `pool-${Math.random().toString(36).slice(2, 7)}`;
}

function getAgentIncomingProtocol(agentId: TargetAgentId): WireProtocolKind {
  switch (agentId) {
    case "codex_desktop":
      return "openai_responses";
    case "claude_desktop":
      return "anthropic_messages";
    case "dsh":
    case "opencode":
    case "mimo":
      return "openai_chat";
    case "zcode":
      return "anthropic_messages";
  }
}

function getUpstreamModelProtocol(toolId: string, modelId: string): WireProtocolKind {
  const opts = UPSTREAM_MODEL_OPTIONS[toolId] ?? [];
  const found = opts.find((o) => o.id === modelId);
  if (found) return found.protocol;
  return toolId === "minimax_code_cn" ? "anthropic_messages" : "openai_chat";
}

function formatTimeHms(d: Date): string {
  return [d.getHours(), d.getMinutes(), d.getSeconds()]
    .map((n) => String(n).padStart(2, "0"))
    .join(":");
}

function resolveValidDefaultModelKey(
  agentId: TargetAgentId,
  mountedPoolIds: string[],
  preferredKey: string | null | undefined,
  allPools: AccountPool[]
): string | null {
  if (mountedPoolIds.length === 0) return null;
  const exposed = computeAgentExposedModels(
    { agentId, mountedPoolIds, defaultModelKey: preferredKey ?? null },
    allPools
  );
  if (exposed.length === 0) return null;
  if (preferredKey && exposed.some((e) => e.key === preferredKey)) {
    return preferredKey;
  }
  return exposed[0].key;
}

/**
 * 在同一梯队内排序：
 * - `quota_first`（同工具水位优先）：不同工具严守成员初始顺位，同工具多账号之间按剩余水位降序择优；
 * - `round_robin`：按请求计数轮转最高可用梯队；
 * - `drain` / `order`：严守优先级。
 */
function sortMembersInTier(
  tierMembers: PoolMember[],
  strategy: AccountPool["strategy"],
  todayRequests: number
): PoolMember[] {
  const byPriority = [...tierMembers].sort((a, b) => a.priority - b.priority);
  if (byPriority.length <= 1) return byPriority;

  if (strategy === "quota_first") {
    const firstPriorityByTool = new Map<string, number>();
    for (const m of byPriority) {
      if (!firstPriorityByTool.has(m.toolId)) {
        firstPriorityByTool.set(m.toolId, m.priority);
      }
    }
    return [...byPriority].sort((a, b) => {
      if (a.toolId === b.toolId) {
        return b.quotaPercent - a.quotaPercent || a.priority - b.priority;
      }
      return (
        (firstPriorityByTool.get(a.toolId) ?? a.priority) -
        (firstPriorityByTool.get(b.toolId) ?? b.priority)
      );
    });
  }

  if (strategy === "round_robin") {
    const offset = todayRequests % byPriority.length;
    return [...byPriority.slice(offset), ...byPriority.slice(0, offset)];
  }

  return byPriority;
}

const RECOMMENDED_FLAGSHIP_IDS = new Set([
  "deepseek-v4-pro",
  "glm-5.3",
  "kimi-k3.1-pro",
  "qwen4-max",
  "minimax-m3.1-flash",
  "doubao-seed-2.1-pro",
  "openpangu-coder-pro",
  "claude-sonnet-4.6",
]);

function materializeDedicatedPoolForAgent(
  prevPools: AccountPool[],
  prevBindings: AgentBinding[],
  allCandidates: UpstreamAccountCandidate[],
  agentId: TargetAgentId
): {
  dedicatedId: string;
  nextPools: AccountPool[];
  nextBindings: AgentBinding[];
} {
  const dedicatedId = `pool-agent-${agentId}`;
  const binding = prevBindings.find((b) => b.agentId === agentId);
  const existing = prevPools.find((p) => p.id === dedicatedId);

  if (existing) {
    const existingAccIds = new Set(existing.members.map((m) => m.accountId));
    const missingCandidates = allCandidates.filter(
      (c) => !existingAccIds.has(c.accountId) && c.codeArtsMode !== "client_readonly"
    );
    let updatedPool = existing;
    if (missingCandidates.length > 0) {
      let nextPrio =
        existing.members.reduce((max, m) => Math.max(max, m.priority), 0) + 1;
      const appended: PoolMember[] = missingCandidates.map((cand) => ({
        id: `pm-${agentId}-${cand.accountId}`,
        accountId: cand.accountId,
        toolId: cand.toolId,
        label: cand.label,
        uidMasked: cand.uidMasked,
        supportedModels: [],
        tier: cand.toolId === "custom_api_key" ? "t3_fallback" : "t1_primary",
        channel: cand.channel,
        enabled: true,
        priority: nextPrio++,
        quotaPercent: cand.quotaPercent,
        quotaText: cand.quotaText,
        isActiveOnClient: cand.isActiveOnClient,
        accountStatus: cand.accountStatus,
        codeArtsMode: cand.codeArtsMode,
        lastLatencyMs: 540,
      }));
      updatedPool = {
        ...existing,
        members: [...existing.members, ...appended],
      };
    }

    const nextPools = prevPools.map((p) => (p.id === dedicatedId ? updatedPool : p));
    const nextBindings =
      binding?.mountedPoolIds.length === 1 && binding.mountedPoolIds[0] === dedicatedId
        ? prevBindings
        : prevBindings.map((b) =>
            b.agentId === agentId ? { ...b, mountedPoolIds: [dedicatedId] } : b
          );
    return { dedicatedId, nextPools, nextBindings };
  }

  const sourcePools = (binding?.mountedPoolIds ?? [])
    .map((pid) => prevPools.find((p) => p.id === pid))
    .filter((p): p is AccountPool => Boolean(p));
  const seenAccIds = new Set<string>();
  const builtMembers: PoolMember[] = [];

  for (const srcPool of sourcePools) {
    const enabledSet = new Set(
      srcPool.modelPolicies.filter((mp) => mp.enabled).map((mp) => mp.modelId)
    );
    for (const m of [...srcPool.members].sort((a, b) => a.priority - b.priority)) {
      if (seenAccIds.has(m.accountId)) continue;
      seenAccIds.add(m.accountId);
      const scoped = Array.from(
        new Set(
          m.supportedModels
            .filter(
              (mid) =>
                enabledSet.size === 0 ||
                enabledSet.has(mid) ||
                enabledSet.has(formatScopedModelId(m.toolId, mid))
            )
            .map((mid) => formatScopedModelId(m.toolId, mid))
        )
      );
      builtMembers.push({
        ...m,
        id: `pm-${agentId}-${m.accountId}`,
        supportedModels: scoped,
        priority: builtMembers.length + 1,
      });
    }
  }

  for (const cand of allCandidates) {
    if (seenAccIds.has(cand.accountId)) continue;
    if (cand.codeArtsMode === "client_readonly") continue;
    seenAccIds.add(cand.accountId);
    builtMembers.push({
      id: `pm-${agentId}-${cand.accountId}`,
      accountId: cand.accountId,
      toolId: cand.toolId,
      label: cand.label,
      uidMasked: cand.uidMasked,
      supportedModels: [],
      tier: cand.toolId === "custom_api_key" ? "t3_fallback" : "t1_primary",
      channel: cand.channel,
      enabled: true,
      priority: builtMembers.length + 1,
      quotaPercent: cand.quotaPercent,
      quotaText: cand.quotaText,
      isActiveOnClient: cand.isActiveOnClient,
      accountStatus: cand.accountStatus,
      codeArtsMode: cand.codeArtsMode,
      lastLatencyMs: 540,
    });
  }

  const primarySrc = sourcePools[0];
  const agentName = binding?.name ?? agentId;
  const dedicatedPool: AccountPool = {
    id: dedicatedId,
    slug: agentId,
    name: `${agentName} 专属路由`,
    description: `直接为 ${agentName} 配置订阅工具、开放模型与同工具主备调度`,
    sceneTag: "Agent 专属",
    enabled: true,
    strategy: primarySrc?.strategy ?? "order",
    affinityMode: primarySrc?.affinityMode ?? "auto_cache",
    skipActiveClientAccount: primarySrc?.skipActiveClientAccount ?? true,
    cooldownSeconds: primarySrc?.cooldownSeconds ?? 60,
    autoRefreshOn401: primarySrc?.autoRefreshOn401 ?? true,
    fallbackApiKeyEnabled: primarySrc?.fallbackApiKeyEnabled ?? true,
    modelPolicies: syncPoolModelPolicies(agentId, builtMembers, []),
    members: builtMembers,
    todayRequests: primarySrc?.todayRequests ?? 0,
    todayFailovers: primarySrc?.todayFailovers ?? 0,
    cacheHitRate: primarySrc?.cacheHitRate ?? 0,
    createdAt: new Date().toISOString(),
  };

  let mappedDefaultKey = binding?.defaultModelKey ?? null;
  if (mappedDefaultKey && mappedDefaultKey.includes("::")) {
    const [srcPid, rawMid] = mappedDefaultKey.split("::");
    const srcPool = sourcePools.find((p) => p.id === srcPid) ?? primarySrc;
    const { toolId: existingTid, modelId: bare } = splitScopedModelId(rawMid);
    const matchingMember = existingTid
      ? builtMembers.find((m) => m.toolId === existingTid)
      : srcPool?.members.find((m) =>
          m.supportedModels.some(
            (s) => s === rawMid || splitScopedModelId(s).modelId === bare
          )
        );
    if (matchingMember) {
      mappedDefaultKey = makeDefaultModelKey(
        dedicatedId,
        formatScopedModelId(matchingMember.toolId, bare)
      );
    }
  }

  const nextPools = [dedicatedPool, ...prevPools];
  const nextBindings = prevBindings.map((b) =>
    b.agentId === agentId
      ? {
          ...b,
          mountedPoolIds: [dedicatedId],
          defaultModelKey: mappedDefaultKey,
        }
      : b
  );
  return { dedicatedId, nextPools, nextBindings };
}

export function PoolPrototypeProvider({
  children,
  onToast,
}: {
  children: React.ReactNode;
  onToast?: (message: string, tone?: ToastTone) => void;
}) {
  const isTauriRuntime = detectTauriRuntime();
  // Mock 数据仅在「非 Tauri 运行时 + 开发构建」下启用；生产构建即使探测失效也只展示空态，
  // 绝不回落到假号池/假候选/假成功。
  const showMock = !isTauriRuntime && import.meta.env.DEV;
  const [gateway, setGateway] = useState<GatewayRuntimeState>(() =>
    isTauriRuntime
      ? { ...INITIAL_GATEWAY_STATE, running: false }
      : INITIAL_GATEWAY_STATE
  );
  const [pools, setPools] = useState<AccountPool[]>(() =>
    showMock ? INITIAL_POOLS : []
  );
  const [selectedPoolId, setSelectedPoolId] = useState<string>(() =>
    showMock ? INITIAL_POOLS[0].id : ""
  );
  const [candidates, setCandidates] = useState<UpstreamAccountCandidate[]>(() =>
    showMock ? AVAILABLE_UPSTREAM_CANDIDATES : []
  );
  const [rawBindings, setRawBindings] = useState<AgentBinding[]>(() =>
    showMock
      ? createInitialAgentBindings(INITIAL_POOLS, INITIAL_GATEWAY_STATE)
      : createInitialAgentBindings([], INITIAL_GATEWAY_STATE)
  );
  const [traces, setTraces] = useState<RoutingTraceItem[]>(() =>
    showMock ? INITIAL_ROUTING_TRACES : []
  );
  const [nowMs, setNowMs] = useState<number>(() => Date.now());
  const [isRefreshingModels, setIsRefreshingModels] = useState<boolean>(false);
  const sessionAffinityRef = useRef<Record<string, string>>({});
  const poolsRef = useRef<AccountPool[]>(pools);
  poolsRef.current = pools;
  const rawBindingsRef = useRef<AgentBinding[]>(rawBindings);
  rawBindingsRef.current = rawBindings;
  // 命令响应乱序保护：仅应用「不小于已应用序号」的快照，避免旧响应覆盖新状态。
  const opSeqRef = useRef(0);
  const appliedSeqRef = useRef(0);

  const hasActiveCooldown = useMemo(
    () =>
      pools.some((p) =>
        p.members.some((m) => typeof m.cooldownUntilMs === "number" && m.cooldownUntilMs > nowMs)
      ),
    [pools, nowMs]
  );

  useEffect(() => {
    if (!hasActiveCooldown) return;
    const timer = window.setInterval(() => {
      setNowMs(Date.now());
    }, 1000);
    return () => window.clearInterval(timer);
  }, [hasActiveCooldown]);

  const notify = useCallback(
    (msg: string, tone: ToastTone = "info") => {
      onToast?.(msg, tone);
    },
    [onToast]
  );

  const applyWorkspaceState = useCallback((ws: PoolWorkspaceState) => {
    setGateway(ws.gateway);
    setPools(ws.pools);
    setCandidates(ws.candidates);
    setRawBindings(ws.agentBindings);
    setTraces(ws.traces);
    setSelectedPoolId((prev) =>
      ws.pools.some((p) => p.id === prev) ? prev : (ws.pools[0]?.id ?? "")
    );
  }, []);

  const refreshWorkspace = useCallback(async () => {
    if (!isTauriRuntime) return;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const ws = await invoke<PoolWorkspaceState>("get_pool_workspace_state");
      applyWorkspaceState(ws);
    } catch (err) {
      notify(`加载网关与号池状态失败：${err instanceof Error ? err.message : String(err)}`, "error");
    }
  }, [applyWorkspaceState, isTauriRuntime, notify]);

  const refreshUpstreamModels = useCallback(
    async (accountId?: string) => {
      setIsRefreshingModels(true);
      try {
        if (!isTauriRuntime) {
          notify("原型预览模式已按最新上游协议目录对齐各账号模型", "info");
          return;
        }
        const { invoke } = await import("@tauri-apps/api/core");
        const ws = await invoke<PoolWorkspaceState>("refresh_upstream_models_cmd", {
          accountId: accountId ?? null,
        });
        applyWorkspaceState(ws);
        notify(
          accountId ? "已完成该账号实时模型探测与缓存刷新" : "已完成全部上游账号真实模型目录探测",
          "success"
        );
      } catch (err) {
        notify(
          `刷新上游模型失败：${err instanceof Error ? err.message : String(err)}`,
          "error"
        );
      } finally {
        setIsRefreshingModels(false);
      }
    },
    [applyWorkspaceState, isTauriRuntime, notify]
  );

  const syncWorkspaceCommand = useCallback(
    async (command: string, args?: Record<string, unknown>) => {
      if (!isTauriRuntime) return;
      const seq = ++opSeqRef.current;
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const ws = await invoke<PoolWorkspaceState>(command, args);
        if (seq >= appliedSeqRef.current) {
          appliedSeqRef.current = seq;
          applyWorkspaceState(ws);
        }
      } catch (err) {
        notify(`操作同步失败：${err instanceof Error ? err.message : String(err)}`, "error");
        void refreshWorkspace();
      }
    },
    [applyWorkspaceState, isTauriRuntime, notify, refreshWorkspace]
  );

  useEffect(() => {
    if (!isTauriRuntime) return;
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    let debounceTimer: number | null = null;

    const scheduleRefresh = () => {
      if (debounceTimer !== null) window.clearTimeout(debounceTimer);
      debounceTimer = window.setTimeout(() => {
        debounceTimer = null;
        if (!disposed) void refreshWorkspace();
      }, 80);
    };

    void refreshWorkspace();

    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        const register = async (eventName: string) => {
          const fn = await listen(eventName, () => {
            if (!disposed) scheduleRefresh();
          });
          if (disposed) fn();
          else unlisteners.push(fn);
        };
        await Promise.all([
          register("qwitch://pool-workspace-updated"),
          register("qwitch://account-updated"),
        ]);
      } catch {
        // Ignore event subscription errors in non-Tauri environments.
      }
    })();

    return () => {
      disposed = true;
      if (debounceTimer !== null) window.clearTimeout(debounceTimer);
      for (const unlisten of unlisteners) unlisten();
    };
  }, [isTauriRuntime, refreshWorkspace]);

  // 实时同步每个 Agent 的多号池配置 Diff 与代理预警（Tauri 模式优先保留 Rust 磁盘探测结果）
  const agentBindings = useMemo<AgentBinding[]>(() => {
    const ordered = [...rawBindings].sort((a, b) => {
      const ia = TARGET_AGENT_ORDER.indexOf(a.agentId);
      const ib = TARGET_AGENT_ORDER.indexOf(b.agentId);
      return (ia === -1 ? 99 : ia) - (ib === -1 ? 99 : ib);
    });
    return ordered.map((binding) => {
      const validPoolIds = binding.mountedPoolIds.filter((pid) =>
        pools.some((p) => p.id === pid)
      );
      const validDefaultKey = resolveValidDefaultModelKey(
        binding.agentId,
        validPoolIds,
        binding.defaultModelKey,
        pools
      );
      if (isTauriRuntime && (binding.beforeSnippet || binding.afterSnippet)) {
        return {
          ...binding,
          mountedPoolIds: validPoolIds,
          defaultModelKey: validDefaultKey,
        };
      }
      const snippets = buildAgentConfigSnippets(
        binding.agentId,
        validPoolIds,
        validDefaultKey,
        pools,
        gateway,
        binding.options,
        binding.status === "drifted"
      );
      const proxyWarning =
        binding.agentId === "codex_desktop" && gateway.globalProxyDetected
          ? "检测到系统开启 HTTP 全局代理，Codex 内置 reqwest 默认不读 macOS 忽略列表，直连 127.0.0.1 可能返回 502（建议设置 NO_PROXY=127.0.0.1,localhost）"
          : undefined;
      return {
        ...binding,
        mountedPoolIds: validPoolIds,
        defaultModelKey: validDefaultKey,
        proxyWarning,
        ...snippets,
      };
    });
  }, [rawBindings, pools, gateway, isTauriRuntime]);

  const toggleGateway = useCallback(() => {
    setGateway((prev) => {
      const next = !prev.running;
      notify(
        next
          ? `本地多协议网关已启动 (${prev.host}:${prev.port})`
          : "本地多协议网关已停止，已指向 Agent 将暂停接收流量",
        next ? "success" : "warning"
      );
      return { ...prev, running: next };
    });
    void syncWorkspaceCommand("toggle_gateway_server");
  }, [notify, syncWorkspaceCommand]);

  const updateGatewayPort = useCallback(
    (port: number) => {
      if (!Number.isFinite(port) || port < 1024 || port > 65535) {
        notify("端口范围必须在 1024–65535 之间", "error");
        return;
      }
      setGateway((prev) => ({ ...prev, port }));
      notify(`网关监听端口已更新为 127.0.0.1:${port}，已同步所有已挂载配置`, "success");
      void syncWorkspaceCommand("update_gateway_port_cmd", { port });
    },
    [notify, syncWorkspaceCommand]
  );

  const cycleBridgeRuntime = useCallback(() => {
    setGateway((prev) => {
      const order: GatewayRuntimeState["bridgeRuntime"][] = [
        "bun_ready",
        "node_ready",
        "missing",
      ];
      const next = order[(order.indexOf(prev.bridgeRuntime) + 1) % order.length];
      const version =
        next === "bun_ready"
          ? "Bun 1.2.15"
          : next === "node_ready"
            ? "Node v22.14.0 (回退)"
            : "未检测到运行时";
      notify(`OpenCode 插件桥接运行时已切换为 ${version}`, "info");
      return { ...prev, bridgeRuntime: next, bridgeVersion: version };
    });
    void syncWorkspaceCommand("cycle_bridge_runtime_cmd");
  }, [notify, syncWorkspaceCommand]);

  const toggleProxySimulation = useCallback(() => {
    setGateway((prev) => {
      const next = !prev.globalProxyDetected;
      notify(
        next
          ? "已开启系统全局代理预警检测"
          : "已关闭系统全局代理预警检测",
        next ? "warning" : "success"
      );
      return { ...prev, globalProxyDetected: next };
    });
    void syncWorkspaceCommand("toggle_proxy_simulation_cmd");
  }, [notify, syncWorkspaceCommand]);

  const createPool = useCallback(
    (input: CreatePoolInput): AccountPool => {
      const id =
        input.id ??
        `pool-${crypto.randomUUID()}`;
      const slug = slugifyPoolName(input.name);
      const members: PoolMember[] = input.members.map((item, idx) => ({
        id: `pm-${crypto.randomUUID()}`,
        accountId: item.candidate.accountId,
        toolId: item.candidate.toolId,
        label: item.candidate.label,
        uidMasked: item.candidate.uidMasked,
        supportedModels:
          item.supportedModels.length > 0
            ? Array.from(new Set(item.supportedModels))
            : item.candidate.defaultModels,
        tier:
          item.candidate.toolId === "custom_api_key"
            ? "t3_fallback"
            : item.tier,
        channel: item.candidate.channel,
        enabled: true,
        priority: idx + 1,
        quotaPercent: item.candidate.quotaPercent,
        quotaText: item.candidate.quotaText,
        isActiveOnClient: item.candidate.isActiveOnClient,
        accountStatus: item.candidate.accountStatus,
        codeArtsMode: item.candidate.codeArtsMode,
        lastLatencyMs: 520 + idx * 65,
      }));

      const enabledSet = input.enabledModelIds
        ? new Set(input.enabledModelIds)
        : undefined;

      const newPool: AccountPool = {
        id,
        slug,
        name: input.name.trim() || "未命名号池",
        description: input.description.trim() || "自定义多模型调度号池",
        sceneTag: input.sceneTag || "自定义",
        enabled: true,
        strategy: input.strategy,
        affinityMode: input.affinityMode,
        skipActiveClientAccount: input.skipActiveClientAccount,
        cooldownSeconds: input.cooldownSeconds,
        autoRefreshOn401: input.autoRefreshOn401,
        fallbackApiKeyEnabled: input.fallbackApiKeyEnabled,
        modelPolicies: syncPoolModelPolicies(slug, members, [], enabledSet),
        members,
        todayRequests: 0,
        todayFailovers: 0,
        cacheHitRate: 0,
        createdAt: new Date().toISOString(),
      };

      setPools((prev) => [newPool, ...prev]);
      setSelectedPoolId(newPool.id);

      notify(
        `已创建号池「${newPool.name}」（${members.length} 账号，开放 ${newPool.modelPolicies.filter((p) => p.enabled).length} 个模型）`,
        "success"
      );

      if (isTauriRuntime) {
        void (async () => {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            const created = await invoke<AccountPool>("create_account_pool", {
              input: {
                id,
                name: newPool.name,
                description: newPool.description,
                sceneTag: newPool.sceneTag,
                strategy: newPool.strategy,
                affinityMode: newPool.affinityMode,
                skipActiveClientAccount: newPool.skipActiveClientAccount,
                cooldownSeconds: newPool.cooldownSeconds,
                autoRefreshOn401: newPool.autoRefreshOn401,
                fallbackApiKeyEnabled: newPool.fallbackApiKeyEnabled,
                members: input.members.map((m) => ({
                  accountId: m.candidate.accountId,
                  supportedModels: m.supportedModels,
                  tier: m.tier,
                })),
                enabledModelIds: input.enabledModelIds ?? null,
              },
            });
            setPools((prev) => prev.map((p) => (p.id === created.id ? created : p)));
            void refreshWorkspace();
          } catch (err) {
            notify(`保存号池失败：${err instanceof Error ? err.message : String(err)}`, "error");
            void refreshWorkspace();
          }
        })();
      }
      return newPool;
    },
    [isTauriRuntime, notify, refreshWorkspace]
  );

  const updatePool = useCallback(
    (poolId: string, patch: Partial<AccountPool>) => {
      setPools((prev) =>
        prev.map((p) => (p.id === poolId ? { ...p, ...patch } : p))
      );
      if (isTauriRuntime) {
        void syncWorkspaceCommand("update_account_pool", {
          poolId,
          patch: {
            ...(patch.name !== undefined ? { name: patch.name } : {}),
            ...(patch.description !== undefined ? { description: patch.description } : {}),
            ...(patch.strategy !== undefined ? { strategy: patch.strategy } : {}),
            ...(patch.affinityMode !== undefined ? { affinityMode: patch.affinityMode } : {}),
            ...(patch.skipActiveClientAccount !== undefined
              ? { skipActiveClientAccount: patch.skipActiveClientAccount }
              : {}),
            ...(patch.cooldownSeconds !== undefined
              ? { cooldownSeconds: patch.cooldownSeconds }
              : {}),
            ...(patch.autoRefreshOn401 !== undefined
              ? { autoRefreshOn401: patch.autoRefreshOn401 }
              : {}),
            ...(patch.fallbackApiKeyEnabled !== undefined
              ? { fallbackApiKeyEnabled: patch.fallbackApiKeyEnabled }
              : {}),
            ...(patch.enabled !== undefined ? { enabled: patch.enabled } : {}),
          },
        });
      }
    },
    [isTauriRuntime, syncWorkspaceCommand]
  );

  const deletePool = useCallback(
    (poolId: string) => {
      const target = pools.find((p) => p.id === poolId);
      const remainingPools = pools.filter((p) => p.id !== poolId);
      setPools(remainingPools);
      if (selectedPoolId === poolId) {
        setSelectedPoolId(remainingPools[0]?.id ?? "");
      }
      setRawBindings((prev) =>
        prev.map((b) => {
          if (!b.mountedPoolIds.includes(poolId)) return b;
          const nextMounted = b.mountedPoolIds.filter((id) => id !== poolId);
          const nextDefault = resolveValidDefaultModelKey(
            b.agentId,
            nextMounted,
            b.defaultModelKey,
            remainingPools
          );
          return {
            ...b,
            mountedPoolIds: nextMounted,
            defaultModelKey: nextDefault,
            status: nextMounted.length === 0 ? "unbound" : b.status,
            driftReason: nextMounted.length === 0 ? undefined : b.driftReason,
            restartHint: nextMounted.length === 0 ? undefined : b.restartHint,
          };
        })
      );
      if (target) {
        notify(`已删除号池「${target.name}」，关联 Agent 已更新或安全解绑`, "warning");
      }
      void syncWorkspaceCommand("delete_account_pool", { poolId });
    },
    [notify, pools, selectedPoolId, syncWorkspaceCommand]
  );

  const togglePoolEnabled = useCallback(
    (poolId: string) => {
      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          const next = !p.enabled;
          notify(
            `号池「${p.name}」已${next ? "启用" : "暂停接收路由请求"}`,
            next ? "success" : "warning"
          );
          return { ...p, enabled: next };
        })
      );
      void syncWorkspaceCommand("toggle_account_pool_enabled", { poolId });
    },
    [notify, syncWorkspaceCommand]
  );

  const reorderPoolMember = useCallback(
    (poolId: string, memberId: string, direction: "up" | "down") => {
      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          const sorted = [...p.members].sort((a, b) => a.priority - b.priority);
          const idx = sorted.findIndex((m) => m.id === memberId);
          if (idx === -1) return p;
          const targetIdx = direction === "up" ? idx - 1 : idx + 1;
          if (targetIdx < 0 || targetIdx >= sorted.length) return p;
          const tmp = sorted[idx];
          sorted[idx] = sorted[targetIdx];
          sorted[targetIdx] = tmp;
          const normalized = sorted.map((m, i) => ({ ...m, priority: i + 1 }));
          return { ...p, members: normalized };
        })
      );
      void syncWorkspaceCommand("reorder_account_pool_member", {
        poolId,
        memberId,
        direction,
      });
    },
    [syncWorkspaceCommand]
  );

  const togglePoolMember = useCallback(
    (poolId: string, memberId: string) => {
      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          return {
            ...p,
            members: p.members.map((m) =>
              m.id === memberId ? { ...m, enabled: !m.enabled } : m
            ),
          };
        })
      );
      void syncWorkspaceCommand("toggle_account_pool_member", { poolId, memberId });
    },
    [syncWorkspaceCommand]
  );

  const togglePoolMemberModel = useCallback(
    (poolId: string, memberId: string, modelId: string) => {
      const targetMember = pools
        .find((p) => p.id === poolId)
        ?.members.find((m) => m.id === memberId);
      if (
        targetMember &&
        targetMember.supportedModels.includes(modelId) &&
        targetMember.supportedModels.length <= 1
      ) {
        notify("每个账号成员至少需保留 1 个服务模型；若不再需要请移出该账号", "warning");
        return;
      }
      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          const nextMembers = p.members.map((m) => {
            if (m.id !== memberId) return m;
            const exists = m.supportedModels.includes(modelId);
            if (exists && m.supportedModels.length <= 1) {
              return m;
            }
            const nextModels = exists
              ? m.supportedModels.filter((id) => id !== modelId)
              : [...m.supportedModels, modelId];
            return { ...m, supportedModels: nextModels };
          });
          return {
            ...p,
            members: nextMembers,
            modelPolicies: syncPoolModelPolicies(p.slug, nextMembers, p.modelPolicies),
          };
        })
      );
      void syncWorkspaceCommand("toggle_account_pool_member_model", {
        poolId,
        memberId,
        modelId,
      });
    },
    [notify, pools, syncWorkspaceCommand]
  );

  const updatePoolMemberTier = useCallback(
    (poolId: string, memberId: string, tier: MemberTier) => {
      const targetMember = pools
        .find((p) => p.id === poolId)
        ?.members.find((m) => m.id === memberId);
      if (targetMember?.toolId === "custom_api_key" && tier !== "t3_fallback") {
        notify("按量计费 API Key 已强制锁定在 T3 兜底档，防止误抢免费订阅流量", "warning");
        return;
      }
      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          return {
            ...p,
            members: p.members.map((m) => {
              if (m.id !== memberId) return m;
              if (m.toolId === "custom_api_key" && tier !== "t3_fallback") {
                return m;
              }
              return { ...m, tier };
            }),
          };
        })
      );
      notify(`成员梯队已调整为「${MEMBER_TIER_META[tier].label}」`, "success");
      void syncWorkspaceCommand("update_account_pool_member_tier", {
        poolId,
        memberId,
        tier,
      });
    },
    [notify, pools, syncWorkspaceCommand]
  );

  const togglePoolModelEnabled = useCallback(
    (poolId: string, modelId: string) => {
      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          return {
            ...p,
            modelPolicies: p.modelPolicies.map((mp) =>
              mp.modelId === modelId ? { ...mp, enabled: !mp.enabled } : mp
            ),
          };
        })
      );
      void syncWorkspaceCommand("toggle_account_pool_model_enabled", {
        poolId,
        modelId,
      });
    },
    [syncWorkspaceCommand]
  );

  const setPoolModelFallback = useCallback(
    (poolId: string, modelId: string, fallbackModelId: string | null) => {
      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          return {
            ...p,
            modelPolicies: p.modelPolicies.map((mp) =>
              mp.modelId === modelId ? { ...mp, fallbackModelId } : mp
            ),
          };
        })
      );
      notify(
        fallbackModelId
          ? `已设置「${modelId}」全号耗尽后显式降级至 ${fallbackModelId}`
          : `「${modelId}」已恢复严格锁模（全号不可用时绝不私自切换模型）`,
        "info"
      );
      void syncWorkspaceCommand("set_account_pool_model_fallback", {
        poolId,
        modelId,
        fallbackModelId,
      });
    },
    [notify, syncWorkspaceCommand]
  );

  const addMembersToPool = useCallback(
    (poolId: string, newMembers: CreatePoolMemberInput[]) => {
      const valid = newMembers.filter(
        (item) => item.candidate.codeArtsMode !== "client_readonly"
      );
      if (valid.length === 0) {
        notify("所选账号均处于只读隔离受限状态，请先完成独立 OAuth 授权", "warning");
        return;
      }

      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          const byAccId = new Map<string, CreatePoolMemberInput>(
            valid.map((v) => [v.candidate.accountId, v])
          );
          let nextMembers = p.members.map((existing) => {
            const incoming = byAccId.get(existing.accountId);
            if (!incoming) return existing;
            byAccId.delete(existing.accountId);
            const mergedModels = Array.from(
              new Set([...existing.supportedModels, ...incoming.supportedModels])
            );
            return {
              ...existing,
              supportedModels: mergedModels,
            };
          });

          const startPriority = nextMembers.length + 1;
          const appended: PoolMember[] = Array.from(byAccId.values()).map((item, idx) => ({
            id: `pm-${crypto.randomUUID()}`,
            accountId: item.candidate.accountId,
            toolId: item.candidate.toolId,
            label: item.candidate.label,
            uidMasked: item.candidate.uidMasked,
            supportedModels:
              item.supportedModels.length > 0
                ? Array.from(new Set(item.supportedModels))
                : item.candidate.defaultModels,
            tier:
              item.candidate.toolId === "custom_api_key"
                ? "t3_fallback"
                : item.tier,
            channel: item.candidate.channel,
            enabled: true,
            priority: startPriority + idx,
            quotaPercent: item.candidate.quotaPercent,
            quotaText: item.candidate.quotaText,
            isActiveOnClient: item.candidate.isActiveOnClient,
            accountStatus: item.candidate.accountStatus,
            codeArtsMode: item.candidate.codeArtsMode,
            lastLatencyMs: 560 + idx * 40,
          }));
          nextMembers = [...nextMembers, ...appended];
          return {
            ...p,
            members: nextMembers,
            modelPolicies: syncPoolModelPolicies(p.slug, nextMembers, p.modelPolicies),
          };
        })
      );
      notify(`已向号池更新 ${valid.length} 个账号的模型槽位配置`, "success");
      void syncWorkspaceCommand("add_account_pool_members", {
        poolId,
        members: valid.map((m) => ({
          accountId: m.candidate.accountId,
          supportedModels: m.supportedModels,
          tier: m.tier,
        })),
      });
    },
    [notify, syncWorkspaceCommand]
  );

  const removePoolMember = useCallback(
    (poolId: string, memberId: string) => {
      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          const filtered = p.members
            .filter((m) => m.id !== memberId)
            .sort((a, b) => a.priority - b.priority)
            .map((m, i) => ({ ...m, priority: i + 1 }));
          return {
            ...p,
            members: filtered,
            modelPolicies: syncPoolModelPolicies(p.slug, filtered, p.modelPolicies),
          };
        })
      );
      notify("已从号池移除该成员账号", "info");
      void syncWorkspaceCommand("remove_account_pool_member", { poolId, memberId });
    },
    [notify, syncWorkspaceCommand]
  );

  const injectMemberCooldown = useCallback(
    (poolId: string, memberId: string) => {
      const now = Date.now();
      setNowMs(now);
      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          return {
            ...p,
            members: p.members.map((m) =>
              m.id === memberId
                ? { ...m, cooldownUntilMs: now + p.cooldownSeconds * 1000 }
                : m
            ),
          };
        })
      );
      notify("已将该账号置入临时冷却期，后续请求将自动跳过该成员", "warning");
      void syncWorkspaceCommand("inject_pool_member_cooldown", { poolId, memberId });
    },
    [notify, syncWorkspaceCommand]
  );

  const clearMemberCooldown = useCallback(
    (poolId: string, memberId: string) => {
      setPools((prev) =>
        prev.map((p) => {
          if (p.id !== poolId) return p;
          return {
            ...p,
            members: p.members.map((m) =>
              m.id === memberId ? { ...m, cooldownUntilMs: undefined } : m
            ),
          };
        })
      );
      notify("已提前解除该成员冷却状态，恢复参与调度", "success");
      void syncWorkspaceCommand("clear_pool_member_cooldown", { poolId, memberId });
    },
    [notify, syncWorkspaceCommand]
  );

  const authorizeCodeArtsIsolated = useCallback(
    (accountId: string) => {
      setCandidates((prev) =>
        prev.map((c) => {
          if (c.accountId !== accountId) return c;
          return {
            ...c,
            codeArtsMode: "isolated_oauth",
            label: c.label.replace("（客户端导入）", "（已签发独立 OAuth）"),
            isActiveOnClient: false,
          };
        })
      );
      notify(
        "已完成码道 CodeArts 独立 OAuth 授权，已与客户端一次性刷新令牌物理隔离，可安全入池",
        "success"
      );
      void syncWorkspaceCommand("authorize_codearts_isolated_cmd", { accountId });
    },
    [notify, syncWorkspaceCommand]
  );

  const applyMountedPoolsLocally = useCallback(
    (
      agentId: TargetAgentId,
      mountedPoolIds: string[],
      nextDefaultModelKey?: string | null
    ) => {
      const uniqueIds = Array.from(new Set(mountedPoolIds)).filter((id) =>
        pools.some((p) => p.id === id)
      );
      setRawBindings((prev) =>
        prev.map((b) => {
          if (b.agentId !== agentId) return b;
          if (uniqueIds.length === 0) {
            return {
              ...b,
              mountedPoolIds: [],
              defaultModelKey: null,
              status: "unbound",
              driftReason: undefined,
              restartHint: undefined,
            };
          }
          const needsRestart =
          agentId === "claude_desktop" ||
          agentId === "codex_desktop" ||
          agentId === "mimo" ||
          agentId === "zcode";
          const nextDefault = resolveValidDefaultModelKey(
            agentId,
            uniqueIds,
            nextDefaultModelKey ?? b.defaultModelKey,
            pools
          );
          return {
            ...b,
            mountedPoolIds: uniqueIds,
            defaultModelKey: nextDefault,
            status: needsRestart ? "needs_restart" : "bound",
            driftReason: undefined,
            restartHint:
              agentId === "claude_desktop"
                ? "Claude 桌面版仅在启动时读取 3p 配置，请重启以拉取最新多号池 /v1/models"
                : agentId === "codex_desktop"
                  ? "已写入 ~/.codex/config.toml，请重启 app-server（无需关闭桌面窗口）"
                  : undefined,
            options: {
              ...b.options,
              dshDualCredsWritten:
                agentId === "dsh" ? true : b.options.dshDualCredsWritten,
            },
          };
        })
      );
      const agentName = rawBindings.find((b) => b.agentId === agentId)?.name ?? agentId;
      notify(
        uniqueIds.length === 0
          ? `已将 ${agentName} 还原为原生未挂载状态`
          : `已更新 ${agentName} 挂载配置（共 ${uniqueIds.length} 个号池）`,
        uniqueIds.length === 0 ? "info" : "success"
      );
      return uniqueIds;
    },
    [notify, pools, rawBindings]
  );

  const updateAgentMountedPools = useCallback(
    (
      agentId: TargetAgentId,
      mountedPoolIds: string[],
      nextDefaultModelKey?: string | null
    ) => {
      const uniqueIds = applyMountedPoolsLocally(
        agentId,
        mountedPoolIds,
        nextDefaultModelKey
      );
      void syncWorkspaceCommand("update_agent_mounted_pools_cmd", {
        agentId,
        mountedPoolIds: uniqueIds,
        defaultModelKey: nextDefaultModelKey ?? null,
      });
    },
    [applyMountedPoolsLocally, syncWorkspaceCommand]
  );

  const setAgentDefaultModel = useCallback(
    (agentId: TargetAgentId, modelKey: string) => {
      setRawBindings((prev) =>
        prev.map((b) => {
          if (b.agentId !== agentId) return b;
          const [poolId] = modelKey.split("::");
          const nextMounted = b.mountedPoolIds.includes(poolId)
            ? b.mountedPoolIds
            : [...b.mountedPoolIds, poolId];
          return {
            ...b,
            mountedPoolIds: nextMounted,
            defaultModelKey: modelKey,
            status: agentId === "codex_desktop" ? "needs_restart" : b.status === "unbound" ? "bound" : b.status,
            restartHint:
              agentId === "codex_desktop"
                ? "默认模型已变更，请重启 app-server 使新线程默认生效"
                : b.restartHint,
          };
        })
      );
      const modelName = modelKey.split("::")[1] ?? modelKey;
      notify(`已将默认模型切换为 ★ ${modelName}`, "success");
      void syncWorkspaceCommand("set_agent_default_model_cmd", { agentId, modelKey });
    },
    [notify, syncWorkspaceCommand]
  );

  const mountPoolToAgents = useCallback(
    (poolId: string, agentIds: TargetAgentId[]) => {
      const targetPool = pools.find((p) => p.id === poolId);
      if (!targetPool || agentIds.length === 0) return;
      const set = new Set(agentIds);
      setRawBindings((prev) =>
        prev.map((b) => {
          if (!set.has(b.agentId)) return b;
          const nextMounted = b.mountedPoolIds.includes(poolId)
            ? b.mountedPoolIds
            : [...b.mountedPoolIds, poolId];
          const needsRestart =
            b.agentId === "claude_desktop" ||
              b.agentId === "codex_desktop" ||
              b.agentId === "mimo" ||
              b.agentId === "zcode";
          const nextDefault = resolveValidDefaultModelKey(
            b.agentId,
            nextMounted,
            b.defaultModelKey ?? makeDefaultModelKey(poolId, targetPool.modelPolicies.find((m) => m.enabled)?.modelId ?? ""),
            pools
          );
          return {
            ...b,
            mountedPoolIds: nextMounted,
            defaultModelKey: nextDefault,
            status: needsRestart ? "needs_restart" : "bound",
            driftReason: undefined,
            restartHint:
              b.agentId === "claude_desktop"
                ? "Claude 桌面版仅在启动时读取 3p 配置，请重启使新挂载号池进入下拉清单"
                : b.agentId === "codex_desktop"
                  ? "已追加写入 ~/.codex/config.toml，请重启 app-server 生效"
                  : undefined,
            options: {
              ...b.options,
              dshDualCredsWritten:
                b.agentId === "dsh" ? true : b.options.dshDualCredsWritten,
            },
          };
        })
      );
      notify(`已将号池「${targetPool.name}」追加挂载至 ${agentIds.length} 个 Agent`, "success");
      void syncWorkspaceCommand("mount_pool_to_agents_cmd", { poolId, agentIds });
    },
    [notify, pools, syncWorkspaceCommand]
  );

  const unmountPoolFromAgent = useCallback(
    (agentId: TargetAgentId, poolId: string) => {
      const targetBinding = rawBindings.find((b) => b.agentId === agentId);
      if (!targetBinding) return;
      const nextMounted = targetBinding.mountedPoolIds.filter((id) => id !== poolId);
      applyMountedPoolsLocally(agentId, nextMounted);
      void syncWorkspaceCommand("unmount_pool_from_agent_cmd", { agentId, poolId });
    },
    [applyMountedPoolsLocally, rawBindings, syncWorkspaceCommand]
  );

  const unbindAgent = useCallback(
    (agentId: TargetAgentId) => {
      setRawBindings((prev) =>
        prev.map((b) => {
          if (b.agentId !== agentId) return b;
          return {
            ...b,
            mountedPoolIds: [],
            defaultModelKey: null,
            status: "unbound",
            driftReason: undefined,
            restartHint: undefined,
          };
        })
      );
      const agentName = rawBindings.find((b) => b.agentId === agentId)?.name ?? agentId;
      notify(
        agentId === "codex_desktop"
          ? `已从 Stash 还原 ${agentName} 顶层配置（已安全保留 [model_providers.qwitch] 表防旧线程报错）`
          : `已从 Stash 无损还原 ${agentName} 原始配置文件`,
        "info"
      );
      void syncWorkspaceCommand("unbind_agent_cmd", { agentId });
    },
    [notify, rawBindings, syncWorkspaceCommand]
  );

  const repairAgentDrift = useCallback(
    (agentId: TargetAgentId) => {
      setRawBindings((prev) =>
        prev.map((b) => {
          if (b.agentId !== agentId) return b;
          return {
            ...b,
            status: "bound",
            driftReason: undefined,
            restartHint: undefined,
            options: {
              ...b.options,
              dshDualCredsWritten:
                agentId === "dsh" ? true : b.options.dshDualCredsWritten,
            },
          };
        })
      );
      const agentName = rawBindings.find((b) => b.agentId === agentId)?.name ?? agentId;
      notify(`已修复 ${agentName} 配置漂移，双凭据与路由键校验全部通过`, "success");
      void syncWorkspaceCommand("repair_agent_drift_cmd", { agentId });
    },
    [notify, rawBindings, syncWorkspaceCommand]
  );

  const restartAgentClient = useCallback(
    (agentId: TargetAgentId) => {
      setRawBindings((prev) =>
        prev.map((b) => {
          if (b.agentId !== agentId) return b;
          return {
            ...b,
            status: b.mountedPoolIds.length > 0 ? "bound" : "unbound",
            restartHint: undefined,
          };
        })
      );
      if (agentId === "codex_desktop") {
        notify("已重启 ChatGPT/Codex 桌面应用以加载最新配置", "success");
      } else if (agentId === "opencode") {
        notify("OpenCode 为终端客户端：配置将在下次启动/新会话生效（无需重启桌面进程）", "info");
      } else {
        const agentName = rawBindings.find((b) => b.agentId === agentId)?.name ?? agentId;
        notify(`已重启 ${agentName}，最新 Qwitch 网关配置已生效`, "success");
      }
      void syncWorkspaceCommand("restart_agent_client_cmd", { agentId });
    },
    [notify, rawBindings, syncWorkspaceCommand]
  );

  const toggleAgentOption = useCallback(
    (agentId: TargetAgentId, key: keyof AgentBindingOptions) => {
      setRawBindings((prev) =>
        prev.map((b) => {
          if (b.agentId !== agentId) return b;
          const current = b.options[key];
          let nextVal: unknown;
          if (key === "opencodeSchemaVersion") {
            nextVal = current === "v1" ? "v2" : "v1";
          } else {
            nextVal = !current;
          }
          const nextOptions = { ...b.options, [key]: nextVal };
          let nextConfigPath = b.configPath;
          if (agentId === "claude_desktop" && key === "claudeIsolatedTwin") {
            nextConfigPath = nextVal
              ? "~/Library/Application Support/Claude-Qwitch-3p/configLibrary/"
              : "~/Library/Application Support/Claude-3p/configLibrary/";
          }
          return {
            ...b,
            configPath: nextConfigPath,
            options: nextOptions,
          };
        })
      );
      notify("已更新 Agent 专属挂载选项并重新生成配置", "info");
      void syncWorkspaceCommand("toggle_agent_option_cmd", { agentId, key });
    },
    [notify, syncWorkspaceCommand]
  );

  const detectAgentEnvironment = useCallback(() => {
    notify("已完成 4 个目标 Agent 安装路径、Schema 版本与本机代理状态重新探测", "success");
    void syncWorkspaceCommand("detect_agent_environment_cmd");
  }, [notify, syncWorkspaceCommand]);

  const runPoolSimulation = useCallback(
    (input: RunPoolSimulationInput): RunPoolSimulationResult => {
      const now = Date.now();
      setNowMs(now);
      const pool = pools.find((p) => p.id === input.poolId) ?? pools[0];
      const incomingProtocol = getAgentIncomingProtocol(input.agentId);
      if (!pool) {
        const emptyTrace: RoutingTraceItem = {
          id: `tr-${crypto.randomUUID()}`,
          timestamp: formatTimeHms(new Date(now)),
          agentId: input.agentId,
          poolId: input.poolId,
          poolName: input.poolId,
          incomingProtocol,
          upstreamProtocol: "openai_chat",
          requestedModel: input.modelId || "deepseek-v4-pro",
          finalAccountLabel: "无可用成员",
          finalToolId: "custom_api_key",
          finalModel: "—",
          status: "failed",
          cacheAffinityHit: false,
          ttftMs: 0,
          totalTokens: 0,
          attempts: [],
        };
        return { trace: emptyTrace };
      }
      const requestedModel =
        input.modelId ||
        pool.modelPolicies.find((mp) => mp.enabled)?.modelId ||
        pool.members[0]?.supportedModels[0] ||
        "deepseek-v4-pro";

      const collectEligibleForModel = (
        targetModel: string,
        collectAttempts: RoutingAttempt[]
      ): PoolMember[] => {
        const tierBuckets: Record<MemberTier, PoolMember[]> = {
          t1_primary: [],
          t2_backup: [],
          t3_fallback: [],
        };
        const { toolId: scopedTid, modelId: bareTarget } = splitScopedModelId(targetModel);

        for (const m of pool.members) {
          if (scopedTid && m.toolId !== scopedTid) continue;
          if (
            !m.supportedModels.some(
              (id) => id === targetModel || splitScopedModelId(id).modelId === bareTarget
            )
          ) {
            continue;
          }
          if (!m.enabled) continue;
          if (m.toolId === "custom_api_key" && !pool.fallbackApiKeyEnabled) continue;
          if (m.accountStatus === "risk_paused" || m.accountStatus === "expired") continue;
          if (m.codeArtsMode === "client_readonly") continue;
          if (typeof m.cooldownUntilMs === "number" && m.cooldownUntilMs > now) continue;
          if (pool.skipActiveClientAccount && m.isActiveOnClient) {
            collectAttempts.push({
              accountLabel: m.label,
              toolId: m.toolId,
              model: targetModel,
              tier: m.tier,
              outcome: "skipped_active",
              latencyMs: 1,
              detail: "开启了「跳过桌面端活跃账号」护栏，自动避让以免与官方客户端互踢",
            });
            continue;
          }
          tierBuckets[m.tier].push(m);
        }

        return [
          ...sortMembersInTier(tierBuckets.t1_primary, pool.strategy, pool.todayRequests),
          ...sortMembersInTier(tierBuckets.t2_backup, pool.strategy, pool.todayRequests),
          ...sortMembersInTier(tierBuckets.t3_fallback, pool.strategy, pool.todayRequests),
        ];
      };

      const attempts: RoutingAttempt[] = [];
      let effectiveModel = requestedModel;
      let eligible = collectEligibleForModel(effectiveModel, attempts);
      const policy = pool.modelPolicies.find((mp) => mp.modelId === requestedModel);
      let usedExplicitFallback = false;

      if (eligible.length === 0 && policy?.fallbackModelId) {
        const fbAttempts: RoutingAttempt[] = [];
        const fbEligible = collectEligibleForModel(policy.fallbackModelId, fbAttempts);
        if (fbEligible.length > 0) {
          usedExplicitFallback = true;
          effectiveModel = policy.fallbackModelId;
          eligible = fbEligible;
          attempts.push(...fbAttempts);
        }
      }

      const affKey = `${pool.id}::${effectiveModel}::${input.sessionKey ?? "default"}`;
      const stickyMemberId =
        pool.affinityMode !== "off" ? sessionAffinityRef.current[affKey] : undefined;
      let affinityMatched = false;

      if (stickyMemberId) {
        const stickyIdx = eligible.findIndex((m) => m.id === stickyMemberId);
        if (stickyIdx >= 0) {
          affinityMatched = true;
          if (stickyIdx > 0) {
            const [sticky] = eligible.splice(stickyIdx, 1);
            eligible.unshift(sticky);
          }
        }
      }

      let cooledMemberId: string | undefined;
      let finalMember: PoolMember | undefined;
      let status: RoutingTraceItem["status"] = "success";

      if (eligible.length === 0) {
        status = "failed";
        attempts.push({
          accountLabel: pool.name,
          toolId: "custom_api_key",
          model: requestedModel,
          outcome: "429_cooldown",
          latencyMs: 12,
          detail: policy?.fallbackModelId
            ? `主模型「${requestedModel}」与降级模型「${policy.fallbackModelId}」当前均无可用账号`
            : `严格锁模「${requestedModel}」：所属账号当前均处于冷却、禁用或未启用 API Key 兜底（网关已拒绝私自换模型）`,
        });
      } else if (input.failureMode === "429_first") {
        if (eligible.length >= 2) {
          const first = eligible[0];
          const second = eligible[1];
          cooledMemberId = first.id;
          finalMember = second;
          status = "failover_success";
          attempts.push({
            accountLabel: first.label,
            toolId: first.toolId,
            model: effectiveModel,
            tier: first.tier,
            outcome: "429_cooldown",
            latencyMs: 185,
            detail: `${MEMBER_TIER_META[first.tier].shortLabel} 首选号并发限流 HTTP 429，置入 ${pool.cooldownSeconds}s 冷却并触发首字节前 Failover`,
          });
          attempts.push({
            accountLabel: second.label,
            toolId: second.toolId,
            model: effectiveModel,
            tier: second.tier,
            outcome: "ok",
            latencyMs: second.lastLatencyMs ?? 560,
            detail: `${MEMBER_TIER_META[second.tier].shortLabel} 接管成功，模型严格锁定 ${effectiveModel}（${second.channel === "rust_native" ? "Rust 原生" : second.channel === "api_key" ? "T3 API Key 兜底" : "OpenCode 桥接"}）`,
          });
        } else {
          const only = eligible[0];
          cooledMemberId = only.id;
          // 检查是否配置了显式降级模型
          if (!usedExplicitFallback && policy?.fallbackModelId) {
            const fbList = collectEligibleForModel(policy.fallbackModelId, []);
            if (fbList.length > 0) {
              const fbTarget = fbList[0];
              finalMember = fbTarget;
              effectiveModel = policy.fallbackModelId;
              status = "failover_success";
              attempts.push({
                accountLabel: only.label,
                toolId: only.toolId,
                model: requestedModel,
                tier: only.tier,
                outcome: "429_cooldown",
                latencyMs: 180,
                detail: `「${requestedModel}」最后可用账号触发 429 冷却，按显式降级策略切换至 ${effectiveModel}`,
              });
              attempts.push({
                accountLabel: fbTarget.label,
                toolId: fbTarget.toolId,
                model: effectiveModel,
                tier: fbTarget.tier,
                outcome: "ok",
                latencyMs: fbTarget.lastLatencyMs ?? 520,
                detail: `显式降级模型「${effectiveModel}」成功返回，已透明告知客户端`,
              });
            } else {
              status = "failed";
              attempts.push({
                accountLabel: only.label,
                toolId: only.toolId,
                model: requestedModel,
                tier: only.tier,
                outcome: "429_cooldown",
                latencyMs: 180,
                detail: `「${requestedModel}」唯一可用号触发 429 冷却，显式降级目标同样不可用`,
              });
            }
          } else {
            status = "failed";
            attempts.push({
              accountLabel: only.label,
              toolId: only.toolId,
              model: requestedModel,
              tier: only.tier,
              outcome: "429_cooldown",
              latencyMs: 180,
              detail: `「${requestedModel}」最后一个可用账号触发 429 冷却，且未配置降级模型，严格锁模拒绝换模型`,
            });
          }
        }
      } else if (input.failureMode === "401_refresh") {
        const first = eligible[0];
        finalMember = first;
        status = "failover_success";
        attempts.push({
          accountLabel: first.label,
          toolId: first.toolId,
          model: effectiveModel,
          tier: first.tier,
          outcome: "401_refreshed",
          latencyMs: 190,
          detail: "上游返回 401，RefreshSingleFlight 按 account_id 单飞刷新并加密写回 qwitch.db",
        });
        attempts.push({
          accountLabel: first.label,
          toolId: first.toolId,
          model: effectiveModel,
          tier: first.tier,
          outcome: "ok",
          latencyMs: first.lastLatencyMs ?? 520,
          detail: `使用新 Token 原地重试 ${effectiveModel} 成功`,
        });
      } else {
        const first = eligible[0];
        finalMember = first;
        status = usedExplicitFallback ? "failover_success" : "success";
        attempts.push({
          accountLabel: first.label,
          toolId: first.toolId,
          model: effectiveModel,
          tier: first.tier,
          outcome: "ok",
          latencyMs: first.lastLatencyMs ?? 540,
          detail: usedExplicitFallback
            ? `主模型账号不可用，按显式降级配置由「${effectiveModel}」接管`
            : affinityMatched
              ? `同会话亲和命中 Prompt Cache（自动跳过已冷却账号并粘住 ${first.label}）`
              : `按 ${MEMBER_TIER_META[first.tier].shortLabel} + ${pool.strategy === "quota_first" ? "同工具水位" : pool.strategy === "drain" ? "单号耗尽" : pool.strategy === "order" ? "固定顺位" : "轮询"} 命中「${effectiveModel}」`,
        });
      }

      if (finalMember && pool.affinityMode !== "off") {
        sessionAffinityRef.current[`${pool.id}::${effectiveModel}::${input.sessionKey ?? "default"}`] =
          finalMember.id;
      }

      const upstreamProtocol: WireProtocolKind = finalMember
        ? getUpstreamModelProtocol(finalMember.toolId, effectiveModel)
        : "openai_chat";

      const totalLatency = attempts.reduce((sum, a) => sum + a.latencyMs, 0);
      const tokenEstimate = Math.max(1200, input.prompt.trim().length * 38 + 2400);

      const newTrace: RoutingTraceItem = {
        id: `tr-${crypto.randomUUID()}`,
        timestamp: formatTimeHms(new Date(now)),
        agentId: input.agentId,
        poolId: pool.id,
        poolName: pool.name,
        incomingProtocol,
        upstreamProtocol,
        requestedModel,
        finalAccountLabel: finalMember?.label ?? "无可用成员",
        finalToolId: finalMember?.toolId ?? "custom_api_key",
        finalModel: finalMember ? effectiveModel : "—",
        status,
        cacheAffinityHit: Boolean(finalMember) && affinityMatched && input.failureMode === "none",
        ttftMs: totalLatency,
        totalTokens: tokenEstimate,
        attempts,
      };

      // 真机（Tauri）下以 run_pool_simulation_cmd 落库后的 workspace 刷新为唯一真源，
      // 不再做乐观本地写入，避免「本地 TS 结果」与「后端 Rust 结果」双写抖动；
      // 浏览器 Mock 模式（非 Tauri）仍本地推演。
      if (!isTauriRuntime) {
        setTraces((prev) => [newTrace, ...prev]);
        setPools((prev) =>
          prev.map((p) => {
            if (p.id !== pool.id) return p;
            return {
              ...p,
              todayRequests: p.todayRequests + 1,
              todayFailovers:
                status === "failover_success" ? p.todayFailovers + 1 : p.todayFailovers,
              members: p.members.map((m) =>
                cooledMemberId && m.id === cooledMemberId
                  ? { ...m, cooldownUntilMs: now + p.cooldownSeconds * 1000 }
                  : m
              ),
            };
          })
        );
        setRawBindings((prev) =>
          prev.map((b) =>
            b.agentId === input.agentId ? { ...b, lastHitAt: "刚刚" } : b
          )
        );
      }

      notify(
        status === "failover_success"
          ? `路由测试完成：已切至「${finalMember?.label}」(${effectiveModel})`
          : status === "success"
            ? `路由测试命中「${finalMember?.label}」(${effectiveModel} · ${totalLatency}ms)`
            : `严格锁模「${requestedModel}」失败：当前无可用账号`,
        status === "failed" ? "error" : status === "failover_success" ? "warning" : "success"
      );

      if (isTauriRuntime) {
        void (async () => {
          try {
            const { invoke } = await import("@tauri-apps/api/core");
            await invoke<RunPoolSimulationResult>("run_pool_simulation_cmd", {
              input: {
                agentId: input.agentId,
                poolId: input.poolId,
                modelId: input.modelId,
                prompt: input.prompt,
                failureMode: input.failureMode,
                sessionKey: input.sessionKey ?? null,
              },
            });
            void refreshWorkspace();
          } catch (err) {
            notify(`调度演练持久化失败：${err instanceof Error ? err.message : String(err)}`, "error");
            void refreshWorkspace();
          }
        })();
      }

      return {
        trace: newTrace,
        cooledMemberId,
      };
    },
    [isTauriRuntime, notify, pools, refreshWorkspace]
  );

  const getAgentToolModelOptions = useCallback(
    (toolId: string) => getCatalogModelsForTool(toolId, candidates),
    [candidates]
  );

  const getAgentPrimaryPool = useCallback(
    (agentId: TargetAgentId): AccountPool | null => {
      const binding = rawBindings.find((b) => b.agentId === agentId);
      const dedicated = pools.find((p) => p.id === `pool-agent-${agentId}`);
      if (dedicated) return dedicated;
      const firstId = binding?.mountedPoolIds[0];
      return pools.find((p) => p.id === firstId) ?? null;
    },
    [pools, rawBindings]
  );

  const getAgentToolEnabledModelIds = useCallback(
    (agentId: TargetAgentId, toolId: string): string[] => {
      const binding = rawBindings.find((b) => b.agentId === agentId);
      if (!binding) return [];
      const mountedPools = binding.mountedPoolIds
        .map((id) => pools.find((p) => p.id === id))
        .filter((p): p is AccountPool => Boolean(p && p.enabled));
      const enabled = new Set<string>();
      for (const pool of mountedPools) {
        const enabledPolicyIds = new Set(
          pool.modelPolicies.filter((mp) => mp.enabled).map((mp) => mp.modelId)
        );
        const toolMembers = pool.members.filter((m) => m.toolId === toolId);
        for (const m of toolMembers) {
          for (const rawMid of m.supportedModels) {
            const { toolId: scopedTid, modelId: bare } = splitScopedModelId(rawMid);
            if (scopedTid && scopedTid !== toolId) continue;
            if (
              enabledPolicyIds.has(rawMid) ||
              enabledPolicyIds.has(formatScopedModelId(toolId, bare)) ||
              (!scopedTid && enabledPolicyIds.has(bare))
            ) {
              enabled.add(bare);
            }
          }
        }
      }
      return Array.from(enabled);
    },
    [pools, rawBindings]
  );

  const getAgentToolAccounts = useCallback(
    (agentId: TargetAgentId, toolId: string): PoolMember[] => {
      const binding = rawBindings.find((b) => b.agentId === agentId);
      const dedicated = pools.find((p) => p.id === `pool-agent-${agentId}`);
      const sourcePool =
        dedicated ??
        (binding?.mountedPoolIds[0]
          ? pools.find((p) => p.id === binding.mountedPoolIds[0])
          : undefined);
      const result: PoolMember[] = [];
      const seenAccIds = new Set<string>();
      if (sourcePool) {
        const sortedMembers = [...sourcePool.members]
          .filter((m) => m.toolId === toolId)
          .sort((a, b) => a.priority - b.priority);
        for (const m of sortedMembers) {
          if (seenAccIds.has(m.accountId)) continue;
          seenAccIds.add(m.accountId);
          result.push(m);
        }
      }
      let nextPrio = result.reduce((max, m) => Math.max(max, m.priority), 0) + 1;
      for (const cand of candidates) {
        if (cand.toolId !== toolId || seenAccIds.has(cand.accountId)) continue;
        seenAccIds.add(cand.accountId);
        result.push({
          id: `cand-${agentId}-${cand.accountId}`,
          accountId: cand.accountId,
          toolId: cand.toolId,
          label: cand.label,
          uidMasked: cand.uidMasked,
          supportedModels: [],
          tier: cand.toolId === "custom_api_key" ? "t3_fallback" : "t1_primary",
          channel: cand.channel,
          enabled: result.length === 0 && cand.codeArtsMode !== "client_readonly",
          priority: nextPrio++,
          quotaPercent: cand.quotaPercent,
          quotaText: cand.quotaText,
          isActiveOnClient: cand.isActiveOnClient,
          accountStatus: cand.accountStatus,
          codeArtsMode: cand.codeArtsMode,
          lastLatencyMs: 540,
        });
      }
      return result;
    },
    [candidates, pools, rawBindings]
  );

  const setAgentToolModels = useCallback(
    (agentId: TargetAgentId, toolId: string, modelIds: string[]) => {
      const uniqueBare = Array.from(
        new Set(modelIds.map((m) => splitScopedModelId(m).modelId).filter(Boolean))
      );
      const scopedList = uniqueBare.map((bare) => formatScopedModelId(toolId, bare));

      const { dedicatedId, nextPools, nextBindings } = materializeDedicatedPoolForAgent(
        poolsRef.current,
        rawBindingsRef.current,
        candidates,
        agentId
      );

      const updatedPools = nextPools.map((p) => {
        if (p.id !== dedicatedId) return p;
        const nextMembers = p.members.map((m) =>
          m.toolId === toolId ? { ...m, supportedModels: scopedList } : m
        );
        const existingPolicies = p.modelPolicies.filter(
          (mp) => splitScopedModelId(mp.modelId).toolId !== toolId
        );
        const toolPolicies = scopedList.map((scopedId) => {
          const prev = p.modelPolicies.find((mp) => mp.modelId === scopedId);
          return (
            prev
              ? { ...prev, enabled: true }
              : {
                  modelId: scopedId,
                  enabled: true,
                  fallbackModelId: null,
                  claudeAliasId: buildClaudeModelAlias(p.slug, scopedId),
                }
          );
        });
        const nextPolicies = syncPoolModelPolicies(p.slug, nextMembers, [
          ...existingPolicies,
          ...toolPolicies,
        ]);
        return {
          ...p,
          members: nextMembers,
          modelPolicies: nextPolicies,
        };
      });

      let computedDefaultKey: string | null = null;
      const updatedBindings: AgentBinding[] = nextBindings.map((b) => {
        if (b.agentId !== agentId) return b;
        const validDefault = resolveValidDefaultModelKey(
          agentId,
          [dedicatedId],
          b.defaultModelKey,
          updatedPools
        );
        computedDefaultKey = validDefault;
        const needsRestart =
          agentId === "claude_desktop" ||
          agentId === "codex_desktop" ||
          agentId === "mimo" ||
          agentId === "zcode";
        const nextStatus: AgentBinding["status"] =
          b.status === "drifted"
            ? "bound"
            : needsRestart
              ? "needs_restart"
              : "bound";
        return {
          ...b,
          mountedPoolIds: [dedicatedId],
          defaultModelKey: validDefault,
          status: nextStatus,
          driftReason: undefined,
          restartHint:
            agentId === "claude_desktop"
              ? "Claude 桌面版模型清单已更新，请重启以拉取最新 /v1/models"
              : agentId === "codex_desktop"
                ? "已更新 Codex Catalog，请重启 app-server 使新模型清单生效"
                : undefined,
          options: {
            ...b.options,
            dshDualCredsWritten:
              agentId === "dsh" ? true : b.options.dshDualCredsWritten,
          },
        };
      });

      poolsRef.current = updatedPools;
      rawBindingsRef.current = updatedBindings;
      setPools(updatedPools);
      setRawBindings(updatedBindings);

      const toolLabel = TOOL_SHORT_LABELS[toolId] ?? toolId;
      notify(
        scopedList.length === 0
          ? `已清空 ${toolLabel} 的开放模型`
          : `已更新 ${toolLabel} 对该 Agent 开放 ${scopedList.length} 个模型`,
        "success"
      );

      void syncWorkspaceCommand("set_agent_tool_models_cmd", {
        agentId,
        toolId,
        modelIds: uniqueBare,
        defaultModelKey: computedDefaultKey,
      });
    },
    [candidates, notify, syncWorkspaceCommand]
  );

  const selectAllAgentToolModels = useCallback(
    (agentId: TargetAgentId, toolId: string, filterModelIds?: string[]) => {
      const options = getCatalogModelsForTool(toolId, candidates);
      const targetIds = filterModelIds
        ? filterModelIds
        : options.map((o) => o.id);
      const current = getAgentToolEnabledModelIds(agentId, toolId);
      const merged = filterModelIds
        ? Array.from(new Set([...current, ...targetIds]))
        : targetIds;
      setAgentToolModels(agentId, toolId, merged);
    },
    [candidates, getAgentToolEnabledModelIds, setAgentToolModels]
  );

  const selectRecommendedAgentToolModels = useCallback(
    (agentId: TargetAgentId, toolId: string) => {
      const options = getCatalogModelsForTool(toolId, candidates);
      const flagship = options
        .filter(
          (o) =>
            RECOMMENDED_FLAGSHIP_IDS.has(o.id) ||
            (typeof o.creditMultiplier === "number" && o.creditMultiplier >= 0.45)
        )
        .map((o) => o.id);
      const chosen =
        flagship.length > 0 ? flagship.slice(0, 4) : options.slice(0, 3).map((o) => o.id);
      setAgentToolModels(agentId, toolId, chosen);
    },
    [candidates, setAgentToolModels]
  );

  const selectLowRateAgentToolModels = useCallback(
    (agentId: TargetAgentId, toolId: string) => {
      const options = getCatalogModelsForTool(toolId, candidates);
      const lowRate = options
        .filter(
          (o) =>
            o.isFree ||
            (typeof o.creditMultiplier === "number" && o.creditMultiplier <= 0.25) ||
            /flash|turbo|mini|fast/i.test(o.id)
        )
        .map((o) => o.id);
      if (lowRate.length === 0) {
        notify("该工具暂无明显低倍率或 Flash 模型，已保留前 2 个模型", "info");
        setAgentToolModels(
          agentId,
          toolId,
          options.slice(0, 2).map((o) => o.id)
        );
        return;
      }
      setAgentToolModels(agentId, toolId, lowRate);
    },
    [candidates, notify, setAgentToolModels]
  );

  const invertAgentToolModels = useCallback(
    (agentId: TargetAgentId, toolId: string, scopeModelIds?: string[]) => {
      const options = getCatalogModelsForTool(toolId, candidates);
      const scope = new Set(scopeModelIds ?? options.map((o) => o.id));
      const current = new Set(getAgentToolEnabledModelIds(agentId, toolId));
      const next: string[] = [];
      for (const opt of options) {
        if (scope.has(opt.id)) {
          if (!current.has(opt.id)) next.push(opt.id);
        } else if (current.has(opt.id)) {
          next.push(opt.id);
        }
      }
      setAgentToolModels(agentId, toolId, next);
    },
    [candidates, getAgentToolEnabledModelIds, setAgentToolModels]
  );

  const clearAgentToolModels = useCallback(
    (agentId: TargetAgentId, toolId: string) => {
      setAgentToolModels(agentId, toolId, []);
    },
    [setAgentToolModels]
  );

  const setAgentDefaultToolModel = useCallback(
    (agentId: TargetAgentId, toolId: string, modelId: string) => {
      const bare = splitScopedModelId(modelId).modelId;
      const currentEnabled = getAgentToolEnabledModelIds(agentId, toolId);
      const nextModels = currentEnabled.includes(bare)
        ? currentEnabled
        : [...currentEnabled, bare];

      const { dedicatedId, nextPools, nextBindings } = materializeDedicatedPoolForAgent(
        poolsRef.current,
        rawBindingsRef.current,
        candidates,
        agentId
      );
      const scopedDefault = formatScopedModelId(toolId, bare);
      const nextDefaultKey = makeDefaultModelKey(dedicatedId, scopedDefault);

      const scopedList = nextModels.map((id) => formatScopedModelId(toolId, id));
      const updatedPools = nextPools.map((p) => {
        if (p.id !== dedicatedId) return p;
        const nextMembers = p.members.map((m) =>
          m.toolId === toolId ? { ...m, supportedModels: scopedList } : m
        );
        const existingPolicies = p.modelPolicies.filter(
          (mp) => splitScopedModelId(mp.modelId).toolId !== toolId
        );
        const toolPolicies = scopedList.map((scopedId) => {
          const prev = p.modelPolicies.find((mp) => mp.modelId === scopedId);
          return prev
            ? { ...prev, enabled: true }
            : {
                modelId: scopedId,
                enabled: true,
                fallbackModelId: null,
                claudeAliasId: buildClaudeModelAlias(p.slug, scopedId),
              };
        });
        return {
          ...p,
          members: nextMembers,
          modelPolicies: syncPoolModelPolicies(p.slug, nextMembers, [
            ...existingPolicies,
            ...toolPolicies,
          ]),
        };
      });

      const updatedBindings: AgentBinding[] = nextBindings.map((b) => {
        if (b.agentId !== agentId) return b;
        const needsRestart =
          agentId === "claude_desktop" ||
          agentId === "codex_desktop" ||
          agentId === "mimo" ||
          agentId === "zcode";
        const nextStatus: AgentBinding["status"] = needsRestart
          ? "needs_restart"
          : "bound";
        return {
          ...b,
          mountedPoolIds: [dedicatedId],
          defaultModelKey: nextDefaultKey,
          status: nextStatus,
        };
      });

      poolsRef.current = updatedPools;
      rawBindingsRef.current = updatedBindings;
      setPools(updatedPools);
      setRawBindings(updatedBindings);

      const toolLabel = TOOL_SHORT_LABELS[toolId] ?? toolId;
      notify(`已将 ${toolLabel} · ${bare} 设为默认首选模型`, "success");

      void syncWorkspaceCommand("set_agent_tool_models_cmd", {
        agentId,
        toolId,
        modelIds: nextModels,
        defaultModelKey: nextDefaultKey,
      });
    },
    [candidates, getAgentToolEnabledModelIds, notify, syncWorkspaceCommand]
  );

  const commitAgentToolAccountSchedule = useCallback(
    (
      agentId: TargetAgentId,
      toolId: string,
      transformToolMembers: (toolMembers: PoolMember[]) => PoolMember[],
      patch?: {
        strategy?: AccountPool["strategy"];
        skipActiveClientAccount?: boolean;
        cooldownSeconds?: number;
      },
      toastMsg?: string
    ) => {
      const { dedicatedId, nextPools, nextBindings } = materializeDedicatedPoolForAgent(
        poolsRef.current,
        rawBindingsRef.current,
        candidates,
        agentId
      );

      let orderedAccIds: string[] = [];
      let enabledAccIds: string[] = [];

      const updatedPools = nextPools.map((p) => {
        if (p.id !== dedicatedId) return p;
        const currentToolMembers = p.members
          .filter((m) => m.toolId === toolId)
          .sort((a, b) => a.priority - b.priority);
        const nextToolMembers = transformToolMembers(currentToolMembers);
        orderedAccIds = nextToolMembers.map((m) => m.accountId);
        enabledAccIds = nextToolMembers
          .filter((m) => m.enabled)
          .map((m) => m.accountId);

        const toolPrioritySlots = currentToolMembers
          .map((m) => m.priority)
          .sort((a, b) => a - b);
        const updatedByAcc = new Map<string, PoolMember>();
        nextToolMembers.forEach((m, idx) => {
          updatedByAcc.set(m.accountId, {
            ...m,
            priority: toolPrioritySlots[idx] ?? p.members.length + idx + 1,
          });
        });

        const nextMembers = p.members
          .map((m) => (m.toolId === toolId ? (updatedByAcc.get(m.accountId) ?? m) : m))
          .sort((a, b) => a.priority - b.priority);

        return {
          ...p,
          ...(patch?.strategy !== undefined ? { strategy: patch.strategy } : {}),
          ...(patch?.skipActiveClientAccount !== undefined
            ? { skipActiveClientAccount: patch.skipActiveClientAccount }
            : {}),
          ...(patch?.cooldownSeconds !== undefined
            ? { cooldownSeconds: patch.cooldownSeconds }
            : {}),
          members: nextMembers,
        };
      });

      poolsRef.current = updatedPools;
      rawBindingsRef.current = nextBindings;
      setPools(updatedPools);
      setRawBindings(nextBindings);

      if (toastMsg) {
        notify(toastMsg, "success");
      }

      void syncWorkspaceCommand("update_agent_tool_schedule_cmd", {
        agentId,
        toolId,
        orderedAccountIds: orderedAccIds,
        enabledAccountIds: enabledAccIds,
        strategy: patch?.strategy ?? null,
        skipActiveClientAccount: patch?.skipActiveClientAccount ?? null,
        cooldownSeconds: patch?.cooldownSeconds ?? null,
      });
    },
    [candidates, notify, syncWorkspaceCommand]
  );

  const promoteAgentToolPrimaryAccount = useCallback(
    (agentId: TargetAgentId, toolId: string, accountId: string) => {
      commitAgentToolAccountSchedule(
        agentId,
        toolId,
        (list) => {
          const target = list.find((m) => m.accountId === accountId);
          if (!target) return list;
          const rest = list.filter((m) => m.accountId !== accountId);
          return [{ ...target, enabled: true }, ...rest];
        },
        undefined,
        "已切换为 #1 当前主账号，后续请求将优先消耗该账号"
      );
    },
    [commitAgentToolAccountSchedule]
  );

  const reorderAgentToolAccount = useCallback(
    (agentId: TargetAgentId, toolId: string, accountId: string, direction: "up" | "down") => {
      commitAgentToolAccountSchedule(agentId, toolId, (list) => {
        const idx = list.findIndex((m) => m.accountId === accountId);
        if (idx === -1) return list;
        const swapIdx = direction === "up" ? idx - 1 : idx + 1;
        if (swapIdx < 0 || swapIdx >= list.length) return list;
        const copy = [...list];
        const tmp = copy[idx];
        copy[idx] = copy[swapIdx];
        copy[swapIdx] = tmp;
        return copy;
      });
    },
    [commitAgentToolAccountSchedule]
  );

  const toggleAgentToolAccountEnabled = useCallback(
    (agentId: TargetAgentId, toolId: string, accountId: string) => {
      commitAgentToolAccountSchedule(agentId, toolId, (list) =>
        list.map((m) =>
          m.accountId === accountId ? { ...m, enabled: !m.enabled } : m
        )
      );
    },
    [commitAgentToolAccountSchedule]
  );

  const setAllAgentToolAccountsEnabled = useCallback(
    (agentId: TargetAgentId, toolId: string, onlyPrimary: boolean) => {
      commitAgentToolAccountSchedule(
        agentId,
        toolId,
        (list) =>
          list.map((m, idx) => ({
            ...m,
            enabled: onlyPrimary ? idx === 0 : m.codeArtsMode !== "client_readonly",
          })),
        undefined,
        onlyPrimary ? "已切换为仅启用 #1 主账号（暂停备用号）" : "已启用该工具下全部可用账号参与调度"
      );
    },
    [commitAgentToolAccountSchedule]
  );

  const sortAgentToolAccountsByQuota = useCallback(
    (agentId: TargetAgentId, toolId: string) => {
      commitAgentToolAccountSchedule(
        agentId,
        toolId,
        (list) =>
          [...list].sort(
            (a, b) => b.quotaPercent - a.quotaPercent || a.priority - b.priority
          ),
        undefined,
        "已按当前剩余额度降序重排主备顺位"
      );
    },
    [commitAgentToolAccountSchedule]
  );

  const updateAgentToolRoutingOptions = useCallback(
    (
      agentId: TargetAgentId,
      toolId: string,
      patch: {
        strategy?: AccountPool["strategy"];
        skipActiveClientAccount?: boolean;
        cooldownSeconds?: number;
      }
    ) => {
      commitAgentToolAccountSchedule(
        agentId,
        toolId,
        (list) => list,
        patch,
        "已保存该 Agent 的账号调度方案"
      );
    },
    [commitAgentToolAccountSchedule]
  );

  const syncToolScheduleToAllAgents = useCallback(
    (sourceAgentId: TargetAgentId, toolId: string) => {
      const sourcePool = getAgentPrimaryPool(sourceAgentId);
      const sourceOrder = getAgentToolAccounts(sourceAgentId, toolId);
      const orderedIds = sourceOrder.map((m) => m.accountId);
      const enabledSet = new Set(sourceOrder.filter((m) => m.enabled).map((m) => m.accountId));
      const allAgents = TARGET_AGENT_ORDER;
      for (const targetAgentId of allAgents) {
        if (targetAgentId === sourceAgentId) continue;
        commitAgentToolAccountSchedule(
          targetAgentId,
          toolId,
          (list) => {
            const byId = new Map(list.map((m) => [m.accountId, m]));
            const sorted: PoolMember[] = [];
            for (const aid of orderedIds) {
              const found = byId.get(aid);
              if (found) {
                sorted.push({ ...found, enabled: enabledSet.has(aid) });
                byId.delete(aid);
              }
            }
            for (const rem of byId.values()) {
              sorted.push(rem);
            }
            return sorted;
          },
          sourcePool
            ? {
                strategy: sourcePool.strategy,
                skipActiveClientAccount: sourcePool.skipActiveClientAccount,
                cooldownSeconds: sourcePool.cooldownSeconds,
              }
            : undefined
        );
      }
      notify(
        `已将「${TOOL_SHORT_LABELS[toolId] ?? toolId}」的主备顺位同步至全部 Agent`,
        "success"
      );
    },
    [commitAgentToolAccountSchedule, getAgentPrimaryPool, getAgentToolAccounts, notify]
  );

  const value = useMemo<PoolPrototypeContextValue>(
    () => ({
      isTauriRuntime,
      gateway,
      pools,
      selectedPoolId,
      setSelectedPoolId,
      candidates,
      agentBindings,
      traces,
      nowMs,
      isRefreshingModels,
      refreshUpstreamModels,
      toggleGateway,
      updateGatewayPort,
      cycleBridgeRuntime,
      toggleProxySimulation,
      createPool,
      updatePool,
      deletePool,
      togglePoolEnabled,
      reorderPoolMember,
      togglePoolMember,
      togglePoolMemberModel,
      updatePoolMemberTier,
      togglePoolModelEnabled,
      setPoolModelFallback,
      addMembersToPool,
      removePoolMember,
      injectMemberCooldown,
      clearMemberCooldown,
      authorizeCodeArtsIsolated,
      updateAgentMountedPools,
      setAgentDefaultModel,
      mountPoolToAgents,
      unmountPoolFromAgent,
      unbindAgent,
      repairAgentDrift,
      restartAgentClient,
      toggleAgentOption,
      detectAgentEnvironment,
      runPoolSimulation,
      getAgentToolModelOptions,
      getAgentToolEnabledModelIds,
      getAgentToolAccounts,
      getAgentPrimaryPool,
      setAgentToolModels,
      selectAllAgentToolModels,
      selectRecommendedAgentToolModels,
      selectLowRateAgentToolModels,
      invertAgentToolModels,
      clearAgentToolModels,
      setAgentDefaultToolModel,
      promoteAgentToolPrimaryAccount,
      reorderAgentToolAccount,
      toggleAgentToolAccountEnabled,
      setAllAgentToolAccountsEnabled,
      sortAgentToolAccountsByQuota,
      updateAgentToolRoutingOptions,
      syncToolScheduleToAllAgents,
    }),
    [
      isTauriRuntime,
      gateway,
      pools,
      selectedPoolId,
      candidates,
      agentBindings,
      traces,
      nowMs,
      isRefreshingModels,
      refreshUpstreamModels,
      toggleGateway,
      updateGatewayPort,
      cycleBridgeRuntime,
      toggleProxySimulation,
      createPool,
      updatePool,
      deletePool,
      togglePoolEnabled,
      reorderPoolMember,
      togglePoolMember,
      togglePoolMemberModel,
      updatePoolMemberTier,
      togglePoolModelEnabled,
      setPoolModelFallback,
      addMembersToPool,
      removePoolMember,
      injectMemberCooldown,
      clearMemberCooldown,
      authorizeCodeArtsIsolated,
      updateAgentMountedPools,
      setAgentDefaultModel,
      mountPoolToAgents,
      unmountPoolFromAgent,
      unbindAgent,
      repairAgentDrift,
      restartAgentClient,
      toggleAgentOption,
      detectAgentEnvironment,
      runPoolSimulation,
      getAgentToolModelOptions,
      getAgentToolEnabledModelIds,
      getAgentToolAccounts,
      getAgentPrimaryPool,
      setAgentToolModels,
      selectAllAgentToolModels,
      selectRecommendedAgentToolModels,
      selectLowRateAgentToolModels,
      invertAgentToolModels,
      clearAgentToolModels,
      setAgentDefaultToolModel,
      promoteAgentToolPrimaryAccount,
      reorderAgentToolAccount,
      toggleAgentToolAccountEnabled,
      setAllAgentToolAccountsEnabled,
      sortAgentToolAccountsByQuota,
      updateAgentToolRoutingOptions,
      syncToolScheduleToAllAgents,
    ]
  );

  return (
    <PoolPrototypeContext.Provider value={value}>
      {children}
    </PoolPrototypeContext.Provider>
  );
}

export function usePoolPrototype(): PoolPrototypeContextValue {
  const ctx = useContext(PoolPrototypeContext);
  return ctx ?? DEFAULT_FALLBACK_VALUE;
}
