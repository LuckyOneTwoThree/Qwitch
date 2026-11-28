use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::domain::{
    AccountPool, AgentBinding, AgentBindingOptions, TargetAgentId, WireProtocolKind,
};
use crate::storage::QwitchDatabase;

#[derive(Debug, Clone)]
pub struct IngressDetectInfo {
    pub primary_display: String,
    pub files: Vec<PathBuf>,
    pub installed: bool,
    pub version: String,
}

#[allow(clippy::too_many_arguments)]
pub trait IngressPlugin: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn subtitle(&self) -> &str;
    fn protocol(&self) -> WireProtocolKind;
    fn default_options(&self) -> AgentBindingOptions;

    fn build_snippets(
        &self,
        mounted_pool_ids: &[String],
        default_model_key: Option<&str>,
        all_pools: &[AccountPool],
        host: &str,
        port: u16,
        options: &AgentBindingOptions,
        is_drifted: bool,
    ) -> (String, String);

    fn is_running_in_snapshot(&self, snapshot: &str) -> bool;
    fn pids_in_snapshot(&self, snapshot: &str) -> Vec<u32>;

    fn apply_wiring(
        &self,
        db: &QwitchDatabase,
        master_key: &[u8; 32],
        mounted_pool_ids: &[String],
        default_model_key: Option<&str>,
        options: &AgentBindingOptions,
        all_pools: &[AccountPool],
        host: &str,
        port: u16,
        global_proxy_detected: bool,
        base_dir_override: Option<&Path>,
    ) -> Result<AgentBinding, String>;

    fn clear_wiring(
        &self,
        db: &QwitchDatabase,
        master_key: &[u8; 32],
        mode: &str,
        all_pools: &[AccountPool],
        host: &str,
        port: u16,
        global_proxy_detected: bool,
        base_dir_override: Option<&Path>,
    ) -> Result<AgentBinding, String>;

    fn unbind_and_restore(
        &self,
        db: &QwitchDatabase,
        master_key: &[u8; 32],
        all_pools: &[AccountPool],
        host: &str,
        port: u16,
        global_proxy_detected: bool,
        base_dir_override: Option<&Path>,
    ) -> Result<AgentBinding, String>;

    fn restart_client(&self, options: &AgentBindingOptions) -> Result<String, String>;
}

pub struct BuiltinIngressPlugin {
    agent: TargetAgentId,
}

impl BuiltinIngressPlugin {
    pub fn new(agent: TargetAgentId) -> Self {
        Self { agent }
    }
}

impl IngressPlugin for BuiltinIngressPlugin {
    fn id(&self) -> &str {
        self.agent.as_str()
    }

    fn display_name(&self) -> &str {
        match self.agent {
            TargetAgentId::ClaudeDesktop => "Claude 桌面版",
            TargetAgentId::CodexDesktop => "Codex 桌面版",
            TargetAgentId::Dsh => "DeepSeek Harness",
            TargetAgentId::Opencode => "OpenCode",
            TargetAgentId::Mimo => "MiMo Code",
            TargetAgentId::Zcode => "ZCode",
        }
    }

    fn subtitle(&self) -> &str {
        match self.agent {
            TargetAgentId::ClaudeDesktop => "Anthropic Messages 3p 网关",
            TargetAgentId::CodexDesktop => "OpenAI Responses API",
            TargetAgentId::Dsh => "OpenAI Chat Completions",
            TargetAgentId::Opencode => "OpenAI Chat Completions",
            TargetAgentId::Mimo => "OpenAI Chat Completions",
            TargetAgentId::Zcode => "Anthropic Messages API",
        }
    }

    fn protocol(&self) -> WireProtocolKind {
        match self.agent {
            TargetAgentId::CodexDesktop => WireProtocolKind::OpenaiResponses,
            TargetAgentId::ClaudeDesktop | TargetAgentId::Zcode => {
                WireProtocolKind::AnthropicMessages
            }
            TargetAgentId::Dsh | TargetAgentId::Opencode | TargetAgentId::Mimo => {
                WireProtocolKind::OpenaiChat
            }
        }
    }

    fn default_options(&self) -> AgentBindingOptions {
        match self.agent {
            TargetAgentId::ClaudeDesktop => AgentBindingOptions {
                claude_isolated_twin: Some(false),
                ..Default::default()
            },
            TargetAgentId::CodexDesktop => AgentBindingOptions {
                codex_preserve_table: Some(true),
                ..Default::default()
            },
            TargetAgentId::Dsh => AgentBindingOptions {
                dsh_dual_creds_written: Some(false),
                ..Default::default()
            },
            TargetAgentId::Opencode => AgentBindingOptions {
                opencode_schema_version: Some("v2".into()),
                ..Default::default()
            },
            TargetAgentId::Mimo => AgentBindingOptions {
                opencode_schema_version: Some("v1".into()),
                ..Default::default()
            },
            TargetAgentId::Zcode => AgentBindingOptions::default(),
        }
    }

    fn build_snippets(
        &self,
        mounted_pool_ids: &[String],
        default_model_key: Option<&str>,
        all_pools: &[AccountPool],
        host: &str,
        port: u16,
        options: &AgentBindingOptions,
        is_drifted: bool,
    ) -> (String, String) {
        crate::agent_wiring::build_agent_config_snippets(
            self.agent,
            mounted_pool_ids,
            default_model_key,
            all_pools,
            host,
            port,
            options,
            is_drifted,
        )
    }

    fn is_running_in_snapshot(&self, snapshot: &str) -> bool {
        crate::agent_wiring::agent_running_in_snapshot(self.agent, snapshot)
    }

    fn pids_in_snapshot(&self, snapshot: &str) -> Vec<u32> {
        crate::agent_wiring::agent_pids_in_snapshot(self.agent, snapshot)
    }

    fn apply_wiring(
        &self,
        db: &QwitchDatabase,
        master_key: &[u8; 32],
        mounted_pool_ids: &[String],
        default_model_key: Option<&str>,
        options: &AgentBindingOptions,
        all_pools: &[AccountPool],
        host: &str,
        port: u16,
        global_proxy_detected: bool,
        base_dir_override: Option<&Path>,
    ) -> Result<AgentBinding, String> {
        crate::agent_wiring::apply_agent_wiring(
            db,
            master_key,
            self.agent,
            mounted_pool_ids,
            default_model_key,
            options,
            all_pools,
            host,
            port,
            global_proxy_detected,
            base_dir_override,
        )
    }

    fn clear_wiring(
        &self,
        db: &QwitchDatabase,
        master_key: &[u8; 32],
        mode: &str,
        all_pools: &[AccountPool],
        host: &str,
        port: u16,
        global_proxy_detected: bool,
        base_dir_override: Option<&Path>,
    ) -> Result<AgentBinding, String> {
        crate::agent_wiring::clear_agent_wiring(
            db,
            master_key,
            self.agent,
            all_pools,
            host,
            port,
            global_proxy_detected,
            base_dir_override,
            mode,
        )
    }

    fn unbind_and_restore(
        &self,
        db: &QwitchDatabase,
        master_key: &[u8; 32],
        all_pools: &[AccountPool],
        host: &str,
        port: u16,
        global_proxy_detected: bool,
        base_dir_override: Option<&Path>,
    ) -> Result<AgentBinding, String> {
        crate::agent_wiring::unbind_and_restore_agent(
            db,
            master_key,
            self.agent,
            all_pools,
            host,
            port,
            global_proxy_detected,
            base_dir_override,
        )
    }

    fn restart_client(&self, _options: &AgentBindingOptions) -> Result<String, String> {
        #[cfg(target_os = "macos")]
        {
            match self.agent {
                TargetAgentId::CodexDesktop => Ok("codex_desktop".into()),
                TargetAgentId::ClaudeDesktop => Ok("claude_desktop".into()),
                _ => Ok(self.id().to_string()),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(self.id().to_string())
        }
    }
}

pub fn create_builtin_ingress_plugins() -> HashMap<String, Arc<dyn IngressPlugin>> {
    TargetAgentId::all()
        .into_iter()
        .map(|agent| {
            let plugin: Arc<dyn IngressPlugin> = Arc::new(BuiltinIngressPlugin::new(agent));
            (plugin.id().to_string(), plugin)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_all_six_builtin_ingress_plugins() {
        let map = create_builtin_ingress_plugins();
        for id in [
            "codex_desktop",
            "claude_desktop",
            "dsh",
            "opencode",
            "mimo",
            "zcode",
        ] {
            assert!(map.contains_key(id), "missing builtin ingress plugin {id}");
            assert_eq!(map[id].id(), id);
        }
    }
}
