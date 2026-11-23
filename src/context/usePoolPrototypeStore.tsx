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
  UPSTREAM_MODEL_OPTIONS,
  buildAgentConfigSnippets,
  computeAgentExposedModels,
  createInitialAgentBindings,
  makeDefaultModelKey,
  syncPoolModelPolicies,
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
  pools: INITIAL_POOLS,
  selectedPoolId: INITIAL_POOLS[0].id,
  setSelectedPoolId: () => {},
  candidates: AVAILABLE_UPSTREAM_CANDIDATES,
  agentBindings: createInitialAgentBindings(INITIAL_POOLS, INITIAL_GATEWAY_STATE),
  traces: INITIAL_ROUTING_TRACES,
  nowMs: Date.now(),
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
      return "openai_chat";
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

export function PoolPrototypeProvider({
  children,
  onToast,
}: {
  children: React.ReactNode;
  onToast?: (message: string, tone?: ToastTone) => void;
}) {
  const isTauriRuntime = detectTauriRuntime();
  const [gateway, setGateway] = useState<GatewayRuntimeState>(() =>
    isTauriRuntime
      ? { ...INITIAL_GATEWAY_STATE, running: false }
      : INITIAL_GATEWAY_STATE
  );
  const [pools, setPools] = useState<AccountPool[]>(() =>
    isTauriRuntime ? [] : INITIAL_POOLS
  );
  const [selectedPoolId, setSelectedPoolId] = useState<string>(() =>
    isTauriRuntime ? "" : INITIAL_POOLS[0].id
  );
  const [candidates, setCandidates] = useState<UpstreamAccountCandidate[]>(() =>
    isTauriRuntime ? [] : AVAILABLE_UPSTREAM_CANDIDATES
  );
  const [rawBindings, setRawBindings] = useState<AgentBinding[]>(() =>
    isTauriRuntime
      ? createInitialAgentBindings([], INITIAL_GATEWAY_STATE)
      : createInitialAgentBindings(INITIAL_POOLS, INITIAL_GATEWAY_STATE)
  );
  const [traces, setTraces] = useState<RoutingTraceItem[]>(() =>
    isTauriRuntime ? [] : INITIAL_ROUTING_TRACES
  );
  const [nowMs, setNowMs] = useState<number>(() => Date.now());
  const sessionAffinityRef = useRef<Record<string, string>>({});

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

  const syncWorkspaceCommand = useCallback(
    async (command: string, args?: Record<string, unknown>) => {
      if (!isTauriRuntime) return;
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const ws = await invoke<PoolWorkspaceState>(command, args);
        applyWorkspaceState(ws);
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
    return rawBindings.map((binding) => {
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
      notify(`OpenCode 插件桥接运行时已切换为${isTauriRuntime ? "" : "模拟态："} ${version}`, "info");
      return { ...prev, bridgeRuntime: next, bridgeVersion: version };
    });
    void syncWorkspaceCommand("cycle_bridge_runtime_cmd");
  }, [isTauriRuntime, notify, syncWorkspaceCommand]);

  const toggleProxySimulation = useCallback(() => {
    setGateway((prev) => {
      const next = !prev.globalProxyDetected;
      notify(
        next
          ? "已模拟开启系统全局代理：Codex 桌面版将提示 502 预警"
          : "已模拟关闭系统全局代理 / 放行 127.0.0.1",
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
        `pool-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
      const slug = slugifyPoolName(input.name);
      const members: PoolMember[] = input.members.map((item, idx) => ({
        id: `pm-${Date.now().toString(36)}-${idx}`,
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
            id: `pm-${Date.now().toString(36)}-${idx}`,
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
      notify("已对该账号注入模拟 429 冷却期，后续请求将自动跳过该成员", "warning");
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
          const needsRestart = agentId === "claude_desktop" || agentId === "codex_desktop";
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
            b.agentId === "claude_desktop" || b.agentId === "codex_desktop";
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
        notify("已精准重启 Codex app-server 后台进程（桌面窗口保持打开未中断）", "success");
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
          id: `tr-${Date.now().toString(36)}`,
          timestamp: formatTimeHms(new Date(now)),
          agentId: input.agentId,
          poolId: input.poolId,
          poolName: input.poolId,
          incomingProtocol,
          upstreamProtocol: "openai_chat",
          requestedModel: input.modelId || "claude-sonnet-4.6",
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
        "claude-sonnet-4.6";

      const collectEligibleForModel = (
        targetModel: string,
        collectAttempts: RoutingAttempt[]
      ): PoolMember[] => {
        const tierBuckets: Record<MemberTier, PoolMember[]> = {
          t1_primary: [],
          t2_backup: [],
          t3_fallback: [],
        };

        for (const m of pool.members) {
          if (!m.supportedModels.includes(targetModel)) continue;
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
        id: `tr-${Date.now().toString(36)}`,
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

      notify(
        status === "failover_success"
          ? `模拟演练完成：已切至「${finalMember?.label}」(${effectiveModel})`
          : status === "success"
            ? `模拟请求命中「${finalMember?.label}」(${effectiveModel} · ${totalLatency}ms)`
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
