import type { WireProtocolKind } from "./pool";

export type PluginKind = "egress" | "ingress" | "hybrid";
export type PluginTransport = "builtin" | "declarative" | "subprocess";
export type PluginSource = "builtin" | "external";

export interface PluginItemDto {
  id: string;
  name: string;
  version: string;
  kind: PluginKind;
  transport: PluginTransport;
  source: PluginSource;
  enabled: boolean;
  pinnedHash?: string;
  overridesBuiltin: boolean;
  shortTagline: string;
  wireProtocols: WireProtocolKind[];
  providesTools?: boolean;
  requiresTools?: boolean;
}

export type CompatibilityLevel = "compatible" | "degraded" | "incompatible";
export type TransportMode =
  | "passthrough"
  | "incremental_translate"
  | "buffered_translate"
  | "unsupported";
export type ProbeStatus = "unverified" | "verified" | "probe_failed";
export type IssueSeverity = "blocker" | "warning" | "auto_shaped";

export interface CompatibilityIssue {
  severity: IssueSeverity;
  code: string;
  title: string;
  detail: string;
  suggestion?: string;
}

export interface CompatibilityReport {
  level: CompatibilityLevel;
  selectedIngressProtocol?: WireProtocolKind;
  selectedEgressProtocol?: WireProtocolKind;
  transportMode: TransportMode;
  autoShapers: string[];
  issues: CompatibilityIssue[];
  probeStatus: ProbeStatus;
}

export interface StoredCapabilityProbe {
  egressPluginId: string;
  modelId: string;
  probeJson: string;
  verifiedAt: string;
  errorSummary?: string;
}
