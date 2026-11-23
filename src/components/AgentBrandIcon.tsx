import React from "react";
import { KeyRound, TerminalSquare } from "lucide-react";
import type { TargetAgentId } from "../types/pool";
import { ToolBrandIcon } from "./ToolBrandIcon";

const SIZE_CLASSES: Record<"xs" | "sm" | "md" | "lg", string> = {
  xs: "h-4 w-4 rounded-[6px]",
  sm: "h-6 w-6 rounded-lg",
  md: "h-9 w-9 rounded-xl",
  lg: "h-11 w-11 rounded-2xl",
};

const SVG_SIZES: Record<"xs" | "sm" | "md" | "lg", string> = {
  xs: "h-2.5 w-2.5",
  sm: "h-3.5 w-3.5",
  md: "h-5 w-5",
  lg: "h-6 w-6",
};

export type AgentIconKind = TargetAgentId | "custom_endpoint";

const AGENT_SURFACE_STYLE: Record<AgentIconKind, string> = {
  codex_desktop:
    "bg-zinc-900 text-white ring-black/10 dark:bg-zinc-100 dark:text-zinc-950 dark:ring-white/20",
  claude_desktop:
    "bg-[#D97757]/12 text-[#C85A32] ring-[#D97757]/25 dark:bg-[#D97757]/18 dark:text-[#F29C7D] dark:ring-[#D97757]/30",
  dsh: "bg-sky-500/12 text-sky-600 ring-sky-500/25 dark:bg-sky-400/15 dark:text-sky-300 dark:ring-sky-400/25",
  opencode:
    "bg-emerald-500/12 text-emerald-700 ring-emerald-500/25 dark:bg-emerald-400/15 dark:text-emerald-300 dark:ring-emerald-400/25",
  custom_endpoint:
    "bg-zinc-100 text-zinc-700 ring-black/8 dark:bg-zinc-800 dark:text-zinc-300 dark:ring-white/12",
};

export const AgentBrandIcon = React.memo(function AgentBrandIcon({
  agentId,
  size = "md",
  className = "",
  decorative = true,
}: {
  agentId: AgentIconKind;
  size?: "xs" | "sm" | "md" | "lg";
  className?: string;
  decorative?: boolean;
}) {
  const boxClass = SIZE_CLASSES[size];
  const svgClass = SVG_SIZES[size];
  const surfaceClass = AGENT_SURFACE_STYLE[agentId];

  const renderMark = () => {
    switch (agentId) {
      case "codex_desktop":
        return (
          <svg viewBox="0 0 24 24" className={svgClass} fill="none" stroke="currentColor" strokeWidth="1.9">
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M12 2.75a4.2 4.2 0 0 0-3.64 2.1A4.2 4.2 0 0 0 4.5 9.1a4.2 4.2 0 0 0 .9 4.3A4.2 4.2 0 0 0 7.7 19a4.2 4.2 0 0 0 4.3 2.25A4.2 4.2 0 0 0 16.3 19a4.2 4.2 0 0 0 2.3-5.6 4.2 4.2 0 0 0 .9-4.3 4.2 4.2 0 0 0-3.86-4.25A4.2 4.2 0 0 0 12 2.75Z"
            />
            <polygon
              points="12,8.2 15.3,10.1 15.3,13.9 12,15.8 8.7,13.9 8.7,10.1"
              strokeLinejoin="round"
            />
          </svg>
        );
      case "claude_desktop":
        return (
          <svg viewBox="0 0 24 24" className={svgClass} fill="none" stroke="currentColor" strokeWidth="2.1" strokeLinecap="round">
            <path d="M12 3.2v17.6M3.2 12h17.6M5.8 5.8l12.4 12.4M18.2 5.8 5.8 18.2" />
            <circle cx="12" cy="12" r="2.2" fill="currentColor" stroke="none" />
          </svg>
        );
      case "dsh":
        return (
          <svg viewBox="0 0 24 24" className={svgClass} fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round">
            <path d="M4.5 14.5C4.5 9.5 8.2 6 13.4 6c3.8 0 6.1 2.2 6.1 5.3 0 3.7-3 6.7-7.4 6.7-2.2 0-4.2-.8-5.5-2.1" />
            <path d="M8.5 12.2c1.2-2.2 3.3-3.2 5.6-2.9 1.7.2 2.8 1.3 2.7 2.8-.1 1.7-1.6 3-3.7 3-1.4 0-2.6-.5-3.4-1.4" />
            <circle cx="15.8" cy="9.7" r="0.9" fill="currentColor" stroke="none" />
          </svg>
        );
      case "opencode":
        return (
          <svg viewBox="0 0 24 24" className={svgClass} fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <rect x="4" y="4.5" width="16" height="15" rx="3.2" />
            <path d="M8.5 10l2.5 2-2.5 2M12.8 14h2.7" />
          </svg>
        );
      case "custom_endpoint":
        return <TerminalSquare className={svgClass} aria-hidden="true" />;
    }
  };

  return (
    <div
      aria-hidden={decorative || undefined}
      className={`inline-flex shrink-0 items-center justify-center ring-1 shadow-2xs ${boxClass} ${surfaceClass} ${className}`}
    >
      {renderMark()}
    </div>
  );
});

export const UpstreamMemberIcon = React.memo(function UpstreamMemberIcon({
  toolId,
  size = "sm",
  className = "",
}: {
  toolId: string;
  size?: "xs" | "sm" | "md" | "lg";
  className?: string;
}) {
  if (toolId === "custom_api_key") {
    const boxClass = SIZE_CLASSES[size];
    const iconSize =
      size === "xs"
        ? "h-2.5 w-2.5"
        : size === "sm"
          ? "h-3.5 w-3.5"
          : size === "md"
            ? "h-4 w-4"
            : "h-5 w-5";
    return (
      <div
        aria-hidden="true"
        className={`inline-flex shrink-0 items-center justify-center bg-zinc-100 text-zinc-700 ring-1 ring-black/8 dark:bg-zinc-800 dark:text-zinc-200 dark:ring-white/12 ${boxClass} ${className}`}
      >
        <KeyRound className={iconSize} aria-hidden="true" />
      </div>
    );
  }
  return <ToolBrandIcon toolId={toolId} size={size} className={className} decorative />;
});
