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

export type AgentIconKind = TargetAgentId | "custom_endpoint";

const AGENT_ICON_SRC: Partial<Record<AgentIconKind, string>> = {
  codex_desktop: "/icons/agent-codex.png",
  claude_desktop: "/icons/agent-claude.png",
  dsh: "/icons/agent-dsh.png",
  opencode: "/icons/agent-opencode.png",
  mimo: "/icons/agent-mimo.png",
  zcode: "/icons/agent-zcode.png",
};

const AGENT_SURFACE_STYLE: Record<AgentIconKind, string> = {
  codex_desktop: "bg-transparent ring-0 shadow-none",
  claude_desktop: "bg-transparent ring-0 shadow-none",
  dsh: "bg-transparent ring-0 shadow-none",
  opencode: "bg-transparent ring-0 shadow-none",
  mimo: "bg-transparent ring-0 shadow-none",
  zcode: "bg-transparent ring-0 shadow-none",
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
  const surfaceClass = AGENT_SURFACE_STYLE[agentId];

  const renderMark = () => {
    const src = AGENT_ICON_SRC[agentId];
    if (src) {
      return <img src={src} alt="" className="h-full w-full object-contain" draggable={false} />;
    }
    return <TerminalSquare className="h-1/2 w-1/2" aria-hidden="true" />;
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
