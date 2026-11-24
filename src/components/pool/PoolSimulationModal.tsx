import { useEffect, useId, useMemo, useRef, useState } from "react";
import {
  ArrowRight,
  CheckCircle2,
  Cpu,
  Flame,
  Play,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  Sparkles,
  X,
} from "lucide-react";
import { Modal } from "../ui/Modal";
import { usePoolPrototype } from "../../context/usePoolPrototypeStore";
import {
  MEMBER_TIER_META,
  POOL_STRATEGY_META,
  WIRE_PROTOCOL_META,
  computePoolServingAccounts,
} from "../../data/mockPoolData";
import type {
  RoutingTraceItem,
  RunPoolSimulationInput,
  TargetAgentId,
} from "../../types/pool";
import { AgentBrandIcon, UpstreamMemberIcon } from "../AgentBrandIcon";

const PRESET_PROMPTS = [
  "重构 src-tauri/src/gateway/router.rs，加入首字节前 Failover 单测",
  "分析 Claude Desktop 3p 模式下模型 ID 黑名单正则并生成伪装映射",
  "检查 DeepSeek Harness 桌面端凭据读取路径与双写逻辑",
];

export function PoolSimulationModal({
  open,
  defaultPoolId,
  defaultModelId,
  onClose,
  onOpenRoutingView,
}: {
  open: boolean;
  defaultPoolId?: string;
  defaultModelId?: string;
  onClose: () => void;
  onOpenRoutingView?: () => void;
}) {
  const titleId = useId();
  const { pools, agentBindings, gateway, nowMs, runPoolSimulation } = usePoolPrototype();

  const [agentId, setAgentId] = useState<TargetAgentId>("dsh");
  const [poolId, setPoolId] = useState<string>(defaultPoolId || pools[0]?.id || "");
  const [modelId, setModelId] = useState<string>(defaultModelId || "");
  const [prompt, setPrompt] = useState<string>(PRESET_PROMPTS[0]);
  const [failureMode, setFailureMode] =
    useState<RunPoolSimulationInput["failureMode"]>("429_first");
  const [sessionSeq, setSessionSeq] = useState<number>(1);
  const [turnCount, setTurnCount] = useState<number>(0);
  const [runningStep, setRunningStep] = useState<number>(0);
  const [resultTrace, setResultTrace] = useState<RoutingTraceItem | null>(null);

  const poolsRef = useRef(pools);
  poolsRef.current = pools;

  useEffect(() => {
    if (!open) return;
    const pickedPoolId = defaultPoolId || poolsRef.current[0]?.id || "";
    const pickedPool = poolsRef.current.find((p) => p.id === pickedPoolId) ?? poolsRef.current[0];
    const pickedModel =
      defaultModelId ||
      pickedPool?.modelPolicies.find((mp) => mp.enabled)?.modelId ||
      pickedPool?.modelPolicies[0]?.modelId ||
      "";
    setPoolId(pickedPoolId);
    setModelId(pickedModel);
    setResultTrace(null);
    setRunningStep(0);
    setTurnCount(0);
    setSessionSeq((s) => s + 1);
  }, [open, defaultPoolId, defaultModelId]);

  useEffect(() => {
    if (runningStep <= 0 || runningStep >= 5) return;
    const timer = window.setTimeout(() => {
      setRunningStep((s) => s + 1);
    }, 180);
    return () => window.clearTimeout(timer);
  }, [runningStep]);

  const activePool = useMemo(
    () => pools.find((p) => p.id === poolId) ?? pools[0],
    [pools, poolId]
  );
  const selectedAgent = agentBindings.find((b) => b.agentId === agentId);

  const availableModels = useMemo(() => {
    if (!activePool) return [];
    return activePool.modelPolicies.map((mp) => ({
      ...mp,
      serving: computePoolServingAccounts(activePool, mp.modelId, nowMs),
    }));
  }, [activePool, nowMs]);

  const effectiveModelId = useMemo(() => {
    if (availableModels.some((m) => m.modelId === modelId)) return modelId;
    return (
      availableModels.find((m) => m.enabled)?.modelId ??
      availableModels[0]?.modelId ??
      ""
    );
  }, [availableModels, modelId]);

  const handlePoolChange = (nextPoolId: string) => {
    setPoolId(nextPoolId);
    const nextPool = pools.find((p) => p.id === nextPoolId);
    if (nextPool) {
      const nextDefault =
        nextPool.modelPolicies.find((mp) => mp.enabled)?.modelId ??
        nextPool.modelPolicies[0]?.modelId ??
        "";
      setModelId(nextDefault);
    }
  };

  const executeSimulation = (overrideFailureMode?: RunPoolSimulationInput["failureMode"]) => {
    if (!activePool || !effectiveModelId) return;
    const modeToRun = overrideFailureMode ?? failureMode;
    const res = runPoolSimulation({
      agentId,
      poolId: activePool.id,
      modelId: effectiveModelId,
      prompt,
      failureMode: modeToRun,
      sessionKey: `sim-session-${sessionSeq}`,
    });
    setResultTrace(res.trace);
    setRunningStep(1);
    setTurnCount((c) => c + 1);
  };

  const pipelineSteps = [
    "识别来源 Agent 与入口协议",
    `锁定请求模型 ${effectiveModelId || "—"}`,
    "T1→T2→T3 + 同工具水位选号",
    failureMode === "429_first"
      ? "429 冷却 → 同模型无感 Failover"
      : failureMode === "401_refresh"
        ? "401 单飞刷新 Token 回写加密库"
        : "直连或会话亲和命中 Cache",
    "IR 双向转换返回给目标 Agent",
  ];

  return (
    <Modal open={open} onClose={onClose} labelledBy={titleId} maxWidthClass="max-w-2xl">
      <div className="flex max-h-[88vh] flex-col">
        <div className="flex items-start justify-between gap-3 border-b border-zinc-200 px-5 py-4 dark:border-zinc-800">
          <div>
            <h2 id={titleId} className="text-base font-semibold text-zinc-900 dark:text-zinc-100">
              网关路由连通性测试
            </h2>
            <p className="mt-0.5 text-xs text-zinc-500 dark:text-zinc-400">
              请求穿过网关 ({gateway.host}:{gateway.port}) 时严格锁定选定模型，支持同会话连续测试冷却跳过与亲和缓存
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="关闭路由连通性测试弹窗"
            className="btn btn-icon btn-ghost"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>

        <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <div>
              <span className="mb-1.5 block text-xs font-medium text-zinc-700 dark:text-zinc-300">
                1. 测试请求来源 Agent
              </span>
              <div className="grid grid-cols-2 gap-1.5">
                {agentBindings.map((b) => {
                  const active = agentId === b.agentId;
                  return (
                    <button
                      key={b.agentId}
                      type="button"
                      onClick={() => setAgentId(b.agentId)}
                      className={`flex items-center gap-2 rounded-xl border px-2.5 py-2 text-left transition ${
                        active
                          ? "border-brand-500 bg-brand-50/70 ring-1 ring-brand-500/20 dark:border-brand-400/60 dark:bg-brand-500/10"
                          : "border-zinc-200 bg-white hover:bg-zinc-50 dark:border-zinc-800 dark:bg-zinc-900/60"
                      }`}
                    >
                      <AgentBrandIcon agentId={b.agentId} size="xs" />
                      <span className="truncate text-xs font-medium text-zinc-900 dark:text-zinc-100">
                        {b.name}
                      </span>
                    </button>
                  );
                })}
              </div>
            </div>

            <div className="space-y-2.5">
              <div>
                <label
                  htmlFor="sim-pool-select"
                  className="mb-1 block text-xs font-medium text-zinc-700 dark:text-zinc-300"
                >
                  2. 目标调度号池
                </label>
                <select
                  id="sim-pool-select"
                  value={poolId}
                  onChange={(e) => handlePoolChange(e.target.value)}
                  className="w-full rounded-lg border border-zinc-200 bg-white px-3 py-2 text-xs text-zinc-900 focus:border-brand-500 focus:outline-none dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-100"
                >
                  {pools.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}（{POOL_STRATEGY_META[p.strategy].shortLabel}）
                    </option>
                  ))}
                </select>
              </div>

              {selectedAgent && (
                <div className="flex items-center justify-between rounded-lg bg-zinc-100/80 px-2.5 py-1.5 text-[11px] dark:bg-zinc-800/70">
                  <span className="text-zinc-500 dark:text-zinc-400">入口线协议</span>
                  <span className={`badge ${WIRE_PROTOCOL_META[selectedAgent.protocol].badgeClass}`}>
                    {WIRE_PROTOCOL_META[selectedAgent.protocol].label}
                  </span>
                </div>
              )}
            </div>
          </div>

          {/* 3. 首要选择项：目标模型 */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between gap-2">
              <span className="flex items-center gap-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-300">
                <Cpu className="h-3.5 w-3.5 text-brand-600 dark:text-brand-400" aria-hidden="true" />
                3. 指定请求模型（网关严守所选模型，不私自换模）
              </span>
              <span className="font-mono text-[11px] text-zinc-400">
                session #{sessionSeq} · 已发 {turnCount} 轮
              </span>
            </div>
            <div className="flex flex-wrap gap-1.5">
              {availableModels.length === 0 ? (
                <div className="w-full rounded-xl border border-dashed border-zinc-300/80 bg-zinc-50/70 px-3 py-2.5 text-xs text-zinc-500 dark:border-zinc-700 dark:bg-zinc-900/50 dark:text-zinc-400">
                  {pools.length === 0
                    ? "当前暂无可用号池，请先在「Agent 接入与调度」中勾选至少一个订阅工具模型后再测试路由。"
                    : "该号池当前未开放任何模型，请先勾选开放模型或切换其他号池。"}
                </div>
              ) : (
                availableModels.map((item) => {
                  const active = effectiveModelId === item.modelId;
                  return (
                    <button
                      key={item.modelId}
                      type="button"
                      onClick={() => setModelId(item.modelId)}
                      aria-pressed={active}
                      className={`flex items-center gap-1.5 rounded-xl border px-2.5 py-1.5 text-left font-mono text-[11px] transition ${
                        active
                          ? "border-brand-500 bg-brand-500/12 font-semibold text-brand-900 dark:border-brand-400/60 dark:text-brand-100"
                          : "border-zinc-200 bg-white text-zinc-700 hover:bg-zinc-50 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-300"
                      }`}
                    >
                      <span>{item.modelId}</span>
                      <span className="rounded bg-zinc-100 px-1 py-0.5 font-sans text-[10px] text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400">
                        {item.serving.healthy}/{item.serving.total}号 ({item.serving.tierSummary})
                      </span>
                    </button>
                  );
                })
              )}
            </div>
          </div>

          {/* Prompt & 故障注入 */}
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <label
                htmlFor="sim-prompt-input"
                className="text-xs font-medium text-zinc-700 dark:text-zinc-300"
              >
                4. 请求摘要与故障注入演练
              </label>
              <div className="flex gap-1">
                {PRESET_PROMPTS.map((p, i) => (
                  <button
                    key={i}
                    type="button"
                    onClick={() => setPrompt(p)}
                    className="chip border border-zinc-200 dark:border-zinc-700"
                  >
                    示例 {i + 1}
                  </button>
                ))}
              </div>
            </div>
            <input
              id="sim-prompt-input"
              type="text"
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              className="w-full rounded-lg border border-zinc-200 bg-zinc-50/70 px-3 py-2 text-xs text-zinc-900 focus:border-brand-500 focus:bg-white focus:outline-none dark:border-zinc-700 dark:bg-zinc-800/70 dark:text-zinc-100"
            />

            <div className="grid grid-cols-1 gap-2 pt-1 sm:grid-cols-3">
              <button
                type="button"
                onClick={() => setFailureMode("none")}
                className={`flex items-center gap-2 rounded-xl border px-3 py-2 text-left text-xs transition ${
                  failureMode === "none"
                    ? "border-emerald-500 bg-emerald-500/10 text-emerald-800 dark:text-emerald-200"
                    : "border-zinc-200 bg-white text-zinc-700 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-300"
                }`}
              >
                <Sparkles className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                <div>
                  <div className="font-semibold">正常路由命中</div>
                  <div className="text-[11px] opacity-80">按梯队/水位/亲和直连</div>
                </div>
              </button>

              <button
                type="button"
                onClick={() => setFailureMode("429_first")}
                className={`flex items-center gap-2 rounded-xl border px-3 py-2 text-left text-xs transition ${
                  failureMode === "429_first"
                    ? "border-amber-500 bg-amber-500/10 text-amber-900 dark:text-amber-200"
                    : "border-zinc-200 bg-white text-zinc-700 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-300"
                }`}
              >
                <Flame className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                <div>
                  <div className="font-semibold">注入首号 429 限流</div>
                  <div className="text-[11px] opacity-80">同模型自动切下一账号</div>
                </div>
              </button>

              <button
                type="button"
                onClick={() => setFailureMode("401_refresh")}
                className={`flex items-center gap-2 rounded-xl border px-3 py-2 text-left text-xs transition ${
                  failureMode === "401_refresh"
                    ? "border-sky-500 bg-sky-500/10 text-sky-900 dark:text-sky-200"
                    : "border-zinc-200 bg-white text-zinc-700 dark:border-zinc-800 dark:bg-zinc-900/60 dark:text-zinc-300"
                }`}
              >
                <RefreshCw className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                <div>
                  <div className="font-semibold">注入 401 自动刷新</div>
                  <div className="text-[11px] opacity-80">单飞刷 Token 原地重试</div>
                </div>
              </button>
            </div>
          </div>

          {/* 实时网关链路结果 */}
          {resultTrace && (
            <section className="space-y-3 rounded-2xl border border-zinc-200 bg-zinc-50/90 p-3.5 dark:border-zinc-800 dark:bg-zinc-950/70">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="text-xs font-semibold text-zinc-900 dark:text-zinc-100">
                  本次测试链路回放 · {resultTrace.id}
                </span>
                <div className="flex items-center gap-1.5">
                  {resultTrace.cacheAffinityHit && (
                    <span className="badge border-sky-500/25 bg-sky-500/10 text-sky-700 dark:text-sky-300">
                      Prompt Cache 亲和命中
                    </span>
                  )}
                  <span
                    className={`badge ${
                      resultTrace.status === "success"
                        ? "border-emerald-500/30 bg-emerald-500/12 text-emerald-700 dark:text-emerald-300"
                        : resultTrace.status === "failover_success"
                          ? "border-amber-500/30 bg-amber-500/12 text-amber-700 dark:text-amber-300"
                          : "border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300"
                    }`}
                  >
                    {resultTrace.status === "success"
                      ? "直连成功"
                      : resultTrace.status === "failover_success"
                        ? "无感切换接管成功"
                        : "严格锁模调度失败"}
                  </span>
                </div>
              </div>

              <div className="grid grid-cols-1 gap-1.5 sm:grid-cols-5">
                {pipelineSteps.map((label, idx) => {
                  const done = runningStep >= idx + 1;
                  return (
                    <div
                      key={label}
                      className={`rounded-lg border px-2 py-1.5 text-[11px] transition ${
                        done
                          ? "border-brand-500/40 bg-brand-500/10 text-brand-900 dark:text-brand-200"
                          : "border-zinc-200 bg-white text-zinc-400 dark:border-zinc-800 dark:bg-zinc-900"
                      }`}
                    >
                      <div className="font-mono text-[10px] opacity-75">STEP {idx + 1}</div>
                      <div className="mt-0.5 leading-snug font-medium">{label}</div>
                    </div>
                  );
                })}
              </div>

              <div className="space-y-1.5">
                {resultTrace.attempts.map((att, idx) => (
                  <div
                    key={idx}
                    className="flex items-start gap-2.5 rounded-xl border border-zinc-200/80 bg-white px-3 py-2 dark:border-zinc-800 dark:bg-zinc-900"
                  >
                    <UpstreamMemberIcon toolId={att.toolId} size="xs" className="mt-0.5" />
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-1.5 text-xs">
                        <span className="font-semibold text-zinc-900 dark:text-zinc-100">
                          {att.accountLabel}
                        </span>
                        {att.tier && (
                          <span className={`badge ${MEMBER_TIER_META[att.tier].badgeClass}`}>
                            {MEMBER_TIER_META[att.tier].shortLabel}
                          </span>
                        )}
                        <span className="font-mono text-[11px] text-zinc-500">{att.model}</span>
                        <span className="font-mono text-[11px] tabular-nums text-zinc-400">
                          {att.latencyMs}ms
                        </span>
                        {att.outcome === "ok" ? (
                          <CheckCircle2 className="h-3.5 w-3.5 text-emerald-500" aria-hidden="true" />
                        ) : (
                          <ShieldAlert className="h-3.5 w-3.5 text-amber-500" aria-hidden="true" />
                        )}
                      </div>
                      <p className="mt-0.5 text-[11px] text-zinc-600 dark:text-zinc-400">
                        {att.detail}
                      </p>
                    </div>
                  </div>
                ))}
              </div>
            </section>
          )}
        </div>

        <div className="flex flex-wrap items-center justify-between gap-2 border-t border-zinc-200 bg-zinc-50/70 px-5 py-3 dark:border-zinc-800 dark:bg-zinc-900/70">
          <div className="flex items-center gap-1.5">
            {resultTrace && onOpenRoutingView && (
              <button
                type="button"
                onClick={() => {
                  onClose();
                  onOpenRoutingView();
                }}
                className="btn btn-sm btn-ghost gap-1 text-brand-700 dark:text-brand-300"
              >
                去路由观测页查看完整流水
                <ArrowRight className="h-3.5 w-3.5" aria-hidden="true" />
              </button>
            )}
            {resultTrace && (
              <button
                type="button"
                onClick={() => setSessionSeq((s) => s + 1)}
                className="btn btn-xs btn-ghost text-zinc-500"
                title="重开全新对话上下文（清除当前测试会话亲和记忆）"
              >
                <RotateCcw className="h-3 w-3" aria-hidden="true" />
                新会话
              </button>
            )}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button type="button" onClick={onClose} className="btn btn-md btn-secondary">
              关闭
            </button>
            {resultTrace && (
              <button
                type="button"
                onClick={() => {
                  setFailureMode("none");
                  executeSimulation("none");
                }}
                disabled={!activePool || !effectiveModelId}
                className="btn btn-md btn-brand-soft gap-1.5"
              >
                <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
                继续发下一条（验证冷却跳过与亲和）
              </button>
            )}
            <button
              type="button"
              onClick={() => executeSimulation()}
              disabled={!activePool || !effectiveModelId}
              className="btn btn-md btn-primary gap-1.5"
            >
              <Play className="h-3.5 w-3.5" aria-hidden="true" />
              发送测试请求
            </button>
          </div>
        </div>
      </div>
    </Modal>
  );
}
