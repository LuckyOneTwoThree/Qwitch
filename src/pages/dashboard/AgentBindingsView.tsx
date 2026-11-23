import { useMemo, useState } from "react";
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  Copy,
  FileCode2,
  Layers,
  Power,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  ShieldCheck,
  Star,
  Terminal,
  Unlink,
  Wrench,
} from "lucide-react";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import {
  WIRE_PROTOCOL_META,
  buildAgentConfigSnippets,
  computeAgentExposedModels,
} from "../../data/mockPoolData";
import type { AgentBinding, TargetAgentId } from "../../types/pool";
import { AgentBrandIcon } from "../../components/AgentBrandIcon";
import { ConfigDiffModal, type DiffActionKind } from "../../components/pool/ConfigDiffModal";

function renderStatusBadge(status: AgentBinding["status"]) {
  switch (status) {
    case "bound":
      return (
        <span className="badge border-emerald-500/30 bg-emerald-500/12 text-emerald-800 dark:text-emerald-300">
          <CheckCircle2 className="h-3 w-3" aria-hidden="true" />
          已挂载并同步
        </span>
      );
    case "drifted":
      return (
        <span className="badge border-amber-500/35 bg-amber-500/15 text-amber-800 dark:text-amber-300">
          <AlertTriangle className="h-3 w-3" aria-hidden="true" />
          配置漂移
        </span>
      );
    case "needs_restart":
      return (
        <span className="badge border-sky-500/30 bg-sky-500/12 text-sky-800 dark:text-sky-300">
          <RotateCcw className="h-3 w-3" aria-hidden="true" />
          待重启生效
        </span>
      );
    case "unbound":
      return <span className="badge badge-neutral">未挂载（原生运行）</span>;
  }
}

export function AgentBindingsView({
  highlightAgentId,
  onNavigatePools,
}: {
  highlightAgentId?: TargetAgentId | null;
  onNavigatePools: (poolId?: string) => void;
}) {
  const {
    gateway,
    pools,
    agentBindings,
    nowMs,
    toggleGateway,
    cycleBridgeRuntime,
    toggleProxySimulation,
    updateAgentMountedPools,
    setAgentDefaultModel,
    unbindAgent,
    repairAgentDrift,
    restartAgentClient,
    toggleAgentOption,
    detectAgentEnvironment,
  } = usePoolPrototype();

  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [customPoolId, setCustomPoolId] = useState<string>(pools[0]?.id ?? "");
  const [customModelId, setCustomModelId] = useState<string>(
    pools[0]?.modelPolicies.find((m) => m.enabled)?.modelId ?? "claude-sonnet-4.6"
  );
  const [diffModal, setDiffModal] = useState<{
    binding: AgentBinding;
    actionKind: DiffActionKind;
    nextPoolIds?: string[];
    mountedPoolNames: string[];
    defaultModelDisplay?: string | null;
    beforeText: string;
    afterText: string;
  } | null>(null);

  const selectedCustomPool = useMemo(
    () => pools.find((p) => p.id === customPoolId) ?? pools[0] ?? null,
    [pools, customPoolId]
  );

  const selectedCustomModels = useMemo(
    () => selectedCustomPool?.modelPolicies.filter((m) => m.enabled) ?? [],
    [selectedCustomPool]
  );

  const activeCustomModel = useMemo(() => {
    if (selectedCustomModels.some((m) => m.modelId === customModelId)) {
      return customModelId;
    }
    return selectedCustomModels[0]?.modelId ?? "claude-sonnet-4.6";
  }, [selectedCustomModels, customModelId]);

  const copyText = (id: string, text: string) => {
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      void navigator.clipboard.writeText(text);
    }
    setCopiedId(id);
    window.setTimeout(() => {
      setCopiedId((prev) => (prev === id ? null : prev));
    }, 1500);
  };

  const openDiffForMountChange = (binding: AgentBinding, nextPoolIds: string[]) => {
    const snippets = buildAgentConfigSnippets(
      binding.agentId,
      nextPoolIds,
      binding.defaultModelKey,
      pools,
      gateway,
      binding.options,
      false
    );
    const exposed = computeAgentExposedModels(
      {
        agentId: binding.agentId,
        mountedPoolIds: nextPoolIds,
        defaultModelKey: binding.defaultModelKey,
      },
      pools,
      nowMs
    );
    const defaultItem = exposed.find((e) => e.isDefault) ?? exposed[0] ?? null;
    setDiffModal({
      binding,
      actionKind: nextPoolIds.length === 0 ? "unbind" : "update_mounts",
      nextPoolIds,
      mountedPoolNames: nextPoolIds
        .map((id) => pools.find((p) => p.id === id)?.name)
        .filter((n): n is string => Boolean(n)),
      defaultModelDisplay: defaultItem?.displayName ?? null,
      beforeText: snippets.beforeSnippet,
      afterText: snippets.afterSnippet,
    });
  };

  const noProxyCmd = "export NO_PROXY=127.0.0.1,localhost,::1";
  const customEndpointEnv = selectedCustomPool
    ? `export OPENAI_BASE_URL=http://${gateway.host}:${gateway.port}/v1
export OPENAI_API_KEY=qwitch-bind-custom-${selectedCustomPool.slug}
# 指定请求模型（严格锁模不私换）：${selectedCustomPool.slug}/${activeCustomModel}
# 或 Anthropic Messages 协议:
export ANTHROPIC_BASE_URL=http://${gateway.host}:${gateway.port}
export ANTHROPIC_AUTH_TOKEN=qwitch-bind-custom-${selectedCustomPool.slug}`
    : "";

  return (
    <div className="space-y-4">
      {/* 顶部环境与网关自检条 */}
      <section className="surface-card flex flex-wrap items-center justify-between gap-3 px-4 py-3">
        <div className="flex flex-wrap items-center gap-2.5 text-xs">
          <div className="flex items-center gap-2">
            <span
              className={`h-2 w-2 rounded-full ${
                gateway.running ? "bg-emerald-500" : "bg-zinc-400"
              }`}
            />
            <span className="font-semibold text-zinc-900 dark:text-zinc-100">
              本地网关
            </span>
            <code className="rounded bg-zinc-100 px-1.5 py-0.5 font-mono text-[11px] tabular-nums text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300">
              {gateway.host}:{gateway.port}
            </code>
          </div>

          <button
            type="button"
            onClick={cycleBridgeRuntime}
            title="点击可模拟切换 OpenCode 插件桥运行时检测态"
            className={`badge cursor-pointer ${
              gateway.bridgeRuntime === "missing"
                ? "border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300"
                : "border-indigo-500/25 bg-indigo-500/10 text-indigo-700 dark:text-indigo-300"
            }`}
          >
            插件桥宿主：{gateway.bridgeVersion}
          </button>

          <button
            type="button"
            onClick={toggleProxySimulation}
            title="点击可切换模拟系统全局代理状态（演示 Codex 502 检测）"
            className={`badge cursor-pointer ${
              gateway.globalProxyDetected
                ? "border-amber-500/35 bg-amber-500/12 text-amber-800 dark:text-amber-300"
                : "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
            }`}
          >
            {gateway.globalProxyDetected ? "⚠️ 检测到全局代理" : "✓ 本机回环直连正常"}
          </button>
        </div>

        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={detectAgentEnvironment}
            className="btn btn-sm btn-secondary gap-1.5"
          >
            <RefreshCw className="h-3.5 w-3.5" aria-hidden="true" />
            重新探测环境
          </button>
          <button
            type="button"
            onClick={toggleGateway}
            className={`btn btn-sm gap-1.5 ${
              gateway.running ? "btn-secondary" : "btn-primary"
            }`}
          >
            <Power className="h-3.5 w-3.5" aria-hidden="true" />
            {gateway.running ? "停止网关" : "启动网关"}
          </button>
        </div>
      </section>

      {/* 4 大目标消费端 Agent 挂载卡片 */}
      <div className="grid grid-cols-1 gap-4 @3xl:grid-cols-2">
        {agentBindings.map((binding) => {
          const isHighlighted = highlightAgentId === binding.agentId;
          const exposedModels = computeAgentExposedModels(binding, pools, nowMs);

          return (
            <section
              key={binding.agentId}
              aria-label={`${binding.name} 挂载卡片`}
              className={`surface-card flex flex-col justify-between gap-3.5 p-4 transition ${
                isHighlighted
                  ? "border-brand-500 ring-2 ring-brand-500/25"
                  : binding.status === "drifted"
                    ? "border-amber-500/50"
                    : ""
              }`}
            >
              <div className="space-y-3">
                {/* 头部 */}
                <div className="flex items-start justify-between gap-2.5">
                  <div className="flex min-w-0 items-center gap-2.5">
                    <AgentBrandIcon agentId={binding.agentId} size="md" />
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <h2 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                          {binding.name}
                        </h2>
                        <span className="font-mono text-[11px] text-zinc-500 dark:text-zinc-400">
                          v{binding.version}
                        </span>
                      </div>
                      <p className="truncate text-[11px] text-zinc-500 dark:text-zinc-400">
                        {binding.subtitle}
                      </p>
                    </div>
                  </div>
                  {renderStatusBadge(binding.status)}
                </div>

                {/* 协议与路径 */}
                <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl bg-zinc-50 px-3 py-2 text-[11px] dark:bg-zinc-950/60">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className={`badge ${WIRE_PROTOCOL_META[binding.protocol].badgeClass}`}>
                      {WIRE_PROTOCOL_META[binding.protocol].label}
                    </span>
                    <span className="truncate font-mono text-zinc-500 dark:text-zinc-400">
                      {binding.configPath}
                    </span>
                  </div>
                  {binding.lastHitAt && (
                    <span className="shrink-0 text-zinc-400">
                      最近调用：{binding.lastHitAt}
                    </span>
                  )}
                </div>

                {/* 多号池勾选挂载区 */}
                <div className="space-y-1.5">
                  <div className="flex items-center justify-between text-xs">
                    <span className="flex items-center gap-1.5 font-medium text-zinc-700 dark:text-zinc-300">
                      <Layers className="h-3.5 w-3.5 text-brand-600 dark:text-brand-400" aria-hidden="true" />
                      挂载号池组合（可多选 · 共挂 {binding.mountedPoolIds.length} 池）
                    </span>
                    {binding.mountedPoolIds[0] && (
                      <button
                        type="button"
                        onClick={() => onNavigatePools(binding.mountedPoolIds[0])}
                        className="text-[11px] font-medium text-brand-700 hover:underline dark:text-brand-300"
                      >
                        去号池编排 →
                      </button>
                    )}
                  </div>

                  <div className="flex flex-wrap gap-1.5">
                    {pools.map((pool) => {
                      const mounted = binding.mountedPoolIds.includes(pool.id);
                      const enabledModelCount = pool.modelPolicies.filter((m) => m.enabled).length;
                      return (
                        <button
                          key={pool.id}
                          type="button"
                          aria-pressed={mounted}
                          aria-label={`${binding.name} 挂载号池 ${pool.name}`}
                          onClick={() => {
                            const nextPoolIds = mounted
                              ? binding.mountedPoolIds.filter((id) => id !== pool.id)
                              : [...binding.mountedPoolIds, pool.id];
                            updateAgentMountedPools(binding.agentId, nextPoolIds);
                          }}
                          className={`inline-flex items-center gap-1.5 rounded-xl border px-2.5 py-1.5 text-xs transition ${
                            mounted
                              ? "border-brand-500/60 bg-brand-500/10 font-medium text-brand-900 dark:border-brand-400/50 dark:text-brand-100"
                              : "border-zinc-200 bg-white text-zinc-600 hover:bg-zinc-50 dark:border-zinc-700 dark:bg-zinc-900/60 dark:text-zinc-300"
                          }`}
                        >
                          <span
                            className={`flex h-3.5 w-3.5 items-center justify-center rounded border ${
                              mounted
                                ? "border-brand-600 bg-brand-600 text-white dark:border-brand-400 dark:bg-brand-400 dark:text-zinc-950"
                                : "border-zinc-300 dark:border-zinc-600"
                            }`}
                          >
                            {mounted && <Check className="h-2.5 w-2.5" aria-hidden="true" />}
                          </span>
                          <span>{pool.name}</span>
                          <span className="rounded bg-black/5 px-1 text-[10px] tabular-nums text-zinc-500 dark:bg-white/10 dark:text-zinc-400">
                            {enabledModelCount} 模型
                          </span>
                        </button>
                      );
                    })}
                  </div>
                </div>

                {/* 客户端模型下拉菜单实时预览 + ★ 默认模型点选 */}
                <div className="space-y-1.5 rounded-xl border border-zinc-200/80 bg-zinc-50/75 p-2.5 dark:border-zinc-800 dark:bg-zinc-950/45">
                  <div className="flex items-center justify-between gap-2 text-[11px]">
                    <span className="font-semibold text-zinc-800 dark:text-zinc-200">
                      客户端模型下拉菜单实时预览（共 {exposedModels.length} 项）
                    </span>
                    <span className="text-zinc-400">点按可设 ★ 默认模型</span>
                  </div>

                  {exposedModels.length === 0 ? (
                    <div className="rounded-lg border border-dashed border-zinc-200 py-3 text-center text-[11px] text-zinc-400 dark:border-zinc-800">
                      尚未挂载号池；客户端仅保留原厂配置
                    </div>
                  ) : (
                    <div className="max-h-36 space-y-1 overflow-y-auto pr-0.5">
                      {exposedModels.map((item) => (
                        <div
                          key={item.key}
                          className={`flex flex-wrap items-center justify-between gap-2 rounded-lg border px-2.5 py-1.5 text-[11px] transition ${
                            item.isDefault
                              ? "border-amber-500/45 bg-amber-500/[0.08] dark:border-amber-400/40"
                              : "border-zinc-200/80 bg-white dark:border-zinc-800 dark:bg-zinc-900/70"
                          }`}
                        >
                          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
                            <code className="font-mono font-semibold text-zinc-900 dark:text-zinc-100">
                              {item.displayName}
                            </code>
                            <span className="rounded bg-zinc-100 px-1.5 py-0.5 text-[10px] text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                              {item.poolName} · {item.healthyAccountCount}/{item.accountCount}号
                            </span>
                            {binding.agentId === "claude_desktop" && item.isClaudeDisguised && (
                              <span
                                className="font-mono text-[10px] text-[#B8502C] dark:text-[#F29C7D]"
                                title="底层通过此伪装 ID 绕过 Claude Desktop 正则黑名单"
                              >
                                ({item.wireModelId})
                              </span>
                            )}
                          </div>

                          {item.isDefault ? (
                            <span className="inline-flex shrink-0 items-center gap-1 font-semibold text-amber-700 dark:text-amber-300">
                              <Star className="h-3 w-3 fill-current" aria-hidden="true" />
                              默认模型
                            </span>
                          ) : (
                            <button
                              type="button"
                              onClick={() => setAgentDefaultModel(binding.agentId, item.key)}
                              className="shrink-0 font-medium text-brand-700 hover:underline dark:text-brand-300"
                            >
                              设为默认
                            </button>
                          )}
                        </div>
                      ))}
                    </div>
                  )}
                </div>

                {/* 专属告警与诊断区 */}
                {binding.status === "drifted" && binding.driftReason && (
                  <div className="flex items-start gap-2 rounded-xl border border-amber-500/35 bg-amber-500/10 p-2.5 text-[11px] text-amber-900 dark:text-amber-200">
                    <ShieldAlert className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    <div className="min-w-0 flex-1">
                      <p className="leading-relaxed">{binding.driftReason}</p>
                    </div>
                  </div>
                )}

                {binding.status === "needs_restart" && binding.restartHint && (
                  <div className="flex items-start justify-between gap-2 rounded-xl border border-sky-500/30 bg-sky-500/10 p-2.5 text-[11px] text-sky-900 dark:text-sky-200">
                    <span>{binding.restartHint}</span>
                    <button
                      type="button"
                      onClick={() => restartAgentClient(binding.agentId)}
                      className="btn btn-xs btn-sky-soft shrink-0"
                    >
                      {binding.agentId === "codex_desktop" ? "重启 app-server" : "立即重启"}
                    </button>
                  </div>
                )}

                {binding.proxyWarning && (
                  <div className="flex items-start justify-between gap-2 rounded-xl border border-amber-500/30 bg-amber-500/[0.07] p-2.5 text-[11px] text-amber-900 dark:text-amber-200">
                    <span>{binding.proxyWarning}</span>
                    <button
                      type="button"
                      onClick={() => copyText("noproxy", noProxyCmd)}
                      className="btn btn-xs btn-secondary shrink-0 gap-1"
                    >
                      {copiedId === "noproxy" ? (
                        <Check className="h-3 w-3 text-emerald-500" aria-hidden="true" />
                      ) : (
                        <Copy className="h-3 w-3" aria-hidden="true" />
                      )}
                      复制 NO_PROXY
                    </button>
                  </div>
                )}

                {/* 各 Agent 专属技术细节控制 */}
                {binding.agentId === "claude_desktop" && (
                  <div className="space-y-1.5 rounded-xl border border-zinc-200/80 bg-zinc-50/70 p-2.5 text-[11px] dark:border-zinc-800 dark:bg-zinc-950/40">
                    <label className="flex cursor-pointer items-center justify-between gap-2">
                      <span className="font-medium text-zinc-700 dark:text-zinc-300">
                        macOS 独立双开隔离副本（保留官方 Claude.app 登录态）
                      </span>
                      <input
                        type="checkbox"
                        checked={Boolean(binding.options.claudeIsolatedTwin)}
                        onChange={() =>
                          toggleAgentOption("claude_desktop", "claudeIsolatedTwin")
                        }
                        className="rounded border-zinc-300 text-brand-600 focus:ring-brand-500"
                      />
                    </label>
                    <p className="text-[10px] text-zinc-500 dark:text-zinc-400">
                      说明：<code>claude-*</code> 名直过客户端白名单，非 Anthropic 名（如 deepseek / MiniMax）自动在底层伪装为{" "}
                      <code>mythos-qwitch-*</code> 并保留 Thinking Effort 滑块。
                    </p>
                  </div>
                )}

                {binding.agentId === "codex_desktop" && (
                  <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-zinc-200/80 bg-zinc-50/70 p-2.5 text-[11px] dark:border-zinc-800 dark:bg-zinc-950/40">
                    <span className="flex items-center gap-1 text-zinc-600 dark:text-zinc-400">
                      <ShieldCheck className="h-3.5 w-3.5 text-emerald-500" aria-hidden="true" />
                      WS 升级返回 426 自动降级 HTTP SSE · 解绑保留 provider 表
                    </span>
                    {binding.mountedPoolIds.length > 0 && (
                      <button
                        type="button"
                        onClick={() => restartAgentClient("codex_desktop")}
                        className="btn btn-xs btn-secondary"
                      >
                        重启 app-server
                      </button>
                    )}
                  </div>
                )}

                {binding.agentId === "dsh" && (
                  <div className="flex items-center justify-between gap-2 rounded-xl border border-zinc-200/80 bg-zinc-50/70 p-2.5 text-[11px] dark:border-zinc-800 dark:bg-zinc-950/40">
                    <span className="text-zinc-600 dark:text-zinc-400">
                      双凭据校验：<code>.env</code> 已写入 ·{" "}
                      <code>.credentials.yaml</code>{" "}
                      {binding.options.dshDualCredsWritten ? (
                        <span className="font-semibold text-emerald-600 dark:text-emerald-400">
                          已同步
                        </span>
                      ) : (
                        <span className="font-semibold text-amber-600 dark:text-amber-400">
                          缺失（桌面版不可用）
                        </span>
                      )}
                    </span>
                  </div>
                )}

                {binding.agentId === "opencode" && (
                  <div className="flex items-center justify-between gap-2 rounded-xl border border-zinc-200/80 bg-zinc-50/70 p-2.5 text-[11px] dark:border-zinc-800 dark:bg-zinc-950/40">
                    <span className="text-zinc-600 dark:text-zinc-400">
                      配置 Schema 格式（避免 v2 忽略 v1 provider）
                    </span>
                    <button
                      type="button"
                      onClick={() =>
                        toggleAgentOption("opencode", "opencodeSchemaVersion")
                      }
                      className="chip border border-zinc-200 bg-white font-mono dark:border-zinc-700 dark:bg-zinc-800"
                    >
                      {binding.options.opencodeSchemaVersion === "v1"
                        ? "v1 (provider)"
                        : "v2 (providers)"}
                    </button>
                  </div>
                )}
              </div>

              {/* 底部动作栏 */}
              <div className="flex flex-wrap items-center justify-between gap-2 border-t border-zinc-200/70 pt-3 dark:border-zinc-800">
                <button
                  type="button"
                  onClick={() =>
                    setDiffModal({
                      binding,
                      actionKind: "preview",
                      mountedPoolNames: binding.mountedPoolIds
                        .map((id) => pools.find((p) => p.id === id)?.name)
                        .filter((n): n is string => Boolean(n)),
                      defaultModelDisplay:
                        exposedModels.find((m) => m.isDefault)?.displayName ?? null,
                      beforeText: binding.beforeSnippet,
                      afterText: binding.afterSnippet,
                    })
                  }
                  className="btn btn-xs btn-secondary gap-1"
                >
                  <FileCode2 className="h-3.5 w-3.5" aria-hidden="true" />
                  预览配置 Diff
                </button>

                <div className="flex items-center gap-1.5">
                  {binding.status === "drifted" && (
                    <button
                      type="button"
                      onClick={() =>
                        setDiffModal({
                          binding,
                          actionKind: "repair",
                          nextPoolIds: binding.mountedPoolIds,
                          mountedPoolNames: binding.mountedPoolIds
                            .map((id) => pools.find((p) => p.id === id)?.name)
                            .filter((n): n is string => Boolean(n)),
                          beforeText: binding.beforeSnippet,
                          afterText: binding.afterSnippet,
                        })
                      }
                      className="btn btn-xs btn-primary gap-1"
                    >
                      <Wrench className="h-3 w-3" aria-hidden="true" />
                      查看差异并修复
                    </button>
                  )}

                  {binding.mountedPoolIds.length > 0 ? (
                    <button
                      type="button"
                      onClick={() => openDiffForMountChange(binding, [])}
                      className="btn btn-xs btn-danger gap-1"
                    >
                      <Unlink className="h-3 w-3" aria-hidden="true" />
                      全部解绑并还原
                    </button>
                  ) : (
                    <button
                      type="button"
                      disabled={!pools[0]}
                      onClick={() =>
                        pools[0] && openDiffForMountChange(binding, [pools[0].id])
                      }
                      className="btn btn-xs btn-primary gap-1"
                    >
                      预览并挂载号池
                    </button>
                  )}
                </div>
              </div>
            </section>
          );
        })}
      </div>

      {/* 通用标准端点生成卡 */}
      <section className="surface-card p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2.5">
            <AgentBrandIcon agentId="custom_endpoint" size="sm" />
            <div>
              <h2 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                通用标准网关端点（适用于任意兼容 OpenAI / Anthropic 的客户端）
              </h2>
              <p className="text-[11px] text-zinc-500 dark:text-zinc-400">
                选定号池与具体模型后直接生成接入环境变量，支持标准 OpenAI / Anthropic 协议客户端
              </p>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <select
              value={selectedCustomPool?.id ?? ""}
              onChange={(e) => setCustomPoolId(e.target.value)}
              aria-label="选择导出环境变量的号池"
              className="rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-800"
            >
              {pools.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
            <select
              value={activeCustomModel}
              onChange={(e) => setCustomModelId(e.target.value)}
              aria-label="选择导出环境变量的目标模型"
              className="rounded-lg border border-zinc-200 bg-white px-2.5 py-1.5 font-mono text-xs dark:border-zinc-700 dark:bg-zinc-800"
            >
              {selectedCustomModels.map((m) => (
                <option key={m.modelId} value={m.modelId}>
                  {m.modelId}
                </option>
              ))}
            </select>
            <button
              type="button"
              onClick={() => copyText("customEnv", customEndpointEnv)}
              className="btn btn-sm btn-secondary gap-1.5"
            >
              {copiedId === "customEnv" ? (
                <Check className="h-3.5 w-3.5 text-emerald-500" aria-hidden="true" />
              ) : (
                <Terminal className="h-3.5 w-3.5" aria-hidden="true" />
              )}
              {copiedId === "customEnv" ? "已复制环境变量" : "复制环境变量"}
            </button>
          </div>
        </div>

        <pre className="mt-3 overflow-x-auto rounded-xl border border-zinc-200/80 bg-zinc-50 p-3 font-mono text-[11px] leading-relaxed text-zinc-700 dark:border-zinc-800 dark:bg-zinc-950 dark:text-zinc-300">
          {customEndpointEnv}
        </pre>
      </section>

      {diffModal && (
        <ConfigDiffModal
          open={Boolean(diffModal)}
          binding={diffModal.binding}
          actionKind={diffModal.actionKind}
          targetSummary={{
            mountedPoolNames: diffModal.mountedPoolNames,
            defaultModelDisplay: diffModal.defaultModelDisplay,
          }}
          beforeText={diffModal.beforeText}
          afterText={diffModal.afterText}
          onClose={() => setDiffModal(null)}
          onConfirm={
            diffModal.actionKind === "preview"
              ? undefined
              : () => {
                  if (diffModal.actionKind === "repair") {
                    repairAgentDrift(diffModal.binding.agentId);
                  } else if (diffModal.actionKind === "unbind") {
                    unbindAgent(diffModal.binding.agentId);
                  } else if (
                    diffModal.actionKind === "update_mounts" &&
                    diffModal.nextPoolIds
                  ) {
                    updateAgentMountedPools(
                      diffModal.binding.agentId,
                      diffModal.nextPoolIds
                    );
                  }
                }
          }
        />
      )}
    </div>
  );
}
