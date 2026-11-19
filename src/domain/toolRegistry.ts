import type { AdapterManifest, ToolId } from "../types/qwitch";

export interface ToolBrandMeta {
  id: string;
  name: string;
  shortTagline: string;
  tagline: string;
  icon?: string;
  accentDot: string;
  badgeClass: string;
  softBgClass: string;
}

export const TOOL_BRAND_META: Record<string, ToolBrandMeta> = {
  trae: {
    id: "trae",
    name: "Trae",
    shortTagline: "AI 原生 IDE",
    tagline: "AI 原生编程 IDE",
    icon: "/icons/trae-cn.svg",
    accentDot: "bg-emerald-500",
    badgeClass:
      "border-emerald-500/20 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
    softBgClass: "bg-emerald-500/8",
  },
  trae_cn: {
    id: "trae_cn",
    name: "Trae CN",
    shortTagline: "字节 AI IDE",
    tagline: "字节 AI 编程 IDE（国内版）",
    icon: "/icons/trae-cn.svg",
    accentDot: "bg-emerald-500",
    badgeClass:
      "border-emerald-500/20 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
    softBgClass: "bg-emerald-500/8",
  },
  workbuddy: {
    id: "workbuddy",
    name: "WorkBuddy",
    shortTagline: "腾讯 AI 助手",
    tagline: "腾讯 AI 桌面协作助手",
    icon: "/icons/workbuddy.svg",
    accentDot: "bg-teal-500",
    badgeClass:
      "border-teal-500/20 bg-teal-500/10 text-teal-700 dark:text-teal-300",
    softBgClass: "bg-teal-500/8",
  },
  qoder: {
    id: "qoder",
    name: "Qoder",
    shortTagline: "Agent 编程",
    tagline: "智能 Agent 编程平台",
    icon: "/icons/qoder-cn.svg",
    accentDot: "bg-amber-500",
    badgeClass:
      "border-amber-500/20 bg-amber-500/10 text-amber-700 dark:text-amber-300",
    softBgClass: "bg-amber-500/8",
  },
  qoder_cn: {
    id: "qoder_cn",
    name: "Qoder CN",
    shortTagline: "Agent 编程",
    tagline: "智能 Agent 编程工作台",
    icon: "/icons/qoder-cn.svg",
    accentDot: "bg-amber-500",
    badgeClass:
      "border-amber-500/20 bg-amber-500/10 text-amber-700 dark:text-amber-300",
    softBgClass: "bg-amber-500/8",
  },
  minimax_code_cn: {
    id: "minimax_code_cn",
    name: "MiniMax Code CN",
    shortTagline: "MiniMax 桌面端",
    tagline: "MiniMax Code 桌面版（国内版）",
    icon: "/icons/minimax-code.png",
    accentDot: "bg-sky-500",
    badgeClass:
      "border-sky-500/20 bg-sky-500/10 text-sky-700 dark:text-sky-300",
    softBgClass: "bg-sky-500/8",
  },
};

export const BUILTIN_TOOL_ORDER: ToolId[] = [
  "trae_cn",
  "workbuddy",
  "qoder_cn",
  "minimax_code_cn",
];

export function getToolBrandMeta(
  toolId: string,
  adapters?: AdapterManifest[]
): ToolBrandMeta {
  const builtin = TOOL_BRAND_META[toolId];
  if (builtin) return builtin;

  const adapter = adapters?.find((a) => a.id === toolId);
  return {
    id: toolId,
    name: adapter?.name ?? toolId,
    shortTagline: adapter?.isBuiltin ? "内置工具" : "外部插件",
    tagline: adapter?.isBuiltin ? "内置 AI 工具适配器" : "外部 JSON-RPC 扩展适配器",
    accentDot: "bg-zinc-500",
    badgeClass:
      "border-zinc-500/20 bg-zinc-500/10 text-zinc-700 dark:text-zinc-300",
    softBgClass: "bg-zinc-500/8",
  };
}

export interface VisibleToolEntry {
  id: ToolId;
  name: string;
  shortTagline: string;
  tagline: string;
  isBuiltin: boolean;
}

export function getVisibleTools(adapters: AdapterManifest[]): VisibleToolEntry[] {
  const byId = new Map<string, AdapterManifest>();
  for (const ad of adapters) {
    byId.set(ad.id, ad);
  }

  const orderedIds: string[] = [];
  for (const id of BUILTIN_TOOL_ORDER) {
    if (byId.has(id) || adapters.length === 0) {
      orderedIds.push(id);
    }
  }
  for (const ad of adapters) {
    if (!orderedIds.includes(ad.id)) {
      orderedIds.push(ad.id);
    }
  }

  return orderedIds.map((id) => {
    const meta = getToolBrandMeta(id, adapters);
    const ad = byId.get(id);
    return {
      id,
      name: ad?.name ?? meta.name,
      shortTagline: meta.shortTagline,
      tagline: meta.tagline,
      isBuiltin: ad?.isBuiltin ?? true,
    };
  });
}
