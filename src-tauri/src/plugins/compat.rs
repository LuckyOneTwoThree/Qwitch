use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::domain::WireProtocolKind;
use crate::plugins::manifest::{EgressProfile, IngressProfile, ModelIdPolicy, SystemPromptPolicy};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityLevel {
    Compatible,
    Degraded,
    Incompatible,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    Passthrough,
    IncrementalTranslate,
    BufferedTranslate,
    Unsupported,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Unverified,
    Verified,
    ProbeFailed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    Blocker,
    Warning,
    AutoShaped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityIssue {
    pub severity: IssueSeverity,
    pub code: String,
    pub title: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityReport {
    pub level: CompatibilityLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_ingress_protocol: Option<WireProtocolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_egress_protocol: Option<WireProtocolKind>,
    pub transport_mode: TransportMode,
    pub auto_shapers: Vec<String>,
    pub issues: Vec<CompatibilityIssue>,
    pub probe_status: ProbeStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProbeCapabilities {
    #[serde(default)]
    pub streaming: Option<bool>,
    #[serde(default)]
    pub tools: Option<bool>,
    #[serde(default)]
    pub system_prompt_ok: Option<bool>,
    #[serde(default)]
    pub vision: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
pub struct BridgeSupport {
    pub mode: TransportMode,
    pub cross_protocol_tools_buffered: bool,
    pub strips_reasoning_signature: bool,
}

pub struct BridgeMatrix {
    routes: HashMap<(WireProtocolKind, WireProtocolKind), BridgeSupport>,
}

fn proto_key(p: WireProtocolKind) -> u8 {
    match p {
        WireProtocolKind::OpenaiChat => 0,
        WireProtocolKind::AnthropicMessages => 1,
        WireProtocolKind::OpenaiResponses => 2,
    }
}

/// 包装 HashMap<(u8, u8)> 以便无需为 WireProtocolKind 强加 Hash trait。
impl BridgeMatrix {
    pub fn empty() -> Self {
        Self {
            routes: HashMap::new(),
        }
    }

    pub fn insert(&mut self, from: WireProtocolKind, to: WireProtocolKind, support: BridgeSupport) {
        self.routes.insert((from, to), support);
    }

    pub fn remove(&mut self, from: WireProtocolKind, to: WireProtocolKind) {
        self.routes.remove(&(from, to));
    }

    pub fn get(&self, from: WireProtocolKind, to: WireProtocolKind) -> Option<&BridgeSupport> {
        let _ = proto_key(from);
        self.routes.get(&(from, to))
    }

    /// 内核补齐三协议全双向 IR 后的默认桥接矩阵。
    pub fn default_kernel() -> Self {
        let mut m = Self::empty();
        let all = [
            WireProtocolKind::OpenaiChat,
            WireProtocolKind::AnthropicMessages,
            WireProtocolKind::OpenaiResponses,
        ];
        for from in all {
            for to in all {
                if from == to {
                    m.insert(
                        from,
                        to,
                        BridgeSupport {
                            mode: TransportMode::Passthrough,
                            cross_protocol_tools_buffered: false,
                            strips_reasoning_signature: false,
                        },
                    );
                } else {
                    let tools_buffered = matches!(from, WireProtocolKind::OpenaiResponses)
                        || matches!(to, WireProtocolKind::OpenaiResponses);
                    m.insert(
                        from,
                        to,
                        BridgeSupport {
                            mode: TransportMode::IncrementalTranslate,
                            cross_protocol_tools_buffered: tools_buffered,
                            strips_reasoning_signature: true,
                        },
                    );
                }
            }
        }
        m
    }
}

pub struct CompatibilityEngine;

impl CompatibilityEngine {
    pub fn evaluate(
        ingress: &IngressProfile,
        egress: &EgressProfile,
        probe: Option<&ProbeCapabilities>,
        matrix: &BridgeMatrix,
    ) -> CompatibilityReport {
        let mut issues = Vec::new();
        let mut auto_shapers = Vec::new();

        // Step 0: 运行时探针覆写静态声明（仅在实测为 false 时降权，实测为 true 保留状态）
        let mut effective_provides = egress.provides.clone();
        let probe_status = match probe {
            None => ProbeStatus::Unverified,
            Some(p) => {
                let mut any_failed = false;
                if p.streaming == Some(false) {
                    effective_provides.streaming = false;
                    any_failed = true;
                }
                if p.tools == Some(false) {
                    effective_provides.tools = false;
                    any_failed = true;
                }
                if p.vision == Some(false) {
                    effective_provides.vision = false;
                }
                if p.system_prompt_ok == Some(false) {
                    any_failed = true;
                }
                if any_failed {
                    ProbeStatus::ProbeFailed
                } else {
                    ProbeStatus::Verified
                }
            }
        };

        // Step 1: 协议桥接寻路
        if !ingress.quirks.supports_custom_base_url {
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::Blocker,
                code: "CLIENT_NO_CUSTOM_ENDPOINT".into(),
                title: "入口客户端不支持自定义网关地址".into(),
                detail: "该入口插件声明无法覆盖 Base URL，无法将请求路由到 Qwitch 本地网关。"
                    .into(),
                suggestion: Some(
                    "请检查入口客户端是否有第三方 API 配置开关，或改用支持的 Agent。".into(),
                ),
            });
            return CompatibilityReport {
                level: CompatibilityLevel::Incompatible,
                selected_ingress_protocol: None,
                selected_egress_protocol: None,
                transport_mode: TransportMode::Unsupported,
                auto_shapers,
                issues,
                probe_status,
            };
        }

        let mut best_route: Option<(WireProtocolKind, WireProtocolKind, &BridgeSupport)> = None;
        let rank = |mode: TransportMode| -> u8 {
            match mode {
                TransportMode::Passthrough => 3,
                TransportMode::IncrementalTranslate => 2,
                TransportMode::BufferedTranslate => 1,
                TransportMode::Unsupported => 0,
            }
        };
        for &in_p in &ingress.wire_protocols {
            for &out_p in &egress.wire_protocols {
                if let Some(sup) = matrix.get(in_p, out_p) {
                    if sup.mode == TransportMode::Unsupported {
                        continue;
                    }
                    let replace = best_route
                        .as_ref()
                        .is_none_or(|(_, _, cur)| rank(sup.mode) > rank(cur.mode));
                    if replace {
                        best_route = Some((in_p, out_p, sup));
                    }
                }
            }
        }

        let Some((in_proto, out_proto, bridge)) = best_route else {
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::Blocker,
                code: "NO_PROTOCOL_BRIDGE".into(),
                title: "缺少协议桥接路径".into(),
                detail: format!(
                    "入口协议 {:?} 与出口协议 {:?} 之间没有可用的内核 IR 转换路径。",
                    ingress.wire_protocols, egress.wire_protocols
                ),
                suggestion: Some("请升级 Qwitch 内核或更换支持兼容协议的出口/入口插件。".into()),
            });
            return CompatibilityReport {
                level: CompatibilityLevel::Incompatible,
                selected_ingress_protocol: None,
                selected_egress_protocol: None,
                transport_mode: TransportMode::Unsupported,
                auto_shapers,
                issues,
                probe_status,
            };
        };

        let mut transport_mode = bridge.mode;

        // Step 2: 硬性能力求交
        if ingress.requires.tools && !effective_provides.tools {
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::Blocker,
                code: "MISSING_TOOLS".into(),
                title: "出口不支持 Function Calling (tools)".into(),
                detail: "该入口 Agent 依赖工具调用驱动编码工作流，但目标出口未提供 tools 能力。"
                    .into(),
                suggestion: Some("请更换支持 tools 的订阅出口或 API Key 兜底。".into()),
            });
        }
        if ingress.requires.vision && !effective_provides.vision {
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::Blocker,
                code: "MISSING_VISION".into(),
                title: "出口不支持多模态图像输入".into(),
                detail: "该入口强制要求图像理解能力，但目标出口未提供 vision 能力。".into(),
                suggestion: Some("请选择支持视觉输入的模型或出口。".into()),
            });
        }
        if ingress.requires.thinking && !effective_provides.thinking {
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::Blocker,
                code: "MISSING_THINKING".into(),
                title: "出口不支持推理思考输出".into(),
                detail: "该入口强制要求思考链输出，但目标出口不支持 thinking。".into(),
                suggestion: None,
            });
        }
        if ingress.requires.streaming && !effective_provides.streaming {
            if egress.constraints.must_stream {
                issues.push(CompatibilityIssue {
                    severity: IssueSeverity::Blocker,
                    code: "CONTRADICTORY_STREAMING".into(),
                    title: "出口流式能力声明冲突".into(),
                    detail: "出口声明必须流式但流式能力不可用。".into(),
                    suggestion: None,
                });
            } else {
                issues.push(CompatibilityIssue {
                    severity: IssueSeverity::Warning,
                    code: "SYNTHESIZED_STREAMING".into(),
                    title: "由内核合成 SSE 流式响应".into(),
                    detail: "目标出口仅支持非流式响应，网关将自动等待完整响应后合成为 SSE 流。"
                        .into(),
                    suggestion: None,
                });
            }
        }

        // Step 3: 软降级与自动整形
        if in_proto != out_proto && ingress.requires.tools && bridge.cross_protocol_tools_buffered {
            transport_mode = TransportMode::BufferedTranslate;
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::Warning,
                code: "BUFFERED_TOOL_STREAM".into(),
                title: "跨协议工具调用采用缓冲模式".into(),
                detail: "由于入口与出口协议不同且携带 tools，网关将缓冲上游完整响应后再转换输出，首字延迟（TTFT）会略有增加。".into(),
                suggestion: None,
            });
        }
        if in_proto != out_proto
            && bridge.strips_reasoning_signature
            && (ingress.requires.thinking || ingress.optional.thinking)
        {
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::AutoShaped,
                code: "REASONING_SIGNATURE_STRIPPED".into(),
                title: "思考签名无法跨协议透传".into(),
                detail: "跨协议调度时，加密思考签名将被安全剥离并降级为纯文本思考。".into(),
                suggestion: None,
            });
        }
        if ingress.optional.vision && !effective_provides.vision {
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::AutoShaped,
                code: "OPTIONAL_VISION_UNSUPPORTED".into(),
                title: "不支持图片输入（可选增强项）".into(),
                detail: "目标出口暂未开放多模态视觉能力，纯文本与工具调用不受影响。".into(),
                suggestion: None,
            });
        }
        if ingress.optional.thinking && !effective_provides.thinking {
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::AutoShaped,
                code: "OPTIONAL_THINKING_UNSUPPORTED".into(),
                title: "不支持思考链输出（可选增强项）".into(),
                detail: "目标出口不返回独立思考过程，仅输出正文与工具调用。".into(),
                suggestion: None,
            });
        }
        if ingress.optional.compact && !effective_provides.compact {
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::AutoShaped,
                code: "OPTIONAL_COMPACT_UNSUPPORTED".into(),
                title: "不支持上下文远程压缩端点（可选增强项）".into(),
                detail: "目标出口不提供 /responses/compact，长上下文压缩将依赖客户端本地策略。"
                    .into(),
                suggestion: None,
            });
        }
        if probe.is_some_and(|p| p.system_prompt_ok == Some(false)) {
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::Warning,
                code: "SYSTEM_PROMPT_BLOCKED_BY_UPSTREAM".into(),
                title: "探针检测到系统提示词易触发上游风控".into(),
                detail: "实测显示上游对该 Agent 特征的系统提示词存在拦截风险，网关将尝试自动洗脱。"
                    .into(),
                suggestion: None,
            });
        }

        // 记录内核自动应用的整形器
        let mut push_shaper = |key: &str, title: &str, detail: &str| {
            auto_shapers.push(key.to_string());
            issues.push(CompatibilityIssue {
                severity: IssueSeverity::AutoShaped,
                code: format!("AUTO_SHAPE_{}", key.to_ascii_uppercase()),
                title: title.to_string(),
                detail: detail.to_string(),
                suggestion: None,
            });
        };

        if egress.constraints.must_stream {
            push_shaper(
                "must_stream",
                "自动开启上游流式请求",
                "出口要求强制流式，若入口请求非流式，内核将自动开流并聚合返回。",
            );
        }
        if egress.constraints.requires_instructions
            && out_proto == WireProtocolKind::OpenaiResponses
        {
            push_shaper(
                "requires_instructions",
                "自动映射并补齐 instructions",
                "内核自动将 system/developer 提示词映射为 Responses instructions 字段。",
            );
        }
        if egress.constraints.disallow_store && out_proto == WireProtocolKind::OpenaiResponses {
            push_shaper(
                "disallow_store",
                "自动注入 store: false",
                "已按出口约束自动禁用服务端会话存储。",
            );
        }
        if egress.constraints.strip_temperature_on_reasoning {
            push_shaper(
                "strip_temperature",
                "自动剥离不兼容的 temperature 参数",
                "推理类上游不支持采样温度参数，内核将在出站时自动剔除。",
            );
        }
        if egress.constraints.system_prompt_policy == SystemPromptPolicy::SanitizeAgentOpeners {
            push_shaper(
                "sanitize_agent_openers",
                "自动洗脱敏感 Agent 开场白",
                "出站前自动改写易触发通道拦截的 Agent 自述前缀。",
            );
        }
        if ingress.quirks.model_id_policy == ModelIdPolicy::AnthropicWhitelistAlias {
            push_shaper(
                "anthropic_whitelist_alias",
                "自动启用 Claude 客户端模型名伪装",
                "非 Anthropic 模型名将自动映射为过白名单的别名并保留真实展示名。",
            );
        }

        let has_blocker = issues.iter().any(|i| i.severity == IssueSeverity::Blocker);
        let has_warning = issues.iter().any(|i| i.severity == IssueSeverity::Warning);
        let level = if has_blocker {
            CompatibilityLevel::Incompatible
        } else if has_warning {
            CompatibilityLevel::Degraded
        } else {
            CompatibilityLevel::Compatible
        };

        CompatibilityReport {
            level,
            selected_ingress_protocol: Some(in_proto),
            selected_egress_protocol: Some(out_proto),
            transport_mode,
            auto_shapers,
            issues,
            probe_status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::manifest::{
        EgressConstraints, EgressProvides, IngressOptional, IngressQuirks, IngressRequires,
    };

    fn qoder_ingress() -> IngressProfile {
        IngressProfile {
            wire_protocols: vec![WireProtocolKind::OpenaiChat],
            requires: IngressRequires {
                streaming: true,
                tools: true,
                vision: false,
                thinking: false,
            },
            optional: IngressOptional {
                vision: true,
                thinking: true,
                tool_choice: true,
                count_tokens: false,
                compact: false,
            },
            quirks: IngressQuirks::default(),
            declarative_wiring: None,
        }
    }

    fn codex_sub_egress() -> EgressProfile {
        EgressProfile {
            wire_protocols: vec![WireProtocolKind::OpenaiResponses],
            provides: EgressProvides {
                streaming: true,
                tools: true,
                tool_choice: true,
                vision: true,
                thinking: true,
                encrypted_reasoning: true,
                json_schema: true,
                count_tokens: false,
                compact: true,
            },
            constraints: EgressConstraints {
                must_stream: true,
                requires_instructions: true,
                disallow_store: true,
                strip_temperature_on_reasoning: true,
                system_prompt_policy: SystemPromptPolicy::Standard,
            },
            ..Default::default()
        }
    }

    #[test]
    fn codex_egress_supports_qoder_ingress_with_auto_shapers() {
        let report = CompatibilityEngine::evaluate(
            &qoder_ingress(),
            &codex_sub_egress(),
            None,
            &BridgeMatrix::default_kernel(),
        );
        assert_eq!(report.level, CompatibilityLevel::Degraded);
        assert_eq!(report.transport_mode, TransportMode::BufferedTranslate);
        assert_eq!(
            report.selected_ingress_protocol,
            Some(WireProtocolKind::OpenaiChat)
        );
        assert_eq!(
            report.selected_egress_protocol,
            Some(WireProtocolKind::OpenaiResponses)
        );
        assert!(report.auto_shapers.contains(&"must_stream".to_string()));
        assert!(report
            .auto_shapers
            .contains(&"requires_instructions".to_string()));
        assert!(report.auto_shapers.contains(&"disallow_store".to_string()));
        assert!(!report
            .issues
            .iter()
            .any(|i| i.severity == IssueSeverity::Blocker));
    }

    #[test]
    fn reports_no_protocol_bridge_when_matrix_lacks_route() {
        let mut matrix = BridgeMatrix::default_kernel();
        matrix.remove(
            WireProtocolKind::OpenaiChat,
            WireProtocolKind::OpenaiResponses,
        );
        let report =
            CompatibilityEngine::evaluate(&qoder_ingress(), &codex_sub_egress(), None, &matrix);
        assert_eq!(report.level, CompatibilityLevel::Incompatible);
        assert!(report.issues.iter().any(|i| i.code == "NO_PROTOCOL_BRIDGE"));
    }

    #[test]
    fn blocks_when_egress_lacks_tools_required_by_coding_agent() {
        let mut web_egress = codex_sub_egress();
        web_egress.provides.tools = false;
        let report = CompatibilityEngine::evaluate(
            &qoder_ingress(),
            &web_egress,
            None,
            &BridgeMatrix::default_kernel(),
        );
        assert_eq!(report.level, CompatibilityLevel::Incompatible);
        assert!(report.issues.iter().any(|i| i.code == "MISSING_TOOLS"));
    }

    #[test]
    fn blocks_when_client_cannot_override_base_url() {
        let mut ingress = qoder_ingress();
        ingress.quirks.supports_custom_base_url = false;
        let report = CompatibilityEngine::evaluate(
            &ingress,
            &codex_sub_egress(),
            None,
            &BridgeMatrix::default_kernel(),
        );
        assert_eq!(report.level, CompatibilityLevel::Incompatible);
        assert!(report
            .issues
            .iter()
            .any(|i| i.code == "CLIENT_NO_CUSTOM_ENDPOINT"));
    }

    #[test]
    fn probe_override_downgrades_false_static_declaration() {
        let probe = ProbeCapabilities {
            streaming: Some(true),
            tools: Some(false),
            system_prompt_ok: Some(true),
            vision: Some(true),
        };
        let report = CompatibilityEngine::evaluate(
            &qoder_ingress(),
            &codex_sub_egress(),
            Some(&probe),
            &BridgeMatrix::default_kernel(),
        );
        assert_eq!(report.level, CompatibilityLevel::Incompatible);
        assert_eq!(report.probe_status, ProbeStatus::ProbeFailed);
        assert!(report.issues.iter().any(|i| i.code == "MISSING_TOOLS"));
    }

    #[test]
    fn claude_desktop_same_protocol_is_compatible_with_alias_shaper() {
        let claude = IngressProfile {
            wire_protocols: vec![WireProtocolKind::AnthropicMessages],
            requires: IngressRequires {
                streaming: true,
                tools: true,
                vision: false,
                thinking: false,
            },
            optional: IngressOptional::default(),
            quirks: IngressQuirks {
                supports_custom_base_url: true,
                model_id_policy: ModelIdPolicy::AnthropicWhitelistAlias,
                ..Default::default()
            },
            declarative_wiring: None,
        };
        let anthropic_egress = EgressProfile {
            wire_protocols: vec![WireProtocolKind::AnthropicMessages],
            provides: standard_egress(),
            ..Default::default()
        };
        let report = CompatibilityEngine::evaluate(
            &claude,
            &anthropic_egress,
            None,
            &BridgeMatrix::default_kernel(),
        );
        assert_eq!(report.level, CompatibilityLevel::Compatible);
        assert_eq!(report.transport_mode, TransportMode::Passthrough);
        assert!(report
            .auto_shapers
            .contains(&"anthropic_whitelist_alias".to_string()));
    }

    #[test]
    fn same_protocol_coding_agent_is_compatible_despite_optional_vision_missing() {
        let mut egress = codex_sub_egress();
        egress.wire_protocols = vec![WireProtocolKind::OpenaiChat];
        egress.provides.vision = false;
        egress.constraints = EgressConstraints::default();

        let report = CompatibilityEngine::evaluate(
            &qoder_ingress(),
            &egress,
            None,
            &BridgeMatrix::default_kernel(),
        );
        assert_eq!(report.level, CompatibilityLevel::Compatible);
        assert_eq!(report.transport_mode, TransportMode::Passthrough);
        assert!(report
            .issues
            .iter()
            .any(|i| i.code == "OPTIONAL_VISION_UNSUPPORTED"));
    }

    #[test]
    fn opencode_to_minimax_is_compatible_with_incremental_translate() {
        let mut minimax = codex_sub_egress();
        minimax.wire_protocols = vec![WireProtocolKind::AnthropicMessages];
        minimax.provides.vision = false;
        minimax.provides.encrypted_reasoning = false;
        minimax.constraints = EgressConstraints::default();

        let report = CompatibilityEngine::evaluate(
            &qoder_ingress(),
            &minimax,
            None,
            &BridgeMatrix::default_kernel(),
        );
        assert_eq!(report.level, CompatibilityLevel::Compatible);
        assert_eq!(report.transport_mode, TransportMode::IncrementalTranslate);
        assert!(report
            .issues
            .iter()
            .any(|i| i.code == "REASONING_SIGNATURE_STRIPPED"
                && i.severity == IssueSeverity::AutoShaped));
    }

    fn standard_egress() -> EgressProvides {
        EgressProvides {
            streaming: true,
            tools: true,
            tool_choice: true,
            vision: true,
            thinking: true,
            encrypted_reasoning: true,
            json_schema: true,
            count_tokens: true,
            compact: false,
        }
    }
}
