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
    "bg-zinc-900 text-white ring-black/15 dark:bg-zinc-100 dark:text-zinc-950 dark:ring-white/25",
  claude_desktop:
    "bg-[#D97757] text-white ring-[#C45E3D]/30 dark:bg-[#D97757] dark:text-white dark:ring-[#F29C7D]/30",
  dsh: "bg-[#4D6BFE] text-white ring-[#3A56E8]/30 dark:bg-[#4D6BFE] dark:text-white dark:ring-[#7B93FF]/30",
  opencode:
    "bg-zinc-950 text-emerald-400 ring-emerald-500/30 dark:bg-zinc-900 dark:text-emerald-300 dark:ring-emerald-400/30",
  mimo: "bg-transparent ring-black/10 dark:ring-white/10",
  zcode: "bg-transparent ring-black/10 dark:ring-white/10",
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
          <svg viewBox="0 0 24 24" className={svgClass} fill="currentColor">
            <path d="M22.282 9.821a5.985 5.985 0 0 0-.516-4.91 6.046 6.046 0 0 0-6.51-2.9A6.065 6.065 0 0 0 4.981 4.18a5.985 5.985 0 0 0-3.998 2.9 6.046 6.046 0 0 0 .743 7.097 5.98 5.98 0 0 0 .51 4.911 6.051 6.051 0 0 0 6.515 2.9A5.985 5.985 0 0 0 13.26 24a6.056 6.056 0 0 0 5.772-4.206 5.99 5.99 0 0 0 3.997-2.9 6.056 6.056 0 0 0-.747-7.073zM13.26 22.43a4.476 4.476 0 0 1-2.876-1.04l.141-.081 4.779-2.758a.795.795 0 0 0 .392-.681v-6.737l2.02 1.168a.071.071 0 0 1 .038.052v5.583a4.504 4.504 0 0 1-4.494 4.494zM3.6 18.304a4.47 4.47 0 0 1-.535-3.014l.142.085 4.783 2.759a.771.771 0 0 0 .78 0l5.843-3.369v2.332a.08.08 0 0 1-.033.062L9.74 19.95a4.5 4.5 0 0 1-6.14-1.646zM2.34 7.896a4.485 4.485 0 0 1 2.366-1.973V11.6a.766.766 0 0 0 .388.676l5.815 3.355-2.02 1.168a.076.076 0 0 1-.071 0l-4.83-2.786A4.504 4.504 0 0 1 2.34 7.872zm16.597 3.855l-5.833-3.387L15.119 7.2a.076.076 0 0 1 .071 0l4.83 2.791a4.494 4.494 0 0 1-.676 8.105v-5.678a.79.79 0 0 0-.407-.667zm2.01-3.023l-.141-.085-4.774-2.782a.776.776 0 0 0-.785 0L9.409 9.23V6.897a.066.066 0 0 1 .028-.061l4.83-2.787a4.5 4.5 0 0 1 6.68 4.66zm-12.64 4.135l-2.02-1.164a.08.08 0 0 1-.038-.057V6.075a4.5 4.5 0 0 1 7.375-3.453l-.142.08L8.704 5.46a.795.795 0 0 0-.393.681zm1.097-2.365l2.602-1.5 2.607 1.5v2.999l-2.597 1.5-2.607-1.5Z" />
          </svg>
        );
      case "claude_desktop":
        return (
          <svg viewBox="0 0 24 24" className={svgClass} fill="currentColor">
            <path d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z" />
          </svg>
        );
      case "dsh":
        return (
          <svg viewBox="0 0 24 24" className={svgClass} fill="currentColor">
            <path d="M21.2 12.1c-.2-3-2-5.4-4.7-6.3-2.5-.8-5.4-.3-7.4 1.3C7.7 8.2 6.8 9.7 6.3 11.1c-1-.1-2-.7-2.6-1.6-.2-.3-.6-.2-.7.1-.5 1.3-.3 2.8.5 3.9-.8.4-1.6.4-2.2.2-.3-.1-.5.2-.4.5.7 1.3 2.1 2.1 3.7 2.1h.3c1.6 2 4.3 3.2 7.5 3.2 4.9 0 8.8-3.2 8.8-7.4zm-5.4-1.7a1.1 1.1 0 1 1 0 2.2 1.1 1.1 0 0 1 0-2.2z" />
          </svg>
        );
      case "opencode":
        return (
          <svg viewBox="0 0 24 24" className={svgClass} fill="currentColor">
            <path
              fillRule="evenodd"
              clipRule="evenodd"
              d="M4 4.5A1.5 1.5 0 0 1 5.5 3h13A1.5 1.5 0 0 1 20 4.5v15a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 19.5v-15Zm3.5 2a.5.5 0 0 0-.5.5v10a.5.5 0 0 0 .5.5h9a.5.5 0 0 0 .5-.5V7a.5.5 0 0 0-.5-.5h-9Zm2.5 3.5h4v4h-4v-4Z"
            />
          </svg>
        );
      case "mimo":
        return <img src="/icons/mimo.png" alt="" className="h-full w-full object-contain" />;
      case "zcode":
        return <img src="/icons/zcode.png" alt="" className="h-full w-full object-contain" />;
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
