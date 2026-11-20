import React, { useEffect, useId, useState } from "react";
import { Layers } from "lucide-react";
import { getToolBrandMeta as resolveToolBrandMeta } from "../domain/toolRegistry";
import type { AdapterManifest } from "../types/qwitch";

export { getToolBrandMeta, TOOL_BRAND_META, type ToolBrandMeta } from "../domain/toolRegistry";

const SIZE_CLASSES: Record<"xs" | "sm" | "md" | "lg", string> = {
  xs: "h-4 w-4 rounded-[4px]",
  sm: "h-6 w-6 rounded-md",
  md: "h-9 w-9 rounded-xl",
  lg: "h-11 w-11 rounded-2xl",
};

export const ToolBrandIcon = React.memo(function ToolBrandIcon({
  toolId,
  adapters,
  size = "md",
  className = "",
  decorative = true,
}: {
  toolId: string;
  adapters?: AdapterManifest[];
  size?: "xs" | "sm" | "md" | "lg";
  className?: string;
  decorative?: boolean;
}) {
  const meta = resolveToolBrandMeta(toolId, adapters);
  const sizeClass = SIZE_CLASSES[size];
  const [iconFailed, setIconFailed] = useState(false);
  const iconSrc = meta.icon;

  useEffect(() => {
    setIconFailed(false);
  }, [iconSrc]);

  if (!iconSrc || iconFailed) {
    return (
      <div
        aria-hidden={decorative || undefined}
        className={`inline-flex shrink-0 items-center justify-center bg-zinc-100 text-zinc-600 ring-1 ring-black/8 dark:bg-zinc-800 dark:text-zinc-300 dark:ring-white/10 ${sizeClass} ${className}`}
      >
        <Layers
          className={size === "xs" ? "h-2.5 w-2.5" : size === "sm" ? "h-3.5 w-3.5" : "h-4 w-4"}
          aria-hidden="true"
        />
      </div>
    );
  }

  return (
    <div
      aria-hidden={decorative || undefined}
      className={`inline-flex shrink-0 items-center justify-center overflow-hidden bg-white shadow-2xs ring-1 ring-black/8 dark:bg-zinc-900 dark:ring-white/12 ${sizeClass} ${className}`}
    >
      <img
        src={iconSrc}
        alt={decorative ? "" : meta.name}
        className="h-full w-full object-cover select-none"
        draggable={false}
        onError={() => setIconFailed(true)}
      />
    </div>
  );
});

export function QwitchLogo({ size = "md" }: { size?: "sm" | "md" }) {
  // React useId 默认包含冒号（:r0:），作为 SVG url(#id) 引用时部分 WebKit 版本会解析失败
  const uid = useId().replace(/:/g, "");
  const bgId = `${uid}-qwBg`;
  const glowId = `${uid}-qwGlow`;
  const silverId = `${uid}-qwSilver`;
  const emeraldId = `${uid}-qwEmerald`;
  const tailId = `${uid}-qwTail`;
  const box = size === "sm" ? "h-5 w-5 rounded-[6px]" : "h-8 w-8 rounded-xl";
  return (
    <div
      className={`inline-flex shrink-0 items-center justify-center overflow-hidden bg-[#0B0D11] shadow-xs ring-1 ring-black/10 dark:ring-white/15 ${box}`}
      aria-hidden="true"
    >
      <svg viewBox="0 0 1024 1024" className="h-full w-full">
        <defs>
          <radialGradient id={bgId} cx="52%" cy="46%" r="65%">
            <stop offset="0%" stopColor="#1E242D" />
            <stop offset="60%" stopColor="#12161C" />
            <stop offset="100%" stopColor="#0B0D11" />
          </radialGradient>
          <radialGradient id={glowId} cx="62%" cy="62%" r="42%">
            <stop offset="0%" stopColor="#10B981" stopOpacity="0.30" />
            <stop offset="55%" stopColor="#0D9488" stopOpacity="0.10" />
            <stop offset="100%" stopColor="#0B0D11" stopOpacity="0" />
          </radialGradient>
          <linearGradient id={silverId} x1="15%" y1="15%" x2="85%" y2="85%">
            <stop offset="0%" stopColor="#F8FAFC" />
            <stop offset="42%" stopColor="#CBD5E1" />
            <stop offset="80%" stopColor="#64748B" />
            <stop offset="100%" stopColor="#334155" />
          </linearGradient>
          <linearGradient id={emeraldId} x1="20%" y1="10%" x2="90%" y2="90%">
            <stop offset="0%" stopColor="#6EE7B7" />
            <stop offset="45%" stopColor="#10B981" />
            <stop offset="100%" stopColor="#0F766E" />
          </linearGradient>
          <linearGradient id={tailId} x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#5EEAD4" />
            <stop offset="50%" stopColor="#10B981" />
            <stop offset="100%" stopColor="#047857" />
          </linearGradient>
        </defs>
        <rect width="1024" height="1024" fill={`url(#${bgId})`} />
        <rect width="1024" height="1024" fill={`url(#${glowId})`} />
        <path
          d="M 512 232 C 680 232, 796 352, 796 508 C 796 616, 730 706, 628 744 C 578 762, 518 766, 464 756 C 556 744, 666 662, 676 524 C 686 382, 604 274, 512 232 Z"
          fill={`url(#${emeraldId})`}
        />
        <path
          d="M 512 232 C 340 232, 218 358, 218 510 C 218 664, 340 778, 502 778 C 554 778, 604 762, 644 738 C 526 758, 348 670, 348 500 C 348 354, 454 262, 580 266 C 558 244, 536 232, 512 232 Z"
          fill={`url(#${silverId})`}
        />
        <path
          d="M 548 536 L 748 736 C 756 744, 756 756, 748 764 L 694 818 C 686 826, 674 826, 666 818 L 466 618 C 458 610, 458 598, 466 590 L 520 536 C 528 528, 540 528, 548 536 Z"
          fill={`url(#${tailId})`}
        />
      </svg>
    </div>
  );
}
