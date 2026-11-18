import { Layers } from "lucide-react";

export interface ToolBrandMeta {
  id: string;
  name: string;
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
    tagline: "腾讯 AI 桌面协作助手",
    icon: "/icons/workbuddy.svg",
    accentDot: "bg-teal-500",
    badgeClass: "border-teal-500/20 bg-teal-500/10 text-teal-700 dark:text-teal-300",
    softBgClass: "bg-teal-500/8",
  },
  qoder: {
    id: "qoder",
    name: "Qoder",
    tagline: "智能 Agent 编程平台",
    icon: "/icons/qoder-cn.svg",
    accentDot: "bg-amber-500",
    badgeClass: "border-amber-500/20 bg-amber-500/10 text-amber-700 dark:text-amber-300",
    softBgClass: "bg-amber-500/8",
  },
  qoder_cn: {
    id: "qoder_cn",
    name: "Qoder CN",
    tagline: "智能 Agent 编程工作台",
    icon: "/icons/qoder-cn.svg",
    accentDot: "bg-amber-500",
    badgeClass: "border-amber-500/20 bg-amber-500/10 text-amber-700 dark:text-amber-300",
    softBgClass: "bg-amber-500/8",
  },
};

export function getToolBrandMeta(toolId: string): ToolBrandMeta {
  return (
    TOOL_BRAND_META[toolId] ?? {
      id: toolId,
      name: toolId,
      tagline: "AI 工具适配器",
      accentDot: "bg-zinc-400",
      badgeClass: "border-zinc-500/20 bg-zinc-500/10 text-zinc-600 dark:text-zinc-300",
      softBgClass: "bg-zinc-500/8",
    }
  );
}

const SIZE_CLASSES: Record<"xs" | "sm" | "md" | "lg", string> = {
  xs: "h-4 w-4 rounded-[4px]",
  sm: "h-6 w-6 rounded-md",
  md: "h-9 w-9 rounded-xl",
  lg: "h-11 w-11 rounded-2xl",
};

export function ToolBrandIcon({
  toolId,
  size = "md",
  className = "",
}: {
  toolId: string;
  size?: "xs" | "sm" | "md" | "lg";
  className?: string;
}) {
  const meta = getToolBrandMeta(toolId);
  const sizeClass = SIZE_CLASSES[size];

  if (!meta.icon) {
    return (
      <div
        className={`inline-flex shrink-0 items-center justify-center bg-zinc-100 text-zinc-500 ring-1 ring-black/8 dark:bg-zinc-800 dark:text-zinc-300 dark:ring-white/10 ${sizeClass} ${className}`}
      >
        <Layers className={size === "xs" ? "h-2.5 w-2.5" : size === "sm" ? "h-3.5 w-3.5" : "h-4 w-4"} />
      </div>
    );
  }

  return (
    <div
      className={`inline-flex shrink-0 items-center justify-center overflow-hidden bg-white shadow-2xs ring-1 ring-black/8 dark:bg-zinc-900 dark:ring-white/12 ${sizeClass} ${className}`}
    >
      <img
        src={meta.icon}
        alt={meta.name}
        className="h-full w-full object-cover select-none"
        draggable={false}
      />
    </div>
  );
}

export function QwitchLogo({ size = "md" }: { size?: "sm" | "md" }) {
  const box = size === "sm" ? "h-5 w-5 rounded-md" : "h-8 w-8 rounded-xl";
  return (
    <div
      className={`inline-flex shrink-0 items-center justify-center bg-gradient-to-br from-zinc-900 via-zinc-800 to-zinc-700 text-white shadow-xs ring-1 ring-black/10 dark:from-zinc-100 dark:via-zinc-200 dark:to-zinc-300 dark:text-zinc-900 dark:ring-white/15 ${box}`}
    >
      <svg
        viewBox="0 0 24 24"
        fill="none"
        className={size === "sm" ? "h-3.5 w-3.5" : "h-4.5 w-4.5"}
        aria-hidden="true"
      >
        <circle
          cx="11"
          cy="11"
          r="6"
          stroke="currentColor"
          strokeWidth="2.1"
        />
        <path
          d="M14.5 14.5L19 19"
          stroke="currentColor"
          strokeWidth="2.2"
          strokeLinecap="round"
        />
        <path
          d="M8.5 11H13.5M12 9.2L13.8 11L12 12.8"
          stroke="currentColor"
          strokeWidth="1.6"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    </div>
  );
}
