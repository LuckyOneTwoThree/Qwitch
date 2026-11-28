import React from "react";
import type { CompatibilityReport } from "../../types/plugin";

export const CompatibilityBadge = React.memo(function CompatibilityBadge({
  report,
  compact = true,
}: {
  report?: CompatibilityReport;
  compact?: boolean;
}) {
  if (!report) return null;

  const badgeStyle =
    report.level === "compatible"
      ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
      : report.level === "degraded"
        ? "border-amber-500/30 bg-amber-500/12 text-amber-800 dark:text-amber-300"
        : "border-rose-500/30 bg-rose-500/12 text-rose-700 dark:text-rose-300";

  const dotStyle =
    report.level === "compatible"
      ? "bg-emerald-500"
      : report.level === "degraded"
        ? "bg-amber-500"
        : "bg-rose-500";

  const label =
    report.level === "compatible"
      ? "兼容"
      : report.level === "degraded"
        ? "降级可用"
        : "不兼容";

  const issueLines = report.issues
    .filter(
      (i) =>
        i.severity !== "auto_shaped" ||
        i.code.startsWith("OPTIONAL_") ||
        i.code === "REASONING_SIGNATURE_STRIPPED"
    )
    .map((i) => `• ${i.title}: ${i.detail}`);
  if (report.autoShapers.length > 0) {
    issueLines.push(`自动适配: ${report.autoShapers.join(", ")}`);
  }
  const tooltip = issueLines.length > 0 ? issueLines.join("\n") : "协议与能力完全兼容";

  return (
    <span
      title={tooltip}
      className={`inline-flex items-center gap-1 rounded-full border font-medium whitespace-nowrap ${
        compact ? "px-1.5 py-0.5 text-[10px]" : "px-2 py-0.5 text-[11px]"
      } ${badgeStyle}`}
    >
      <span className={`h-1.5 w-1.5 rounded-full ${dotStyle}`} aria-hidden="true" />
      {label}
    </span>
  );
});
