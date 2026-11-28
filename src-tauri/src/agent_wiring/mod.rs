use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::adapter::write_private_file_atomic;
use crate::domain::{
    compute_agent_exposed_models, AccountPool, AgentBinding, AgentBindingOptions,
    AgentBindingStatus, AgentExposedModel, TargetAgentId, WireProtocolKind,
};
use crate::storage::pools::StoredAgentBinding;
use crate::storage::QwitchDatabase;

const CLAUDE_CONFIG_UUID: &str = "00000000-0000-4000-8000-717769746368";

/// 每装机随机网关绑定令牌后缀。由启动期以主密钥派生后注入；未注入时（单元测试/开发）
/// 回退为 `dev`，并额外接受旧的静态常量令牌以保持测试与旧配置可用。
static GATEWAY_TOKEN_SUFFIX: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn init_gateway_token_suffix(suffix: String) {
    let _ = GATEWAY_TOKEN_SUFFIX.set(suffix);
}

fn token_suffix() -> &'static str {
    GATEWAY_TOKEN_SUFFIX
        .get()
        .map(String::as_str)
        .unwrap_or("dev")
}

fn agent_slug(agent_id: TargetAgentId) -> &'static str {
    match agent_id {
        TargetAgentId::ClaudeDesktop => "claude-desktop",
        TargetAgentId::CodexDesktop => "codex-desktop",
        TargetAgentId::Dsh => "dsh",
        TargetAgentId::Opencode => "opencode",
        TargetAgentId::Mimo => "mimo",
        TargetAgentId::Zcode => "zcode",
    }
}

/// 返回该 Agent 的网关绑定令牌。生产环境为 `qwitch-bind-<slug>-<每装机随机后缀>`，
/// 不可由公开信息猜测；开发/测试环境后缀为 `dev`。
pub fn agent_token(agent_id: TargetAgentId) -> String {
    format!("qwitch-bind-{}-{}", agent_slug(agent_id), token_suffix())
}

/// 返回任意入口插件 ID 的网关绑定令牌（内置 ID 与 `agent_token` 完全一致）。
pub fn agent_token_for_id(agent_id: &str) -> String {
    if let Some(builtin) = TargetAgentId::from_id(agent_id) {
        return agent_token(builtin);
    }
    let slug = agent_id.replace('_', "-");
    format!("qwitch-bind-{slug}-{}", token_suffix())
}

/// 在任意候选入口插件 ID 集合中以常量时间比对匹配网关绑定令牌。
pub fn agent_id_str_from_token(token: &str, candidate_ids: &[String]) -> Option<String> {
    if let Some(builtin) = agent_id_from_token(token) {
        return Some(builtin.as_str().to_string());
    }
    let trimmed = token.trim();
    for id in candidate_ids {
        let expected = agent_token_for_id(id);
        if ct_eq(trimmed, &expected) {
            return Some(id.clone());
        }
    }
    None
}

fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

pub fn agent_id_from_token(token: &str) -> Option<TargetAgentId> {
    let token = token.trim();
    for agent in [
        TargetAgentId::ClaudeDesktop,
        TargetAgentId::CodexDesktop,
        TargetAgentId::Dsh,
        TargetAgentId::Opencode,
        TargetAgentId::Mimo,
        TargetAgentId::Zcode,
    ] {
        if ct_eq(token, &agent_token(agent)) {
            return Some(agent);
        }
    }
    // 未注入后缀（单元测试/开发）时兼容旧的静态常量令牌
    if GATEWAY_TOKEN_SUFFIX.get().is_none() {
        return match token {
            "qwitch-bind-claude-desktop" => Some(TargetAgentId::ClaudeDesktop),
            "qwitch-bind-codex-desktop" => Some(TargetAgentId::CodexDesktop),
            "qwitch-bind-dsh" => Some(TargetAgentId::Dsh),
            "qwitch-bind-opencode" => Some(TargetAgentId::Opencode),
            _ => None,
        };
    }
    None
}

pub fn detect_bridge_runtime() -> (String, String) {
    let bun_candidates = [
        "bun".to_string(),
        std::env::var("HOME")
            .map(|h| format!("{h}/.bun/bin/bun"))
            .unwrap_or_default(),
        "/opt/homebrew/bin/bun".to_string(),
        "/usr/local/bin/bun".to_string(),
    ];
    for bin in bun_candidates {
        if bin.is_empty() {
            continue;
        }
        let mut cmd = Command::new(&bin);
        cmd.arg("--version");
        // 探测型子进程：Windows 下不能弹控制台黑框（release 构建无父控制台可继承）
        crate::platform::hide_console_std(&mut cmd);
        if let Ok(out) = cmd.output() {
            if out.status.success() {
                let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return ("bun_ready".into(), format!("Bun {ver}"));
            }
        }
    }

    let node_candidates = [
        "node".to_string(),
        "/opt/homebrew/bin/node".to_string(),
        "/usr/local/bin/node".to_string(),
    ];
    for bin in node_candidates {
        let mut cmd = Command::new(&bin);
        cmd.arg("--version");
        crate::platform::hide_console_std(&mut cmd);
        if let Ok(out) = cmd.output() {
            if out.status.success() {
                let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return ("node_ready".into(), format!("Node {ver} (回退)"));
            }
        }
    }

    ("missing".into(), "未检测到 Bun/Node".into())
}

pub fn detect_global_proxy_active() -> bool {
    let has_env_proxy = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ]
    .iter()
    .any(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()));
    let no_proxy = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();
    let loopback_bypassed = no_proxy.contains("127.0.0.1") && no_proxy.contains("localhost");
    let env_proxy_active = has_env_proxy && !loopback_bypassed;

    #[cfg(target_os = "macos")]
    {
        if env_proxy_active {
            return true;
        }
        if let Ok(out) = Command::new("scutil").arg("--proxy").output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                let http_on = text.contains("HTTPEnable : 1") || text.contains("HTTPSEnable : 1");
                if http_on && !text.contains("127.0.0.1") {
                    return true;
                }
            }
        }
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        env_proxy_active
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

struct AgentPaths {
    primary_display: String,
    files: Vec<PathBuf>,
    installed: bool,
    version: String,
}

fn resolve_agent_paths(
    agent_id: TargetAgentId,
    options: &AgentBindingOptions,
    base_dir_override: Option<&Path>,
) -> AgentPaths {
    let home = home_dir();
    match agent_id {
        TargetAgentId::ClaudeDesktop => {
            let isolated = options.claude_isolated_twin.unwrap_or(false);
            let (base_support, active_dir, official_3p_dir) = if let Some(base) = base_dir_override
            {
                (
                    base.to_path_buf(),
                    base.join(if isolated {
                        "Claude-Qwitch-3p"
                    } else {
                        "Claude-3p"
                    }),
                    base.join("Claude-3p"),
                )
            } else {
                #[cfg(target_os = "macos")]
                {
                    let app_sup = home.join("Library/Application Support");
                    let active = app_sup.join(if isolated {
                        "Claude-Qwitch-3p"
                    } else {
                        "Claude-3p"
                    });
                    (app_sup.clone(), active, app_sup.join("Claude-3p"))
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let app_sup = std::env::var_os("LOCALAPPDATA")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home.join("AppData/Local"));
                    let active = app_sup.join(if isolated {
                        "Claude-Qwitch-3p"
                    } else {
                        "Claude-3p"
                    });
                    (app_sup.clone(), active, app_sup.join("Claude-3p"))
                }
            };
            let main_cfg = if base_dir_override.is_some() {
                base_support.join("Claude/claude_desktop_config.json")
            } else {
                #[cfg(target_os = "macos")]
                {
                    base_support.join("Claude/claude_desktop_config.json")
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let roaming = std::env::var_os("APPDATA")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home.join("AppData/Roaming"));
                    roaming.join("Claude/claude_desktop_config.json")
                }
            };
            let lib_dir = active_dir.join("configLibrary");
            let official_3p_lib = official_3p_dir.join("configLibrary");
            let installed = base_dir_override.is_some()
                || main_cfg.parent().is_some_and(|p| p.exists())
                || active_dir.exists()
                || official_3p_dir.exists();
            let mut files = vec![
                main_cfg,
                lib_dir.join(format!("{CLAUDE_CONFIG_UUID}.json")),
                lib_dir.join("_meta.json"),
                official_3p_dir.join("claude_desktop_config.json"),
            ];
            if official_3p_dir != active_dir {
                files.push(official_3p_lib.join(format!("{CLAUDE_CONFIG_UUID}.json")));
                files.push(official_3p_lib.join("_meta.json"));
                files.push(active_dir.join("claude_desktop_config.json"));
            }
            AgentPaths {
                primary_display: if isolated {
                    "~/Library/Application Support/Claude-Qwitch-3p & Claude-3p/".into()
                } else {
                    "~/Library/Application Support/Claude-3p/configLibrary/".into()
                },
                files,
                installed,
                version: "2.x 3p".into(),
            }
        }
        TargetAgentId::Opencode => {
            let dir = if let Some(base) = base_dir_override {
                base.join("opencode")
            } else if let Ok(custom) = std::env::var("OPENCODE_CONFIG_DIR") {
                PathBuf::from(custom)
            } else {
                home.join(".config/opencode")
            };
            let cfg_file = ["opencode.jsonc", "opencode.json", "config.json"]
                .iter()
                .map(|name| dir.join(name))
                .find(|p| p.exists())
                .unwrap_or_else(|| dir.join("opencode.jsonc"));
            let mut version_cmd = Command::new("opencode");
            version_cmd.arg("--version");
            crate::platform::hide_console_std(&mut version_cmd);
            let version = version_cmd
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "2.x".into());
            let installed = base_dir_override.is_some() || dir.exists();
            AgentPaths {
                primary_display: "~/.config/opencode/opencode.jsonc".into(),
                files: vec![cfg_file],
                installed,
                version,
            }
        }
        TargetAgentId::Dsh => {
            let dsh_home = if let Some(base) = base_dir_override {
                base.join("dsh")
            } else if let Ok(custom) = std::env::var("DSH_HOME") {
                PathBuf::from(custom)
            } else {
                home.join(".dsh")
            };
            let installed = base_dir_override.is_some() || dsh_home.exists();
            AgentPaths {
                primary_display: "~/.dsh/profiles/desktop/cordis.patch.yml".into(),
                files: vec![
                    dsh_home.join("profiles/desktop/cordis.patch.yml"),
                    dsh_home.join(".env"),
                    dsh_home.join(".credentials.yaml"),
                ],
                installed,
                version: "0.2.x".into(),
            }
        }
        TargetAgentId::CodexDesktop => {
            let codex_home = if let Some(base) = base_dir_override {
                base.join("codex")
            } else if let Ok(custom) = std::env::var("CODEX_HOME") {
                PathBuf::from(custom)
            } else {
                home.join(".codex")
            };
            let installed = base_dir_override.is_some() || codex_home.exists();
            AgentPaths {
                primary_display: "~/.codex/config.toml".into(),
                files: vec![
                    codex_home.join("config.toml"),
                    codex_home.join("qwitch_models.json"),
                ],
                installed,
                version: "Responses".into(),
            }
        }
        TargetAgentId::Mimo => {
            let dir = if let Some(base) = base_dir_override {
                base.join("mimocode")
            } else if let Ok(custom) = std::env::var("MIMOCODE_CONFIG_DIR") {
                PathBuf::from(custom)
            } else {
                home.join(".config/mimocode")
            };
            let cfg_file = ["mimocode.jsonc", "mimocode.json", "config.json"]
                .iter()
                .map(|n| dir.join(n))
                .find(|p| p.exists())
                .unwrap_or_else(|| dir.join("mimocode.jsonc"));
            let installed = base_dir_override.is_some() || dir.exists();
            AgentPaths {
                primary_display: "~/.config/mimocode/mimocode.jsonc".into(),
                files: vec![cfg_file],
                installed,
                version: "MiMo Code".into(),
            }
        }
        TargetAgentId::Zcode => {
            let dir = if let Some(base) = base_dir_override {
                base.join("zcode")
            } else {
                home.join(".zcode")
            };
            let installed = base_dir_override.is_some() || dir.exists();
            AgentPaths {
                primary_display: "~/.zcode/v2/config.json".into(),
                files: vec![
                    dir.join("v2/config.json"),
                    dir.join("v2/provider_config.json"),
                ],
                installed,
                version: "ZCode".into(),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_agent_config_snippets(
    agent_id: TargetAgentId,
    mounted_pool_ids: &[String],
    default_model_key: Option<&str>,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    options: &AgentBindingOptions,
    is_drifted: bool,
) -> (String, String) {
    let base_url = format!("http://{host}:{port}");
    let exposed = compute_agent_exposed_models(
        agent_id,
        mounted_pool_ids,
        default_model_key,
        all_pools,
        chrono::Utc::now().timestamp_millis(),
    );
    let default_wire = exposed
        .iter()
        .find(|m| m.is_default)
        .or_else(|| exposed.first())
        .map(|m| m.wire_model_id.clone())
        .unwrap_or_else(|| "coding-main/deepseek-v4-pro".to_string());

    match agent_id {
        TargetAgentId::ClaudeDesktop => {
            let dir = if options.claude_isolated_twin.unwrap_or(false) {
                "~/Library/Application Support/Claude-Qwitch-3p"
            } else {
                "~/Library/Application Support/Claude-3p"
            };
            let model_lines = if exposed.is_empty() {
                "//   （当前未挂载号池）".to_string()
            } else {
                exposed
                    .iter()
                    .map(|m| {
                        format!(
                            "//   {}{} -> display_name: \"{}\" ({})",
                            if m.is_default { "★ " } else { "" },
                            m.wire_model_id,
                            m.display_name,
                            m.tier_summary
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let before = format!(
                "// {dir}/claude_desktop_config.json\n{{\n  \"deploymentMode\": \"cloud\"\n}}"
            );
            let after = format!(
                "// {dir}/claude_desktop_config.json\n{{\n  \"deploymentMode\": \"3p\"\n}}\n\n// {dir}/configLibrary/{CLAUDE_CONFIG_UUID}.json\n{{\n  \"inferenceProvider\": \"gateway\",\n  \"inferenceGatewayBaseUrl\": \"{base_url}\",\n  \"inferenceGatewayApiKey\": \"{}\",\n  \"inferenceGatewayAuthScheme\": \"bearer\",\n  \"disableDeploymentModeChooser\": true,\n  \"coworkEgressAllowedHosts\": [\"*\"],\n  \"toolSearchEnabled\": true\n}}\n// GET /v1/models 暴露清单:\n{model_lines}",
                agent_token(agent_id)
            );
            (before, after)
        }
        TargetAgentId::CodexDesktop => {
            let catalog_lines = if exposed.is_empty() {
                "#   - (未挂载号池)".to_string()
            } else {
                exposed
                    .iter()
                    .map(|m| {
                        format!(
                            "#   - {} ({}, {})",
                            m.wire_model_id, m.display_name, m.tier_summary
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let before =
                "# ~/.codex/config.toml\nmodel = \"gpt-5.4\"\n# 未写入 Qwitch provider".to_string();
            let after = format!(
                "# ~/.codex/config.toml\n# qwitch:managed-start\nmodel_provider = \"qwitch\"\nmodel = \"{default_wire}\"\nmodel_catalog_json = \"~/.codex/qwitch_models.json\"\n# qwitch:managed-end\n\n[model_providers.qwitch]\nname = \"Qwitch Multi-Pool Gateway\"\nbase_url = \"{base_url}/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"{}\"\nhttp_headers = {{ \"x-openai-actor-authorization\" = \"qwitch\" }}\nrequires_openai_auth = true\n# Catalog 模型清单:\n{catalog_lines}",
                agent_token(agent_id)
            );
            (before, after)
        }
        TargetAgentId::Dsh => {
            let before_creds = if is_drifted {
                "# ~/.dsh/.credentials.yaml  [⚠️ 检测缺失 QWITCH_GATEWAY_KEY]\nrefs:\n  DEEPSEEK_API_KEY: \"sk-orig-***\""
            } else {
                "# ~/.dsh/.credentials.yaml\nrefs:\n  DEEPSEEK_API_KEY: \"sk-orig-***\""
            };
            let models_yaml = if exposed.is_empty() {
                format!("          - id: \"{default_wire}\"\n            name: \"Qwitch Default\"\n            contextWindow: 200000\n            maxTokens: 64000\n            input: [text, image]")
            } else {
                exposed
                    .iter()
                    .map(|m| {
                        format!(
                            "          - id: \"{}\"\n            name: \"{}\"\n            contextWindow: 200000\n            maxTokens: 64000\n            input: [text, image]",
                            m.wire_model_id, m.display_name
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let before = format!(
                "# ~/.dsh/profiles/desktop/cordis.patch.yml\n- id: llm-deepseek\n  config:\n    model: deepseek-chat\n\n{before_creds}"
            );
            let after = format!(
                "# ~/.dsh/profiles/desktop/cordis.patch.yml\n- id: llm-pi-ai\n  name: \"@deepseek-ai/dsh-llm-pi-ai\"\n  config:\n    providers:\n      qwitch:\n        displayName: Qwitch\n        apiKeyEnv: QWITCH_GATEWAY_KEY\n        api: openai-completions\n        baseURL: \"{base_url}/v1\"\n        models:\n{models_yaml}\n        streamIdleTimeoutMs: 600000\n\n# ~/.dsh/.env & ~/.dsh/.credentials.yaml\nversion: 1\nrefs:\n  QWITCH_GATEWAY_KEY: \"{}\"",
                agent_token(agent_id)
            );
            (before, after)
        }
        TargetAgentId::Opencode => {
            let schema_v1 = options.opencode_schema_version.as_deref() == Some("v1");
            let before = "// ~/.config/opencode/opencode.jsonc\n{\n  \"model\": \"deepseek/deepseek-chat\"\n}".to_string();
            let after_json = if schema_v1 {
                let mut map = serde_json::Map::new();
                for m in &exposed {
                    map.insert(
                        m.wire_model_id.clone(),
                        json!({
                            "name": m.display_name,
                            "limit": { "context": 200000, "output": 64000 },
                            "modalities": { "input": ["text", "image"], "output": ["text"] },
                            "tool_call": true
                        }),
                    );
                }
                json!({
                    "model": format!("qwitch/{default_wire}"),
                    "provider": {
                        "qwitch": {
                            "npm": "@ai-sdk/openai-compatible",
                            "name": "Qwitch 号池网关",
                            "options": {
                                "baseURL": format!("{base_url}/v1"),
                                "apiKey": agent_token(agent_id)
                            },
                            "models": map
                        }
                    }
                })
            } else {
                let mut map = serde_json::Map::new();
                for m in &exposed {
                    map.insert(
                        m.wire_model_id.clone(),
                        json!({
                            "name": m.display_name,
                            "capabilities": {
                                "tools": true,
                                "input": ["text", "image"],
                                "output": ["text"]
                            },
                            "limit": { "context": 200000, "output": 64000 }
                        }),
                    );
                }
                json!({
                    "model": format!("qwitch/{default_wire}"),
                    "providers": {
                        "qwitch": {
                            "name": "Qwitch 号池网关",
                            "package": "aisdk:@ai-sdk/openai-compatible",
                            "settings": {
                                "baseURL": format!("{base_url}/v1"),
                                "apiKey": agent_token(agent_id)
                            },
                            "models": map
                        }
                    }
                })
            };
            let after = format!(
                "// ~/.config/opencode/opencode.jsonc\n{}",
                serde_json::to_string_pretty(&after_json).unwrap_or_default()
            );
            (before, after)
        }
        TargetAgentId::Mimo => {
            let mut map = serde_json::Map::new();
            for m in &exposed {
                map.insert(
                    m.wire_model_id.clone(),
                    json!({"name": m.display_name, "limit": {"context": 200000, "output": 64000}, "modalities": {"input": ["text", "image"], "output": ["text"]}, "tool_call": true}),
                );
            }
            let after_json = json!({
                "model": format!("qwitch/{default_wire}"),
                "provider": {"qwitch": {"npm": "@ai-sdk/openai-compatible", "name": "Qwitch 号池网关", "options": {"baseURL": format!("{base_url}/v1"), "apiKey": agent_token(agent_id)}, "models": map}}
            });
            (
                "// ~/.config/mimocode/mimocode.jsonc".to_string(),
                format!(
                    "// ~/.config/mimocode/mimocode.jsonc\n{}",
                    serde_json::to_string_pretty(&after_json).unwrap_or_default()
                ),
            )
        }
        TargetAgentId::Zcode => {
            let after_json = json!({
                "provider": {"qwitch": {"name": "Qwitch 号池网关", "kind": "anthropic", "options": {"apiKey": agent_token(agent_id), "baseURL": base_url}, "enabled": true, "source": "custom"}}
            });
            (
                "// ~/.zcode/v2/config.json".to_string(),
                format!(
                    "// ~/.zcode/v2/config.json\n{}",
                    serde_json::to_string_pretty(&after_json).unwrap_or_default()
                ),
            )
        }
    }
}

fn has_port_ref(content: &str, port: u16) -> bool {
    let needle = format!(":{port}");
    let bytes = content.as_bytes();
    let mut start = 0;
    while let Some(pos) = content[start..].find(&needle) {
        let end = start + pos + needle.len();
        let next = bytes.get(end).copied();
        if matches!(
            next,
            None | Some(b'/')
                | Some(b'"')
                | Some(b'\'')
                | Some(b' ')
                | Some(b'\t')
                | Some(b'\r')
                | Some(b'\n')
        ) {
            return true;
        }
        start += pos + 1;
    }
    false
}

fn check_disk_drift(
    agent_id: TargetAgentId,
    mounted_pool_ids: &[String],
    paths: &AgentPaths,
    port: u16,
    options: &AgentBindingOptions,
) -> Option<String> {
    if mounted_pool_ids.is_empty() {
        return None;
    }
    match agent_id {
        TargetAgentId::Dsh => {
            let patch_content = std::fs::read_to_string(&paths.files[0]).unwrap_or_default();
            let patch_ok = patch_content.contains("qwitch:")
                && patch_content.contains("baseURL:")
                && has_port_ref(&patch_content, port);
            let creds_text = std::fs::read_to_string(&paths.files[2]).unwrap_or_default();
            let creds_ok = creds_text.contains("QWITCH_GATEWAY_KEY")
                && !creds_text.contains("refs: {}\n  QWITCH_GATEWAY_KEY")
                && !creds_text.contains("refs:{}\n  QWITCH_GATEWAY_KEY");
            if !creds_ok {
                return Some(
                    "检测到 ~/.dsh/.credentials.yaml 缺失 QWITCH_GATEWAY_KEY 或 YAML 缩进异常（将导致 dsh 桌面端启动失败）"
                        .into(),
                );
            }
            if !patch_ok {
                return Some("检测到 ~/.dsh/profiles/desktop/cordis.patch.yml 未同步最新 Qwitch 网关地址或 Schema".into());
            }
            None
        }
        TargetAgentId::Opencode => {
            let content = std::fs::read_to_string(&paths.files[0]).unwrap_or_default();
            let wants_v1 = options.opencode_schema_version.as_deref() == Some("v1");
            let has_key = if wants_v1 {
                content.contains("\"provider\"")
            } else {
                content.contains("\"providers\"")
            };
            if !has_key || !content.contains("\"qwitch\"") || !has_port_ref(&content, port) {
                return Some(
                    "检测到 opencode 配置与当前 Schema / 端口不一致，建议重新同步写入".into(),
                );
            }
            None
        }
        TargetAgentId::ClaudeDesktop => {
            let main_ok = std::fs::read_to_string(&paths.files[0])
                .map(|c| c.contains("\"3p\""))
                .unwrap_or(false);
            let lib_ok = std::fs::read_to_string(&paths.files[1])
                .map(|c| has_port_ref(&c, port))
                .unwrap_or(false);
            let official_3p_cfg_ok = paths.files.get(3).is_none_or(|p| {
                std::fs::read_to_string(p)
                    .map(|c| c.contains("\"3p\""))
                    .unwrap_or(false)
            });
            let official_3p_lib_ok = paths.files.get(4).is_none_or(|p| {
                std::fs::read_to_string(p)
                    .map(|c| has_port_ref(&c, port))
                    .unwrap_or(false)
            });
            if !main_ok || !lib_ok || !official_3p_cfg_ok || !official_3p_lib_ok {
                return Some(
                    "检测到 Claude Desktop 3p 网关配置被修改、缺少 Claude-3p 镜像或端口未同步"
                        .into(),
                );
            }
            None
        }
        TargetAgentId::CodexDesktop => {
            let toml_ok = std::fs::read_to_string(&paths.files[0])
                .map(|c| {
                    c.contains("[model_providers.qwitch]")
                        && c.contains("model_provider = \"qwitch\"")
                        && c.contains("model_catalog_json")
                        && c.contains("requires_openai_auth = true")
                        && has_port_ref(&c, port)
                })
                .unwrap_or(false);
            let catalog_ok = paths.files[1].is_file();
            if !toml_ok || !catalog_ok {
                return Some(
                    "检测到 ~/.codex/config.toml 或 qwitch_models.json 目录配置已漂移".into(),
                );
            }
            None
        }
        TargetAgentId::Mimo => {
            let content = std::fs::read_to_string(&paths.files[0]).unwrap_or_default();
            if !content.contains("\"qwitch\"") || !has_port_ref(&content, port) {
                return Some("检测到 MiMo Code 配置未同步 Qwitch provider 或端口".into());
            }
            None
        }
        TargetAgentId::Zcode => {
            let cfg = std::fs::read_to_string(&paths.files[0]).unwrap_or_default();
            let rules = paths
                .files
                .get(1)
                .map(|p| std::fs::read_to_string(p).unwrap_or_default())
                .unwrap_or_default();
            let cfg_ok = cfg.contains("qwitch") && has_port_ref(&cfg, port);
            let rules_ok = rules.contains("qwitch") && has_port_ref(&rules, port);
            if !cfg_ok || (!rules.trim().is_empty() && !rules_ok) {
                return Some("检测到 ZCode 配置未同步 Qwitch provider 或端口".into());
            }
            None
        }
    }
}

fn default_options_for_agent(agent_id: TargetAgentId) -> AgentBindingOptions {
    match agent_id {
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

/// 一次快照内判定某 Agent 是否在运行所需的命令行特征（macOS）。
#[cfg(target_os = "macos")]
fn running_patterns(agent_id: TargetAgentId) -> &'static [&'static str] {
    match agent_id {
        TargetAgentId::CodexDesktop => &["/Applications/ChatGPT.app/", "/Applications/Codex.app/"],
        TargetAgentId::ClaudeDesktop => &["/Applications/Claude.app/"],
        TargetAgentId::Dsh => &["/Applications/DeepSeek Harness.app/"],
        TargetAgentId::Mimo => &["/Applications/Xiaomi MiMo.app/"],
        TargetAgentId::Zcode => &["/Applications/ZCode.app/"],
        TargetAgentId::Opencode => &["/opencode"],
    }
}

/// 一次快照内判定某 Agent 是否在运行所需的进程名（Windows）。
#[cfg(not(target_os = "macos"))]
fn running_exe_names(agent_id: TargetAgentId) -> &'static [&'static str] {
    match agent_id {
        TargetAgentId::CodexDesktop => &["Codex.exe", "ChatGPT.exe"],
        TargetAgentId::ClaudeDesktop => &["Claude.exe"],
        TargetAgentId::Dsh => &["DeepSeek Harness.exe"],
        TargetAgentId::Mimo => &["Xiaomi MiMo.exe"],
        TargetAgentId::Zcode => &["ZCode.exe"],
        TargetAgentId::Opencode => &["opencode.exe"],
    }
}

/// 单次命令行匹配（重启流程等单点判断使用；批量场景走 `process_snapshot`）。
#[cfg(target_os = "macos")]
fn proc_matches(pattern: &str) -> bool {
    Command::new("pgrep")
        .args(["-f", pattern])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 采集一次全量进程命令行快照。
///
/// 历史实现在每个 Agent 上各跑一次 `pgrep -f`（6 个 Agent 合计 7 个子进程），
/// 而快照构建一次就要做一轮：本机实测 75.7ms → 12.1ms。这里统一成**一次**外部调用，
/// 由 `agent_running_in_snapshot` 在内存里做匹配。
fn process_snapshot() -> String {
    #[cfg(target_os = "macos")]
    {
        // `pgrep -fl .` 输出「pid commandline」，一条命令拿到全部进程；
        // 失败时退回 ps，保证极端环境（pgrep 被限制）仍有结论。
        let out = Command::new("pgrep").args(["-fl", "."]).output();
        if let Ok(o) = out {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout).into_owned();
                if !text.trim().is_empty() {
                    return text;
                }
            }
        }
        Command::new("ps")
            .args(["-Ao", "pid=,command="])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut cmd = Command::new("tasklist");
        cmd.args(["/FO", "CSV", "/NH"]);
        // 该函数被前端每 6s 轮询一次；Windows 上若弹控制台黑框会形成周期性闪烁
        crate::platform::hide_console_std(&mut cmd);
        cmd.output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }
}

/// ASCII 大小写不敏感的子串查找（零分配）。
///
/// Windows 的 `tasklist` 输出的进程名大小写取决于磁盘上的实际文件名，未必与
/// `running_exe_name` 的常量写法一致（例如安装包落地为 `zcode.exe`，常量为
/// `ZCode.exe`）。Windows 文件系统本身大小写不敏感，判定也必须一致，否则会漏报
/// 「未运行」，进而跳过重启。
#[cfg(not(target_os = "macos"))]
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let n = needle.as_bytes();
    if n.is_empty() || haystack.len() < n.len() {
        return false;
    }
    haystack
        .as_bytes()
        .windows(n.len())
        .any(|w| w.eq_ignore_ascii_case(n))
}

/// 在给定快照里判定某 Agent 是否在运行（纯函数，便于单测）。
pub fn agent_running_in_snapshot(agent_id: TargetAgentId, snapshot: &str) -> bool {
    if snapshot.is_empty() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        running_patterns(agent_id)
            .iter()
            .any(|p| snapshot.contains(p))
    }
    #[cfg(not(target_os = "macos"))]
    {
        running_exe_names(agent_id).iter().any(|exe| {
            snapshot.lines().any(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with('"') {
                    trimmed
                        .split(',')
                        .next()
                        .map(|col| col.trim_matches('"').eq_ignore_ascii_case(exe))
                        .unwrap_or(false)
                } else {
                    contains_ignore_ascii_case(trimmed, exe)
                }
            })
        })
    }
}

fn is_agent_app_running(agent_id: TargetAgentId) -> bool {
    agent_running_in_snapshot(agent_id, &process_snapshot())
}

fn is_restartable_desktop_agent(agent_id: TargetAgentId) -> bool {
    matches!(
        agent_id,
        TargetAgentId::ClaudeDesktop
            | TargetAgentId::CodexDesktop
            | TargetAgentId::Mimo
            | TargetAgentId::Zcode
    )
}

/// 从进程快照中提取指定 Agent 当前正在运行的主进程 PID 列表（升序去重）。
pub fn agent_pids_in_snapshot(agent_id: TargetAgentId, snapshot: &str) -> Vec<u32> {
    if snapshot.is_empty() {
        return Vec::new();
    }
    let mut pids = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let patterns = running_patterns(agent_id);
        for line in snapshot.lines() {
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            if !patterns.iter().any(|p| trimmed.contains(p)) {
                continue;
            }
            if let Some(pid_token) = trimmed.split_whitespace().next() {
                if let Ok(pid) = pid_token.parse::<u32>() {
                    pids.push(pid);
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let exes = running_exe_names(agent_id);
        for line in snapshot.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('"') {
                let mut cols = trimmed.split(',');
                let img = cols.next().map(|c| c.trim_matches('"')).unwrap_or("");
                if exes.iter().any(|exe| img.eq_ignore_ascii_case(exe)) {
                    if let Some(pid_col) = cols.next() {
                        if let Ok(pid) = pid_col.trim_matches('"').trim().parse::<u32>() {
                            pids.push(pid);
                        }
                    }
                }
            }
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// 判定「待重启生效」是否已被满足：
/// - 若当前无任何进程运行（用户已退出客户端，下次启动自然读取新配置），返回 true；
/// - 若有写入前记录的 `stale_pids` 且当前运行进程中已无任何旧 PID（用户已完成手动重启），返回 true。
pub fn is_pending_restart_satisfied(stale_pids: Option<&[u32]>, current_pids: &[u32]) -> bool {
    if current_pids.is_empty() {
        return true;
    }
    if let Some(stale) = stale_pids {
        if !stale.is_empty() && !current_pids.iter().any(|pid| stale.contains(pid)) {
            return true;
        }
    }
    false
}

/// 根据给定进程快照自动对账所有 `NeedsRestart` 绑定：
/// 若进程已关闭或已更换为新 PID，自动恢复为 `Bound` 并写回数据库。
/// 返回本次被自动清除「待重启」状态的 Agent 列表。
pub fn reconcile_pending_restarts_with_snapshot(
    db: &QwitchDatabase,
    snapshot: &str,
) -> Vec<TargetAgentId> {
    let Ok(stored_list) = db.get_stored_agent_bindings() else {
        return Vec::new();
    };
    let mut cleared = Vec::new();
    for mut stored in stored_list {
        if stored.status != AgentBindingStatus::NeedsRestart {
            continue;
        }
        if stored.mounted_pool_ids.is_empty() {
            stored.status = AgentBindingStatus::Unbound;
            stored.options.pending_restart_pids = None;
            let _ = db.upsert_agent_binding(&stored);
            cleared.push(stored.agent_id);
            continue;
        }
        let current_pids = agent_pids_in_snapshot(stored.agent_id, snapshot);
        if is_pending_restart_satisfied(
            stored.options.pending_restart_pids.as_deref(),
            &current_pids,
        ) {
            stored.status = AgentBindingStatus::Bound;
            stored.options.pending_restart_pids = None;
            if db.upsert_agent_binding(&stored).is_ok() {
                cleared.push(stored.agent_id);
            }
        } else if stored.options.pending_restart_pids.is_none() && !current_pids.is_empty() {
            // 兼容旧版本落盘的 NeedsRestart 记录：首次补录当前 PID，之后用户手动重启即可自动识别清除
            stored.options.pending_restart_pids = Some(current_pids);
            let _ = db.upsert_agent_binding(&stored);
        }
    }
    cleared
}

/// 获取一次进程快照，并对账「待重启」状态后返回所有 Agent 的 `(agent_id, running, cleared_pending_restart)`。
pub fn collect_agent_running_states_and_reconcile(
    db: &QwitchDatabase,
) -> Vec<(TargetAgentId, bool, bool)> {
    let snapshot = process_snapshot();
    let cleared = reconcile_pending_restarts_with_snapshot(db, &snapshot);
    TargetAgentId::all()
        .into_iter()
        .map(|id| {
            (
                id,
                agent_running_in_snapshot(id, &snapshot),
                cleared.contains(&id),
            )
        })
        .collect()
}

/// 获取一次进程快照（供前端轻量轮询）。
pub fn take_process_snapshot() -> String {
    process_snapshot()
}

/// 一次性返回所有 Agent 的运行状态，供前端轻量轮询使用（不构建整份工作区快照）。
pub fn collect_agent_running_states() -> Vec<(TargetAgentId, bool)> {
    let snapshot = process_snapshot();
    TargetAgentId::all()
        .into_iter()
        .map(|id| (id, agent_running_in_snapshot(id, &snapshot)))
        .collect()
}

fn agent_meta(agent_id: TargetAgentId) -> (&'static str, &'static str, WireProtocolKind) {
    match agent_id {
        TargetAgentId::ClaudeDesktop => (
            "Claude 桌面版",
            "官方 3p 网关模式 · 同时挂载多号池，下拉框直选具体模型",
            WireProtocolKind::AnthropicMessages,
        ),
        TargetAgentId::Opencode => (
            "OpenCode",
            "CLI / TUI / OpenChamber 编码代理 · 支持多号池同列切换",
            WireProtocolKind::OpenaiChat,
        ),
        TargetAgentId::Dsh => (
            "DeepSeek Harness",
            "dsh 桌面端与 Web 代理 · 写入 llm-pi-ai 自定义路由",
            WireProtocolKind::OpenaiChat,
        ),
        TargetAgentId::CodexDesktop => (
            "Codex 桌面版",
            "ChatGPT / Codex App · 独立 [model_providers.qwitch] Responses 端点",
            WireProtocolKind::OpenaiResponses,
        ),
        TargetAgentId::Mimo => (
            "MiMo Code",
            "小米 MiMo / MiMo Code · OpenCode 兼容 provider，直连本地网关",
            WireProtocolKind::OpenaiChat,
        ),
        TargetAgentId::Zcode => (
            "ZCode",
            "ZCode 桌面版 · Anthropic Messages 自定义 provider",
            WireProtocolKind::AnthropicMessages,
        ),
    }
}

pub fn build_agent_binding_dto(
    stored: &StoredAgentBinding,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
    // 批量构建时传入的一次性进程快照；单条构建传 None 由本函数自行探测
    running_snapshot: Option<&str>,
) -> AgentBinding {
    let paths = resolve_agent_paths(stored.agent_id, &stored.options, base_dir_override);
    let mut options = stored.options.clone();
    let drift_reason = check_disk_drift(
        stored.agent_id,
        &stored.mounted_pool_ids,
        &paths,
        port,
        &options,
    );
    if stored.agent_id == TargetAgentId::Dsh {
        let creds_written = paths
            .files
            .get(2)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .is_some_and(|c| c.contains("QWITCH_GATEWAY_KEY"));
        options.dsh_dual_creds_written = Some(creds_written);
    }

    let status = if stored.mounted_pool_ids.is_empty() {
        AgentBindingStatus::Unbound
    } else if drift_reason.is_some() {
        AgentBindingStatus::Drifted
    } else if stored.status == AgentBindingStatus::NeedsRestart {
        AgentBindingStatus::NeedsRestart
    } else {
        AgentBindingStatus::Bound
    };

    let (before_snippet, after_snippet) = build_agent_config_snippets(
        stored.agent_id,
        &stored.mounted_pool_ids,
        stored.default_model_key.as_deref(),
        all_pools,
        host,
        port,
        &options,
        status == AgentBindingStatus::Drifted,
    );

    let (name, subtitle, protocol) = agent_meta(stored.agent_id);
    let proxy_warning = if stored.agent_id == TargetAgentId::CodexDesktop && global_proxy {
        Some(
            "检测到系统开启 HTTP 全局代理，Codex 内置 reqwest 默认不读系统忽略列表，直连 127.0.0.1 可能返回 502（建议设置 NO_PROXY=127.0.0.1,localhost）"
                .into(),
        )
    } else {
        None
    };

    let restart_hint = if status == AgentBindingStatus::NeedsRestart {
        Some(match stored.agent_id {
            TargetAgentId::ClaudeDesktop => {
                "Claude 桌面版仅在启动时读取 3p 配置，请重启以拉取最新多号池 /v1/models".into()
            }
            TargetAgentId::CodexDesktop => {
                "已写入 ~/.codex/config.toml，请重启 app-server（无需关闭桌面窗口）".into()
            }
            _ => "配置已更新，重启后生效".into(),
        })
    } else {
        None
    };

    AgentBinding {
        agent_id: stored.agent_id,
        name: name.into(),
        subtitle: subtitle.into(),
        protocol,
        config_path: paths.primary_display,
        installed: paths.installed,
        running: match running_snapshot {
            Some(snapshot) => agent_running_in_snapshot(stored.agent_id, snapshot),
            None => is_agent_app_running(stored.agent_id),
        },
        version: paths.version,
        status,
        mounted_pool_ids: stored.mounted_pool_ids.clone(),
        default_model_key: stored.default_model_key.clone(),
        drift_reason,
        proxy_warning,
        restart_hint,
        last_hit_at: stored.last_hit_at.clone(),
        options,
        gateway_token: Some(agent_token(stored.agent_id)),
        before_snippet,
        after_snippet,
    }
}

pub fn inspect_all_agent_bindings(
    db: &QwitchDatabase,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
) -> Result<Vec<AgentBinding>, String> {
    // 单个快照服务全部 6 个 Agent：避免每个 Agent 各起一个 pgrep（本机实测省下约 64ms）。
    let snapshot = process_snapshot();
    if base_dir_override.is_none() {
        let _ = reconcile_pending_restarts_with_snapshot(db, &snapshot);
    }
    let stored_list = db.get_stored_agent_bindings().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(TargetAgentId::all().len());
    for agent_id in TargetAgentId::all() {
        let stored = stored_list
            .iter()
            .find(|b| b.agent_id == agent_id)
            .cloned()
            .unwrap_or_else(|| StoredAgentBinding {
                agent_id,
                mounted_pool_ids: Vec::new(),
                default_model_key: None,
                options: default_options_for_agent(agent_id),
                status: AgentBindingStatus::Unbound,
                last_hit_at: None,
            });
        out.push(build_agent_binding_dto(
            &stored,
            all_pools,
            host,
            port,
            global_proxy,
            base_dir_override,
            Some(&snapshot),
        ));
    }
    Ok(out)
}

fn stash_original_files(
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    agent_id: TargetAgentId,
    paths: &[PathBuf],
) -> Result<(), String> {
    for p in paths {
        let path_str = p.to_string_lossy().to_string();
        match std::fs::read(p) {
            Ok(bytes) => db
                .save_agent_stash(master_key, agent_id, &path_str, true, Some(&bytes))
                .map_err(|e| e.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => db
                .save_agent_stash(master_key, agent_id, &path_str, false, None)
                .map_err(|e| e.to_string())?,
            Err(e) => return Err(format!("读取原始配置 {} 失败: {e}", p.display())),
        }
    }
    Ok(())
}

fn upsert_toml_key_line(content: &str, key: &str, new_line: &str) -> String {
    let prefix = format!("{key} =");
    let prefix_tight = format!("{key}=");
    let mut replaced = false;
    let mut lines: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = true;
        }
        if !in_section
            && (trimmed.starts_with(&prefix) || trimmed.starts_with(&prefix_tight))
            && !replaced
        {
            lines.push(new_line.to_string());
            replaced = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced {
        lines.insert(0, new_line.to_string());
    }
    lines.join("\n") + "\n"
}

fn is_matching_toml_section_or_subtable(trimmed: &str, section_header: &str) -> bool {
    if trimmed == section_header {
        return true;
    }
    if let (Some(base), Some(rest)) = (section_header.strip_suffix(']'), trimmed.strip_suffix(']'))
    {
        return rest
            .strip_prefix(base)
            .is_some_and(|suffix| suffix.starts_with('.'));
    }
    false
}

fn strip_toml_section(content: &str, section_header: &str) -> String {
    let mut out = Vec::new();
    let mut skipping = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            skipping = is_matching_toml_section_or_subtable(trimmed, section_header);
        }
        if !skipping {
            out.push(line);
        }
    }
    out.join("\n")
}

fn extract_toml_section_block(content: &str, section_header: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut capturing = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if is_matching_toml_section_or_subtable(trimmed, section_header) {
                capturing = true;
            } else if capturing {
                break;
            }
        }
        if capturing {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        None
    } else {
        let mut out = lines.join("\n");
        out.push('\n');
        Some(out)
    }
}

fn strip_jsonc_comments(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    let mut in_str = false;
    let mut escaped = false;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn write_json_deployment_mode_3p(path: &Path) -> Result<(), String> {
    let mut doc: Value = match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s).map_err(|e| {
            format!(
                "拒绝覆盖无法解析的 JSON 配置 {}：{e}（请先修复该文件语法，Qwitch 不会清空你的原有配置）",
                path.display()
            )
        })?,
        _ => json!({}),
    };
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("deploymentMode".into(), Value::String("3p".into()));
    }
    write_private_file_atomic(
        path,
        serde_json::to_string_pretty(&doc)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())
}

/// 去除 JSONC 中位于 `}` / `]` 之前的尾随逗号（字符状态机，尊重字符串）。
fn strip_trailing_commas(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut in_str = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            // 向后看是否只剩空白后跟 } 或 ]
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1; // 跳过该逗号
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 宽松解析 JSONC（去注释 + 去尾随逗号）。解析失败返回错误而**绝不**回退为空文档。
fn parse_jsonc_lenient(raw: &str) -> Result<Value, String> {
    let cleaned = strip_jsonc_comments(raw);
    if let Ok(v) = serde_json::from_str::<Value>(&cleaned) {
        return Ok(v);
    }
    let no_trailing = strip_trailing_commas(&cleaned);
    serde_json::from_str::<Value>(&no_trailing).map_err(|e| {
        format!("无法解析 JSONC：{e}（请先修复该文件语法，Qwitch 不会清空你的原有配置）")
    })
}

fn upsert_env_key(existing: &str, key_name: &str, value: &str) -> String {
    let key_prefix = format!("{key_name}=");
    let export_prefix = format!("export {key_name}=");
    let mut replaced = false;
    let mut lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&key_prefix) || trimmed.starts_with(&export_prefix) {
            if !replaced {
                lines.push(format!("{key_name}={value}"));
                replaced = true;
            }
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced {
        lines.push(format!("{key_name}={value}"));
    }
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn upsert_dsh_credentials_yaml(existing: &str, key_name: &str, token: &str) -> String {
    let key_prefix = format!("{key_name}:");
    let mut replaced_existing = false;
    let mut lines: Vec<String> = Vec::new();

    for line in existing.lines() {
        if line.trim_start().starts_with(&key_prefix) {
            if !replaced_existing {
                lines.push(format!("  {key_name}: \"{token}\""));
                replaced_existing = true;
            }
        } else {
            lines.push(line.to_string());
        }
    }
    if replaced_existing {
        let mut repaired: Vec<String> = Vec::with_capacity(lines.len());
        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if (trimmed == "refs: {}" || trimmed == "refs:{}")
                && lines
                    .get(idx + 1)
                    .is_some_and(|next| next.starts_with(' ') || next.starts_with('\t'))
            {
                repaired.push("refs:".to_string());
            } else {
                repaired.push(line.clone());
            }
        }
        let mut out = repaired.join("\n");
        if !out.ends_with('\n') {
            out.push('\n');
        }
        return out;
    }

    let mut out_lines: Vec<String> = Vec::new();
    let mut inserted_in_refs = false;
    for line in &lines {
        let trimmed = line.trim();
        if !inserted_in_refs
            && (trimmed == "refs:" || trimmed == "refs: {}" || trimmed == "refs:{}")
        {
            out_lines.push("refs:".to_string());
            out_lines.push(format!("  {key_name}: \"{token}\""));
            inserted_in_refs = true;
        } else {
            out_lines.push(line.clone());
        }
    }

    if !inserted_in_refs {
        let has_version = out_lines
            .iter()
            .any(|l| l.trim_start().starts_with("version:"));
        if !has_version {
            out_lines.insert(0, "version: 1".to_string());
        }
        out_lines.push("refs:".to_string());
        out_lines.push(format!("  {key_name}: \"{token}\""));
    }

    let mut out = out_lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn merge_dsh_cordis_patch(existing: &str, qwitch_entry: &str) -> String {
    let mut preserved: Vec<String> = Vec::new();
    let mut skipping_llm_pi_ai = false;

    for line in existing.lines() {
        let trimmed = line.trim();
        let is_top_list_item =
            !line.starts_with(' ') && !line.starts_with('\t') && trimmed.starts_with("- ");
        if is_top_list_item {
            if trimmed == "- id: llm-pi-ai" || trimmed == "- id: \"llm-pi-ai\"" {
                skipping_llm_pi_ai = true;
                continue;
            }
            skipping_llm_pi_ai = false;
        }
        if !skipping_llm_pi_ai && !trimmed.is_empty() && !trimmed.starts_with('#') {
            preserved.push(line.to_string());
        } else if !skipping_llm_pi_ai && trimmed.is_empty() && !preserved.is_empty() {
            preserved.push(String::new());
        }
    }

    let mut out = qwitch_entry.trim_end().to_string();
    let preserved_body = preserved.join("\n").trim().to_string();
    if !preserved_body.is_empty() {
        out.push('\n');
        out.push_str(&preserved_body);
    }
    out.push('\n');
    out
}

fn load_codex_builtin_models(codex_dir: &Path) -> Vec<Value> {
    for candidate in ["models_cache.json", "models_cache.codex_work_desktop.json"] {
        let path = codex_dir.join(candidate);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if let Some(arr) = doc.get("models").and_then(Value::as_array) {
            if !arr.is_empty() {
                return arr.clone();
            }
        }
    }
    Vec::new()
}

fn merge_codex_model_catalog(builtin_models: Vec<Value>, qwitch_entries: Vec<Value>) -> Vec<Value> {
    let mut seen_slugs = std::collections::HashSet::new();
    let mut merged = Vec::with_capacity(builtin_models.len() + qwitch_entries.len());

    for item in builtin_models {
        if let Some(slug) = item.get("slug").and_then(Value::as_str) {
            if !slug.trim().is_empty() {
                seen_slugs.insert(slug.to_string());
            }
        }
        merged.push(item);
    }

    let builtin_count = merged.len();
    let mut appended_idx = 0usize;
    for mut item in qwitch_entries {
        let slug = item
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !slug.is_empty() && !seen_slugs.insert(slug) {
            continue;
        }
        if let Some(obj) = item.as_object_mut() {
            obj.insert(
                "priority".into(),
                json!((builtin_count + appended_idx) as i64),
            );
        }
        appended_idx += 1;
        merged.push(item);
    }

    merged
}

#[allow(clippy::too_many_arguments)]
fn write_opencode_like_provider(
    path: &Path,
    base_url: &str,
    token: &str,
    exposed: &[AgentExposedModel],
    default_wire: &str,
    v1: bool,
) -> Result<(), String> {
    let mut doc: Value = match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => parse_jsonc_lenient(&s)?,
        _ => json!({}),
    };
    if let Some(obj) = doc.as_object_mut() {
        obj.insert(
            "model".into(),
            Value::String(format!("qwitch/{default_wire}")),
        );
        let mut models_map = serde_json::Map::new();
        for m in exposed {
            let entry = if v1 {
                json!({"name": m.display_name, "limit": {"context": 200000, "output": 64000}, "modalities": {"input": ["text", "image"], "output": ["text"]}, "tool_call": true})
            } else {
                json!({"name": m.display_name, "capabilities": {"tools": true, "input": ["text", "image"], "output": ["text"]}, "limit": {"context": 200000, "output": 64000}})
            };
            models_map.insert(m.wire_model_id.clone(), entry);
        }
        if v1 {
            let pm = obj
                .entry("provider")
                .or_insert_with(|| json!({}))
                .as_object_mut();
            if let Some(pm) = pm {
                pm.insert(
                    "qwitch".into(),
                    json!({"npm": "@ai-sdk/openai-compatible", "name": "Qwitch 号池网关", "options": {"baseURL": format!("{base_url}/v1"), "apiKey": token}, "models": models_map}),
                );
            }
        } else {
            let pm = obj
                .entry("providers")
                .or_insert_with(|| json!({}))
                .as_object_mut();
            if let Some(pm) = pm {
                pm.insert(
                    "qwitch".into(),
                    json!({"name": "Qwitch 号池网关", "package": "aisdk:@ai-sdk/openai-compatible", "settings": {"baseURL": format!("{base_url}/v1"), "apiKey": token}, "models": models_map}),
                );
            }
        }
    }
    write_private_file_atomic(
        path,
        serde_json::to_string_pretty(&doc)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())
}

fn write_zcode_provider(
    config_path: &Path,
    rules_path: &Path,
    base_url: &str,
    token: &str,
    exposed: &[AgentExposedModel],
) -> Result<(), String> {
    // ZCode config.json：OpenCode 形状的 provider，kind=anthropic，baseURL 根地址
    let mut doc: Value = match std::fs::read_to_string(config_path) {
        Ok(s) if !s.trim().is_empty() => {
            serde_json::from_str(&s).map_err(|e| format!("ZCode config.json 解析失败：{e}"))?
        }
        _ => json!({}),
    };
    let mut models = serde_json::Map::new();
    for m in exposed {
        models.insert(
            m.wire_model_id.clone(),
            json!({"name": m.display_name, "limit": {"context": 200000, "output": 64000}, "modalities": {"input": ["text"], "output": ["text"]}}),
        );
    }
    if let Some(obj) = doc.as_object_mut() {
        let provider = obj
            .entry("provider")
            .or_insert_with(|| json!({}))
            .as_object_mut();
        if let Some(provider) = provider {
            provider.insert(
                "qwitch".into(),
                json!({"name": "Qwitch 号池网关", "kind": "anthropic", "options": {"apiKey": token, "baseURL": base_url}, "enabled": true, "source": "custom", "models": models}),
            );
        }
    }
    write_private_file_atomic(
        config_path,
        serde_json::to_string_pretty(&doc)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())?;

    // ZCode 3.14+ provider_config.json：rules 形式
    let mut rules: Value = match std::fs::read_to_string(rules_path) {
        Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s)
            .map_err(|e| format!("ZCode provider_config.json 解析失败：{e}"))?,
        _ => json!({"schemaVersion": 1, "config": {}}),
    };
    let ids: Vec<Value> = exposed
        .iter()
        .map(|m| Value::String(m.wire_model_id.clone()))
        .collect();
    if let Some(root) = rules.as_object_mut() {
        root.entry("schemaVersion").or_insert_with(|| json!(1));
        let cfg = root
            .entry("config")
            .or_insert_with(|| json!({}))
            .as_object_mut();
        if let Some(cfg) = cfg {
            let pcr = cfg
                .entry("providerConfigRules")
                .or_insert_with(|| json!({}))
                .as_object_mut();
            if let Some(pcr) = pcr {
                let arr = pcr
                    .entry("providerRules")
                    .or_insert_with(|| json!([]))
                    .as_array_mut();
                if let Some(arr) = arr {
                    arr.retain(|v| v.get("providerId").and_then(Value::as_str) != Some("qwitch"));
                    arr.push(json!({
                        "providerId": "qwitch",
                        "providerName": "Qwitch 号池网关",
                        "enabled": true,
                        "config": {
                            "group": "standard-personal",
                            "access": {"type": "api-key", "apiKey": token},
                            "api": {"type": "anthropic-messages", "baseUrl": base_url},
                            "personalModelIds": ids.clone(),
                            "modelOrder": ids
                        }
                    }));
                }
            }
            let mcr = cfg
                .entry("modelConfigRules")
                .or_insert_with(|| json!({}))
                .as_object_mut();
            if let Some(mcr) = mcr {
                let arr = mcr
                    .entry("providerModelRules")
                    .or_insert_with(|| json!([]))
                    .as_array_mut();
                if let Some(arr) = arr {
                    arr.retain(|v| v.get("providerId").and_then(Value::as_str) != Some("qwitch"));
                    for m in exposed {
                        arr.push(json!({"providerId": "qwitch", "modelId": m.wire_model_id.clone(), "config": {"enabled": true}}));
                    }
                }
                mcr.entry("manualProviderModelRules")
                    .or_insert_with(|| json!([]));
            }
        }
    }
    write_private_file_atomic(
        rules_path,
        serde_json::to_string_pretty(&rules)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
pub fn apply_agent_wiring(
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    agent_id: TargetAgentId,
    mounted_pool_ids: &[String],
    default_model_key: Option<&str>,
    options: &AgentBindingOptions,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
) -> Result<AgentBinding, String> {
    if mounted_pool_ids.is_empty() {
        return unbind_and_restore_agent(
            db,
            master_key,
            agent_id,
            all_pools,
            host,
            port,
            global_proxy,
            base_dir_override,
        );
    }

    let mut clean_opts = options.clone();
    let paths = resolve_agent_paths(agent_id, &clean_opts, base_dir_override);
    stash_original_files(db, master_key, agent_id, &paths.files)?;

    let exposed = compute_agent_exposed_models(
        agent_id,
        mounted_pool_ids,
        default_model_key,
        all_pools,
        chrono::Utc::now().timestamp_millis(),
    );
    let default_item = exposed
        .iter()
        .find(|m| m.is_default)
        .or_else(|| exposed.first());
    let effective_default_key = default_item.map(|m| m.key.clone());
    let default_wire = default_item
        .map(|m| m.wire_model_id.clone())
        .unwrap_or_else(|| "coding-main/deepseek-v4-pro".into());
    let base_url = format!("http://{host}:{port}");
    let token = agent_token(agent_id);

    // 将整套多文件写入包成一次逻辑事务：任一文件失败即按 stash 回滚已写文件，
    // 避免留下“半新半旧”的 Agent 配置。
    let write_result: Result<(), String> = (|| {
        match agent_id {
            TargetAgentId::ClaudeDesktop => {
                write_json_deployment_mode_3p(&paths.files[0])?;

                let lib_cfg = json!({
                    "inferenceProvider": "gateway",
                    "inferenceGatewayBaseUrl": base_url,
                    "inferenceGatewayApiKey": token,
                    "inferenceGatewayAuthScheme": "bearer",
                    "disableDeploymentModeChooser": true,
                    "coworkEgressAllowedHosts": ["*"],
                    "toolSearchEnabled": true
                });
                let lib_bytes =
                    serde_json::to_string_pretty(&lib_cfg).map_err(|e| e.to_string())?;
                let meta_bytes =
                    serde_json::to_string_pretty(&json!({ "appliedId": CLAUDE_CONFIG_UUID }))
                        .map_err(|e| e.to_string())?;

                write_private_file_atomic(&paths.files[1], lib_bytes.as_bytes())
                    .map_err(|e| e.to_string())?;
                write_private_file_atomic(&paths.files[2], meta_bytes.as_bytes())
                    .map_err(|e| e.to_string())?;

                if let Some(official_3p_cfg) = paths.files.get(3) {
                    write_json_deployment_mode_3p(official_3p_cfg)?;
                }
                if let Some(official_3p_lib) = paths.files.get(4) {
                    write_private_file_atomic(official_3p_lib, lib_bytes.as_bytes())
                        .map_err(|e| e.to_string())?;
                }
                if let Some(official_3p_meta) = paths.files.get(5) {
                    write_private_file_atomic(official_3p_meta, meta_bytes.as_bytes())
                        .map_err(|e| e.to_string())?;
                }
                if let Some(twin_cfg) = paths.files.get(6) {
                    write_json_deployment_mode_3p(twin_cfg)?;
                }
            }
            TargetAgentId::Opencode => {
                let mut doc: Value = match std::fs::read_to_string(&paths.files[0]) {
                    Ok(s) if !s.trim().is_empty() => parse_jsonc_lenient(&s)?,
                    _ => json!({}),
                };
                let is_v1 = clean_opts.opencode_schema_version.as_deref() == Some("v1");
                if let Some(obj) = doc.as_object_mut() {
                    obj.insert(
                        "model".into(),
                        Value::String(format!("qwitch/{default_wire}")),
                    );
                    if is_v1 {
                        if let Some(pm) = obj.get_mut("providers").and_then(Value::as_object_mut) {
                            pm.remove("qwitch");
                        }
                        let mut models_map = serde_json::Map::new();
                        for m in &exposed {
                            models_map.insert(
                            m.wire_model_id.clone(),
                            json!({
                                "name": m.display_name,
                                "limit": { "context": 200000, "output": 64000 },
                                "modalities": { "input": ["text", "image"], "output": ["text"] },
                                "tool_call": true
                            }),
                        );
                        }
                        let provider_map = obj
                            .entry("provider")
                            .or_insert_with(|| json!({}))
                            .as_object_mut();
                        if let Some(pm) = provider_map {
                            pm.insert(
                                "qwitch".into(),
                                json!({
                                    "npm": "@ai-sdk/openai-compatible",
                                    "name": "Qwitch 号池网关",
                                    "options": {
                                        "baseURL": format!("{base_url}/v1"),
                                        "apiKey": token
                                    },
                                    "models": models_map
                                }),
                            );
                        }
                    } else {
                        if let Some(pm) = obj.get_mut("provider").and_then(Value::as_object_mut) {
                            pm.remove("qwitch");
                        }
                        let mut models_map = serde_json::Map::new();
                        for m in &exposed {
                            models_map.insert(
                                m.wire_model_id.clone(),
                                json!({
                                    "name": m.display_name,
                                    "capabilities": {
                                        "tools": true,
                                        "input": ["text", "image"],
                                        "output": ["text"]
                                    },
                                    "limit": { "context": 200000, "output": 64000 }
                                }),
                            );
                        }
                        let providers_map = obj
                            .entry("providers")
                            .or_insert_with(|| json!({}))
                            .as_object_mut();
                        if let Some(pm) = providers_map {
                            pm.insert(
                                "qwitch".into(),
                                json!({
                                    "name": "Qwitch 号池网关",
                                    "package": "aisdk:@ai-sdk/openai-compatible",
                                    "settings": {
                                        "baseURL": format!("{base_url}/v1"),
                                        "apiKey": token
                                    },
                                    "models": models_map
                                }),
                            );
                        }
                    }
                }
                write_private_file_atomic(
                    &paths.files[0],
                    serde_json::to_string_pretty(&doc)
                        .map_err(|e| e.to_string())?
                        .as_bytes(),
                )
                .map_err(|e| e.to_string())?;
            }
            TargetAgentId::Mimo => {
                let v1 = clean_opts.opencode_schema_version.as_deref() != Some("v2");
                write_opencode_like_provider(
                    &paths.files[0],
                    &base_url,
                    &token,
                    &exposed,
                    &default_wire,
                    v1,
                )?;
            }
            TargetAgentId::Zcode => {
                let rules_path = paths
                    .files
                    .get(1)
                    .cloned()
                    .unwrap_or_else(|| paths.files[0].with_file_name("provider_config.json"));
                write_zcode_provider(&paths.files[0], &rules_path, &base_url, &token, &exposed)?;
            }
            TargetAgentId::Dsh => {
                let models_yaml = if exposed.is_empty() {
                    format!(
                    "          - id: \"{default_wire}\"\n            name: \"Qwitch Default\"\n            contextWindow: 200000\n            maxTokens: 64000\n            input: [text, image]"
                )
                } else {
                    exposed
                    .iter()
                    .map(|m| {
                        format!(
                            "          - id: \"{}\"\n            name: \"{}\"\n            contextWindow: 200000\n            maxTokens: 64000\n            input: [text, image]",
                            m.wire_model_id, m.display_name
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                };
                let existing_patch = std::fs::read_to_string(&paths.files[0]).unwrap_or_default();
                let qwitch_entry = format!(
                "- id: llm-pi-ai\n  name: \"@deepseek-ai/dsh-llm-pi-ai\"\n  config:\n    providers:\n      qwitch:\n        displayName: Qwitch\n        apiKeyEnv: QWITCH_GATEWAY_KEY\n        api: openai-completions\n        baseURL: \"{base_url}/v1\"\n        models:\n{models_yaml}\n        streamIdleTimeoutMs: 600000\n"
            );
                let patch_content = merge_dsh_cordis_patch(&existing_patch, &qwitch_entry);
                write_private_file_atomic(&paths.files[0], patch_content.as_bytes())
                    .map_err(|e| e.to_string())?;

                // 双写 .env 与 .credentials.yaml（dsh 桌面版强依赖 .credentials.yaml）
                let existing_env = std::fs::read_to_string(&paths.files[1]).unwrap_or_default();
                let env_text = upsert_env_key(&existing_env, "QWITCH_GATEWAY_KEY", &token);
                write_private_file_atomic(&paths.files[1], env_text.as_bytes())
                    .map_err(|e| e.to_string())?;

                let creds_text = std::fs::read_to_string(&paths.files[2]).unwrap_or_default();
                let next_creds =
                    upsert_dsh_credentials_yaml(&creds_text, "QWITCH_GATEWAY_KEY", &token);
                write_private_file_atomic(&paths.files[2], next_creds.as_bytes())
                    .map_err(|e| e.to_string())?;
                clean_opts.dsh_dual_creds_written = Some(true);
            }
            TargetAgentId::CodexDesktop => {
                let catalog_entries: Vec<Value> = exposed
                .iter()
                .enumerate()
                .map(|(idx, m)| {
                    json!({
                        "slug": m.wire_model_id,
                        "display_name": m.display_name,
                        "description": format!("Qwitch {} ({})", m.pool_name, m.tier_summary),
                        "context_window": 272000,
                        "auto_compact_token_limit": 244800,
                        "base_instructions": "You are a helpful coding assistant.",
                        "default_reasoning_level": "medium",
                        "supported_reasoning_levels": [
                            { "effort": "low", "description": "Fast responses with light reasoning" },
                            { "effort": "medium", "description": "Balances speed and reasoning depth" },
                            { "effort": "high", "description": "Deep reasoning for complex coding tasks" }
                        ],
                        "shell_type": "shell_command",
                        "visibility": "list",
                        "supported_in_api": true,
                        "priority": idx as i64,
                        "upgrade": Value::Null,
                        "supports_reasoning_summaries": false,
                        "default_reasoning_summary": "auto",
                        "support_verbosity": false,
                        "default_verbosity": Value::Null,
                        "apply_patch_tool_type": "freeform",
                        "truncation_policy": { "mode": "tokens", "limit": 10000 },
                        "supports_parallel_tool_calls": true,
                        "experimental_supported_tools": [],
                        "input_modalities": ["text", "image"],
                        "prefer_websockets": false
                    })
                })
                .collect();
                let codex_dir = paths.files[0].parent().unwrap_or_else(|| Path::new("."));
                let builtin_models = load_codex_builtin_models(codex_dir);
                let merged_entries = merge_codex_model_catalog(builtin_models, catalog_entries);
                let catalog = json!({ "models": merged_entries });
                write_private_file_atomic(
                    &paths.files[1],
                    serde_json::to_string_pretty(&catalog)
                        .map_err(|e| e.to_string())?
                        .as_bytes(),
                )
                .map_err(|e| e.to_string())?;

                let catalog_path_escaped = paths.files[1]
                    .to_string_lossy()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"");
                let existing = std::fs::read_to_string(&paths.files[0]).unwrap_or_default();
                let stripped = strip_toml_section(&existing, "[model_providers.qwitch]");
                let with_provider = upsert_toml_key_line(
                    &stripped,
                    "model_provider",
                    "model_provider = \"qwitch\"",
                );
                let with_catalog = upsert_toml_key_line(
                    &with_provider,
                    "model_catalog_json",
                    &format!("model_catalog_json = \"{catalog_path_escaped}\""),
                );
                let mut with_model = upsert_toml_key_line(
                    &with_catalog,
                    "model",
                    &format!("model = \"{default_wire}\""),
                );
                if !with_model.ends_with('\n') {
                    with_model.push('\n');
                }
                with_model.push_str(&format!(
                "\n[model_providers.qwitch]\nname = \"Qwitch Multi-Pool Gateway\"\nbase_url = \"{base_url}/v1\"\nwire_api = \"responses\"\nexperimental_bearer_token = \"{token}\"\nhttp_headers = {{ \"x-openai-actor-authorization\" = \"qwitch\" }}\nrequires_openai_auth = true\n"
            ));
                write_private_file_atomic(&paths.files[0], with_model.as_bytes())
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })();
    if let Err(e) = write_result {
        if let Ok(stashes) = db.get_agent_stashes(master_key, agent_id) {
            for item in &stashes {
                let p = PathBuf::from(&item.file_path);
                if item.original_existed {
                    if let Some(bytes) = &item.original_content {
                        let _ = write_private_file_atomic(&p, bytes);
                    }
                } else {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        return Err(format!("写入 Agent 配置失败，已回滚原有配置：{e}"));
    }

    let (next_status, running_snapshot) = if is_restartable_desktop_agent(agent_id) {
        if base_dir_override.is_none() {
            let snapshot = process_snapshot();
            let pids = agent_pids_in_snapshot(agent_id, &snapshot);
            if pids.is_empty() {
                clean_opts.pending_restart_pids = None;
                (AgentBindingStatus::Bound, Some(snapshot))
            } else {
                clean_opts.pending_restart_pids = Some(pids);
                (AgentBindingStatus::NeedsRestart, Some(snapshot))
            }
        } else {
            clean_opts.pending_restart_pids = None;
            (AgentBindingStatus::NeedsRestart, None)
        }
    } else {
        clean_opts.pending_restart_pids = None;
        (AgentBindingStatus::Bound, None)
    };

    let stored = StoredAgentBinding {
        agent_id,
        mounted_pool_ids: mounted_pool_ids.to_vec(),
        default_model_key: effective_default_key,
        options: clean_opts,
        status: next_status,
        last_hit_at: None,
    };
    db.upsert_agent_binding(&stored)
        .map_err(|e| e.to_string())?;

    Ok(build_agent_binding_dto(
        &stored,
        all_pools,
        host,
        port,
        global_proxy,
        base_dir_override,
        running_snapshot.as_deref(),
    ))
}

#[allow(clippy::too_many_arguments)]
fn read_or(path: &Path, default: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|_| default.to_string())
}

fn set_deployment_mode(path: &Path, mode: &str) -> Result<(), String> {
    let mut doc: Value = match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s).map_err(|e| e.to_string())?,
        _ => return Ok(()),
    };
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("deploymentMode".into(), Value::String(mode.into()));
    }
    write_private_file_atomic(
        path,
        serde_json::to_string_pretty(&doc)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())
}

fn remove_qwitch_from_opencode_like(path: &Path) -> Result<(), String> {
    let raw = read_or(path, "{}");
    if raw.trim().is_empty() {
        return Ok(());
    }
    let mut doc: Value = parse_jsonc_lenient(&raw)?;
    if let Some(obj) = doc.as_object_mut() {
        for key in ["provider", "providers"] {
            if let Some(pm) = obj.get_mut(key).and_then(|v| v.as_object_mut()) {
                pm.remove("qwitch");
            }
        }
        for key in ["model", "small_model"] {
            if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
                if s.starts_with("qwitch/") {
                    obj.remove(key);
                }
            }
        }
    }
    write_private_file_atomic(
        path,
        serde_json::to_string_pretty(&doc)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())
}

fn remove_qwitch_from_zcode(config_path: &Path, rules_path: &Path) -> Result<(), String> {
    if let Ok(raw) = std::fs::read_to_string(config_path) {
        if !raw.trim().is_empty() {
            if let Ok(mut doc) = serde_json::from_str::<Value>(&raw) {
                if let Some(pm) = doc.get_mut("provider").and_then(|v| v.as_object_mut()) {
                    pm.remove("qwitch");
                }
                write_private_file_atomic(
                    config_path,
                    serde_json::to_string_pretty(&doc)
                        .map_err(|e| e.to_string())?
                        .as_bytes(),
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    if let Ok(raw) = std::fs::read_to_string(rules_path) {
        if !raw.trim().is_empty() {
            if let Ok(mut rules) = serde_json::from_str::<Value>(&raw) {
                if let Some(cfg) = rules.get_mut("config").and_then(|v| v.as_object_mut()) {
                    if let Some(pcr) = cfg
                        .get_mut("providerConfigRules")
                        .and_then(|v| v.as_object_mut())
                    {
                        if let Some(arr) =
                            pcr.get_mut("providerRules").and_then(|v| v.as_array_mut())
                        {
                            arr.retain(|v| {
                                v.get("providerId").and_then(Value::as_str) != Some("qwitch")
                            });
                        }
                    }
                    if let Some(mcr) = cfg
                        .get_mut("modelConfigRules")
                        .and_then(|v| v.as_object_mut())
                    {
                        for key in ["providerModelRules", "manualProviderModelRules"] {
                            if let Some(arr) = mcr.get_mut(key).and_then(|v| v.as_array_mut()) {
                                arr.retain(|v| {
                                    v.get("providerId").and_then(Value::as_str) != Some("qwitch")
                                });
                            }
                        }
                    }
                }
                write_private_file_atomic(
                    rules_path,
                    serde_json::to_string_pretty(&rules)
                        .map_err(|e| e.to_string())?
                        .as_bytes(),
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

fn remove_qwitch_from_dsh(paths: &AgentPaths) -> Result<(), String> {
    // .env 与 .credentials.yaml 去掉 QWITCH_GATEWAY_KEY 行
    for idx in [1usize, 2usize] {
        if let Some(p) = paths.files.get(idx) {
            if let Ok(raw) = std::fs::read_to_string(p) {
                if raw.contains("QWITCH_GATEWAY_KEY") {
                    let kept: Vec<&str> = raw
                        .lines()
                        .filter(|l| !l.trim_start().starts_with("QWITCH_GATEWAY_KEY"))
                        .collect();
                    let mut next = kept.join("\n");
                    if !next.is_empty() && !next.ends_with('\n') {
                        next.push('\n');
                    }
                    write_private_file_atomic(p, next.as_bytes()).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    // cordis.patch.yml 去掉包含 QWITCH_GATEWAY_KEY 的 `- id: llm-pi-ai` 段落
    if let Some(p) = paths.files.first() {
        if let Ok(raw) = std::fs::read_to_string(p) {
            if raw.contains("QWITCH_GATEWAY_KEY") {
                let lines: Vec<&str> = raw.lines().collect();
                let mut out: Vec<String> = Vec::new();
                let mut i = 0usize;
                while i < lines.len() {
                    if lines[i].trim_start().starts_with("- id: llm-pi-ai") {
                        let start = i;
                        let mut j = i + 1;
                        while j < lines.len() && !lines[j].starts_with("- id: ") {
                            j += 1;
                        }
                        let block = lines[start..j].join("\n");
                        if block.contains("QWITCH_GATEWAY_KEY") {
                            i = j;
                            continue;
                        }
                    }
                    out.push(lines[i].to_string());
                    i += 1;
                }
                let mut next = out.join("\n");
                if !next.is_empty() && !next.ends_with('\n') {
                    next.push('\n');
                }
                write_private_file_atomic(p, next.as_bytes()).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

fn remove_qwitch_from_codex(paths: &AgentPaths) -> Result<(), String> {
    if let Some(p) = paths.files.first() {
        if let Ok(raw) = std::fs::read_to_string(p) {
            let stripped = strip_toml_section(&raw, "[model_providers.qwitch]");
            let kept: Vec<&str> = stripped
                .lines()
                .filter(|l| {
                    let t = l.trim_start();
                    !(t.starts_with("model_provider = \"qwitch\"")
                        || (t.starts_with("model_catalog_json") && t.contains("qwitch_models")))
                })
                .collect();
            let mut next = kept.join("\n");
            if !next.is_empty() && !next.ends_with('\n') {
                next.push('\n');
            }
            write_private_file_atomic(p, next.as_bytes()).map_err(|e| e.to_string())?;
        }
    }
    if let Some(catalog) = paths.files.get(1) {
        let _ = std::fs::remove_file(catalog);
    }
    Ok(())
}

fn remove_qwitch_from_claude(paths: &AgentPaths) -> Result<(), String> {
    // 删除 Qwitch 写入的 configLibrary 与 _meta 条目
    for idx in [1usize, 2usize, 4usize, 5usize] {
        if let Some(p) = paths.files.get(idx) {
            let _ = std::fs::remove_file(p);
        }
    }
    // 主配置还原为 cloud 模式（若存在）
    for idx in [0usize, 3usize, 6usize] {
        if let Some(p) = paths.files.get(idx) {
            let _ = set_deployment_mode(p, "cloud");
        }
    }
    Ok(())
}

/// 强制删除某个 Agent 配置里所有 Qwitch 写入（不依赖 stash，保留用户其它配置）。
fn remove_qwitch_writes(
    agent_id: TargetAgentId,
    paths: &AgentPaths,
    _options: &AgentBindingOptions,
) -> Result<(), String> {
    match agent_id {
        TargetAgentId::Opencode | TargetAgentId::Mimo => {
            remove_qwitch_from_opencode_like(&paths.files[0])
        }
        TargetAgentId::Zcode => {
            let rules = paths
                .files
                .get(1)
                .cloned()
                .unwrap_or_else(|| paths.files[0].with_file_name("provider_config.json"));
            remove_qwitch_from_zcode(&paths.files[0], &rules)
        }
        TargetAgentId::Dsh => remove_qwitch_from_dsh(paths),
        TargetAgentId::CodexDesktop => remove_qwitch_from_codex(paths),
        TargetAgentId::ClaudeDesktop => remove_qwitch_from_claude(paths),
    }
}

/// 清除单个 Agent 的 Qwitch 配置。
/// - `mode == "restore"`：有 stash 则精确还原，否则回退为强制清除；
/// - `mode == "force"`：忽略 stash，仅删除 Qwitch 写入。
#[allow(clippy::too_many_arguments)]
pub fn clear_agent_wiring(
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    agent_id: TargetAgentId,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
    mode: &str,
) -> Result<AgentBinding, String> {
    let stashes = db
        .get_agent_stashes(master_key, agent_id)
        .map_err(|e| e.to_string())?;
    if mode != "force" && !stashes.is_empty() {
        return unbind_and_restore_agent(
            db,
            master_key,
            agent_id,
            all_pools,
            host,
            port,
            global_proxy,
            base_dir_override,
        );
    }
    let existing_opts = db
        .get_stored_agent_bindings()
        .ok()
        .and_then(|list| list.into_iter().find(|b| b.agent_id == agent_id))
        .map(|b| b.options)
        .unwrap_or_else(|| default_options_for_agent(agent_id));
    let paths = resolve_agent_paths(agent_id, &existing_opts, base_dir_override);
    remove_qwitch_writes(agent_id, &paths, &existing_opts)?;
    let _ = db.delete_agent_stashes(agent_id);
    let mut existing_opts = existing_opts;
    existing_opts.pending_restart_pids = None;
    let stored = StoredAgentBinding {
        agent_id,
        mounted_pool_ids: Vec::new(),
        default_model_key: None,
        options: existing_opts,
        status: AgentBindingStatus::Unbound,
        last_hit_at: None,
    };
    db.upsert_agent_binding(&stored)
        .map_err(|e| e.to_string())?;
    let _ = db.clear_agent_dedicated_pool_models(agent_id);
    Ok(build_agent_binding_dto(
        &stored,
        all_pools,
        host,
        port,
        global_proxy,
        base_dir_override,
        None,
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn unbind_and_restore_agent(
    db: &QwitchDatabase,
    master_key: &[u8; 32],
    agent_id: TargetAgentId,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
) -> Result<AgentBinding, String> {
    let stashes = db
        .get_agent_stashes(master_key, agent_id)
        .map_err(|e| e.to_string())?;

    for item in &stashes {
        let p = PathBuf::from(&item.file_path);
        if agent_id == TargetAgentId::CodexDesktop
            && p.file_name().and_then(|s| s.to_str()) == Some("config.toml")
        {
            // Codex 解绑保留 [model_providers.qwitch] 表，防止加载历史线程失败
            if let Ok(curr) = std::fs::read_to_string(&p) {
                let mut restored = String::new();
                if let Some(orig_bytes) = &item.original_content {
                    let orig_str = String::from_utf8_lossy(orig_bytes);
                    restored = orig_str.to_string();
                } else {
                    for line in curr.lines() {
                        let t = line.trim();
                        if !t.starts_with("model_provider = \"qwitch\"")
                            && !t.starts_with("model_catalog_json =")
                            && !t.starts_with("model_catalog_json=")
                        {
                            restored.push_str(line);
                            restored.push('\n');
                        }
                    }
                }
                if !restored.contains("[model_providers.qwitch]") {
                    if let Some(block) =
                        extract_toml_section_block(&curr, "[model_providers.qwitch]")
                    {
                        if !restored.is_empty() && !restored.ends_with('\n') {
                            restored.push('\n');
                        }
                        restored.push('\n');
                        restored.push_str(&block);
                    }
                }
                write_private_file_atomic(&p, restored.as_bytes()).map_err(|e| e.to_string())?;
                continue;
            }
        }

        if item.original_existed {
            if let Some(bytes) = &item.original_content {
                write_private_file_atomic(&p, bytes).map_err(|e| e.to_string())?;
            }
        } else {
            let _ = std::fs::remove_file(&p);
        }
    }

    let _ = db.delete_agent_stashes(agent_id);

    let existing_opts = db
        .get_stored_agent_bindings()
        .ok()
        .and_then(|list| list.into_iter().find(|b| b.agent_id == agent_id))
        .map(|b| b.options)
        .unwrap_or_else(|| default_options_for_agent(agent_id));

    let mut existing_opts = existing_opts;
    existing_opts.pending_restart_pids = None;
    let stored = StoredAgentBinding {
        agent_id,
        mounted_pool_ids: Vec::new(),
        default_model_key: None,
        options: existing_opts,
        status: AgentBindingStatus::Unbound,
        last_hit_at: None,
    };
    db.upsert_agent_binding(&stored)
        .map_err(|e| e.to_string())?;
    let _ = db.clear_agent_dedicated_pool_models(agent_id);

    Ok(build_agent_binding_dto(
        &stored,
        all_pools,
        host,
        port,
        global_proxy,
        base_dir_override,
        None,
    ))
}

#[cfg(target_os = "macos")]
fn open_cmd() -> Command {
    // 关键：dev/宿主进程可能带有 ELECTRON_RUN_AS_NODE=1，`open` 会把它传给被拉起的
    // Electron 应用（ZCode/MiMo/Codex 等），使其以 Node 模式启动后立刻退出。这里显式清除。
    let mut c = Command::new("open");
    c.env_remove("ELECTRON_RUN_AS_NODE");
    c
}

#[cfg(target_os = "macos")]
fn restart_codex_desktop_app() {
    // 真正退出并重新打开 GPT/Codex 桌面应用，确保它加载最新的 ~/.codex/config.toml
    // （仅 pkill app-server 会被桌面 supervisor 立即重新拉起，且 open -a 对已运行应用只是
    // 激活、不会重载配置，故必须“退出应用→重新打开”。）
    for name in ["ChatGPT", "Codex"] {
        let running = Command::new("pgrep")
            .args(["-x", name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !running {
            continue;
        }
        let _ = Command::new("osascript")
            .args(["-e", &format!("tell application \"{name}\" to quit")])
            .output();
        for _ in 0..24 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let still = Command::new("pgrep")
                .args(["-x", name])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !still {
                break;
            }
        }
        let still = Command::new("pgrep")
            .args(["-x", name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if still {
            let _ = Command::new("pkill").args(["-x", name]).output();
        }
    }
    for app in ["/Applications/ChatGPT.app", "/Applications/Codex.app"] {
        if std::path::Path::new(app).exists() {
            let _ = open_cmd().arg(app).spawn();
            break;
        }
    }
}

#[cfg(target_os = "macos")]
fn pgrep_exact(name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn restart_claude_desktop_app(isolated: bool) {
    // Claude Desktop 的退出通常需要数秒（Electron 多进程）；必须等旧进程真正退出再 open，
    // 否则 open 只会激活旧实例，看起来“没重启”。这里“优雅退出 → 轮询等待 → 强制兜底 → 重新拉起”。
    let app_path = "/Applications/Claude.app";
    let pattern = "/Applications/Claude.app/";
    let running = proc_matches(pattern) || pgrep_exact("Claude");
    if running {
        let _ = Command::new("osascript")
            .args([
                "-e",
                "tell application id \"com.anthropic.claudefordesktop\" to quit",
            ])
            .output();
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            if !proc_matches(pattern) && !pgrep_exact("Claude") {
                break;
            }
        }
        if proc_matches(pattern) || pgrep_exact("Claude") {
            let _ = Command::new("pkill").args(["-x", "Claude"]).output();
            let _ = Command::new("pkill").args(["-f", pattern]).output();
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        if proc_matches(pattern) || pgrep_exact("Claude") {
            let _ = Command::new("pkill").args(["-9", "-x", "Claude"]).output();
            let _ = Command::new("pkill").args(["-9", "-f", pattern]).output();
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }
    if isolated {
        let data_dir = home_dir().join("Library/Application Support/Claude-Qwitch-3p");
        let arg = format!("--user-data-dir={}", data_dir.display());
        let status = open_cmd()
            .args(["-n", "-a", app_path, "--args", &arg])
            .status();
        if !matches!(status, Ok(s) if s.success()) {
            let _ = open_cmd().args(["-n", "-a", app_path]).spawn();
        }
    } else {
        let opened = open_cmd()
            .args(["-n", "-b", "com.anthropic.claudefordesktop"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !opened {
            let _ = open_cmd().args(["-n", "-a", app_path]).spawn();
        }
    }
}

#[cfg(target_os = "macos")]
fn restart_macos_app(app_name: &str, bundle_id: &str, app_path: &str) {
    let running = proc_matches(app_path);
    if running {
        // 优雅退出（若在运行）：osascript quit → 等待 → 杀掉该 bundle 的残留辅助进程
        let _ = Command::new("osascript")
            .args(["-e", &format!("tell application \"{app_name}\" to quit")])
            .output();
        for _ in 0..24 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if !proc_matches(app_path) {
                break;
            }
        }
        let _ = Command::new("pkill").args(["-f", app_path]).output();
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    // 优先按 bundle id 打开（对已退出/未运行都可靠），失败再用 .app 路径
    let opened = open_cmd()
        .args(["-b", bundle_id])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !opened {
        let _ = open_cmd().arg(app_path).spawn();
    }
}

/// 查询 Windows 进程的可执行文件路径。
///
/// `tasklist` 只给进程名，不给路径；要"重启"就必须先拿到原 exe 路径，否则无法拉起。
/// 仅在用户点「重启客户端」这类低频操作时调用，单次约 300~800ms，可接受。
#[cfg(target_os = "windows")]
fn windows_process_exe_path(exe_name: &str) -> Option<PathBuf> {
    // Get-Process 的 -Name 不带 .exe 后缀
    let stem = exe_name.strip_suffix(".exe").unwrap_or(exe_name);
    // exe 名来自内置常量表（running_exe_name），不含引号，可安全内插；
    // 仍用单引号包裹，避免名称中的空格/括号破坏 PowerShell 语法。
    let script = format!(
        "Get-Process -Name '{stem}' -ErrorAction SilentlyContinue | \
         Where-Object {{ $_.Path }} | Select-Object -First 1 -ExpandProperty Path"
    );
    // 优先用绝对路径的 powershell.exe：GUI 进程的 PATH 未必包含 System32
    let ps = std::env::var_os("SystemRoot")
        .map(|root| {
            PathBuf::from(root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("powershell"));
    let mut cmd = Command::new(ps);
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    crate::platform::hide_console_std(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

/// 结束指定进程名的全部实例：先优雅关闭，等待退出，超时后强制结束。
#[cfg(target_os = "windows")]
fn terminate_windows_process(exe_name: &str) {
    let mut graceful = Command::new("taskkill");
    graceful.args(["/IM", exe_name]);
    crate::platform::hide_console_std(&mut graceful);
    // 进程不存在时 taskkill 返回非 0，属正常情况，无需区分
    let _ = graceful.output();
    for _ in 0..24 {
        if !contains_ignore_ascii_case(&process_snapshot(), exe_name) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let mut force = Command::new("taskkill");
    force.args(["/F", "/T", "/IM", exe_name]);
    crate::platform::hide_console_std(&mut force);
    let _ = force.output();
    std::thread::sleep(std::time::Duration::from_millis(300));
}

/// Windows 下重启桌面客户端：取原 exe 路径 → 结束旧实例 → 用同一路径重新拉起。
///
/// 与 macOS 的差异：Windows 没有 `open -a <bundle-id>` 这种按应用标识启动的机制，
/// 只能依赖进程自身的可执行文件路径；因此**仅在客户端正在运行时**才能完成重启。
/// 若取不到路径或拉起失败，返回 `Err` 以免前端误以为已重启生效。
#[cfg(target_os = "windows")]
fn restart_windows_app(agent_id: TargetAgentId, claude_isolated: bool) -> Result<(), String> {
    let snap = process_snapshot();
    let Some(exe_name) = running_exe_names(agent_id).iter().copied().find(|exe| {
        snap.lines().any(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with('"') {
                trimmed
                    .split(',')
                    .next()
                    .map(|col| col.trim_matches('"').eq_ignore_ascii_case(exe))
                    .unwrap_or(false)
            } else {
                contains_ignore_ascii_case(trimmed, exe)
            }
        })
    }) else {
        return Ok(());
    };
    let exe_path = windows_process_exe_path(exe_name).filter(|p| p.is_file());
    terminate_windows_process(exe_name);
    let Some(path) = exe_path else {
        return Err(format!(
            "已关闭 {exe_name}，但未能定位其可执行文件路径，请手动重新打开客户端"
        ));
    };
    let mut cmd = Command::new(path);
    cmd.env_remove("ELECTRON_RUN_AS_NODE")
        .env_remove("NODE_OPTIONS");
    if agent_id == TargetAgentId::ClaudeDesktop && claude_isolated {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let isolated_dir = PathBuf::from(local_app_data).join("Claude-Qwitch-3p");
            cmd.arg(format!("--user-data-dir={}", isolated_dir.display()));
        }
    }
    cmd.spawn()
        .map_err(|e| format!("重新启动 {exe_name} 失败: {e}"))?;
    Ok(())
}

pub fn restart_agent_process(
    db: &QwitchDatabase,
    agent_id: TargetAgentId,
    all_pools: &[AccountPool],
    host: &str,
    port: u16,
    global_proxy: bool,
    base_dir_override: Option<&Path>,
) -> Result<AgentBinding, String> {
    let stored_list = db.get_stored_agent_bindings().map_err(|e| e.to_string())?;
    let mut stored = stored_list
        .into_iter()
        .find(|b| b.agent_id == agent_id)
        .unwrap_or_else(|| StoredAgentBinding {
            agent_id,
            mounted_pool_ids: Vec::new(),
            default_model_key: None,
            options: default_options_for_agent(agent_id),
            status: AgentBindingStatus::Unbound,
            last_hit_at: None,
        });

    if base_dir_override.is_none() {
        #[cfg(target_os = "macos")]
        match agent_id {
            TargetAgentId::CodexDesktop => {
                // 先杀掉后台 app-server，再真正重启 GPT/Codex 桌面应用（退出→重开）
                let _ = Command::new("pkill")
                    .args(["-f", "codex.*app-server"])
                    .output();
                restart_codex_desktop_app();
            }
            TargetAgentId::ClaudeDesktop => {
                let isolated = stored.options.claude_isolated_twin.unwrap_or(false);
                restart_claude_desktop_app(isolated);
            }
            TargetAgentId::Dsh => {
                restart_macos_app(
                    "DeepSeek Harness",
                    "com.deepseek.dsh",
                    "/Applications/DeepSeek Harness.app/",
                );
            }
            TargetAgentId::Mimo => {
                restart_macos_app(
                    "Xiaomi MiMo",
                    "com.xiaomi.mimo.desktop",
                    "/Applications/Xiaomi MiMo.app/",
                );
            }
            TargetAgentId::Zcode => {
                restart_macos_app("ZCode", "dev.zcode.app", "/Applications/ZCode.app/");
            }
            TargetAgentId::Opencode => {}
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        if agent_id == TargetAgentId::CodexDesktop {
            let _ = Command::new("pkill")
                .args(["-f", "codex.*app-server"])
                .output();
        }

        // Windows：此前完全没有分支，导致「重启客户端」只重写了配置文件、进程没重启，
        // UI 却返回成功 —— 用户误以为新端口/新号池已生效。
        #[cfg(target_os = "windows")]
        {
            let isolated = stored.options.claude_isolated_twin.unwrap_or(false);
            restart_windows_app(agent_id, isolated)?;
        }
    }

    stored.options.pending_restart_pids = None;
    stored.status = if stored.mounted_pool_ids.is_empty() {
        AgentBindingStatus::Unbound
    } else {
        AgentBindingStatus::Bound
    };
    db.upsert_agent_binding(&stored)
        .map_err(|e| e.to_string())?;

    Ok(build_agent_binding_dto(
        &stored,
        all_pools,
        host,
        port,
        global_proxy,
        base_dir_override,
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AffinityMode, CreatePoolInput, CreatePoolMemberInput, CredentialEnvelope,
        CredentialPayload, MemberTier, PoolStrategy,
    };
    use std::collections::HashMap;
    use uuid::Uuid;

    #[test]
    fn pending_restart_satisfied_on_exit_or_pid_rotation() {
        assert!(is_pending_restart_satisfied(Some(&[101, 102]), &[]));
        assert!(!is_pending_restart_satisfied(
            Some(&[101, 102]),
            &[101, 205]
        ));
        assert!(is_pending_restart_satisfied(Some(&[101, 102]), &[301, 302]));
        assert!(!is_pending_restart_satisfied(None, &[301]));
        assert!(is_pending_restart_satisfied(None, &[]));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn agent_pids_parsed_from_macos_snapshot() {
        let snap = "\
1234 /Applications/Codex.app/Contents/MacOS/Codex
5678 /Applications/Claude.app/Contents/MacOS/Claude
9999 /usr/bin/python3
";
        assert_eq!(
            agent_pids_in_snapshot(TargetAgentId::CodexDesktop, snap),
            vec![1234]
        );
        assert_eq!(
            agent_pids_in_snapshot(TargetAgentId::ClaudeDesktop, snap),
            vec![5678]
        );
        assert!(agent_pids_in_snapshot(TargetAgentId::Zcode, snap).is_empty());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn agent_pids_parsed_from_windows_csv_snapshot() {
        let snap = "\"Codex.exe\",\"4321\",\"Console\",\"1\",\"100,000 K\"\r\n\"zcode.exe\",\"8765\",\"Console\",\"1\",\"80,000 K\"\r\n";
        assert_eq!(
            agent_pids_in_snapshot(TargetAgentId::CodexDesktop, snap),
            vec![4321]
        );
        assert_eq!(
            agent_pids_in_snapshot(TargetAgentId::Zcode, snap),
            vec![8765]
        );
        assert!(agent_pids_in_snapshot(TargetAgentId::ClaudeDesktop, snap).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn process_snapshot_matches_only_running_agents_on_macos() {
        // 回归：历史实现按 Agent 逐个 pgrep；改为单次快照后必须逐 Agent 仍然判准，
        // 且不能因为共享快照把只跑了一个客户端的机器判成「全在运行」。
        let snapshot =
            "501 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT\n502 /usr/bin/tail -f log";
        assert!(agent_running_in_snapshot(
            TargetAgentId::CodexDesktop,
            snapshot
        ));
        assert!(!agent_running_in_snapshot(
            TargetAgentId::ClaudeDesktop,
            snapshot
        ));
        assert!(!agent_running_in_snapshot(TargetAgentId::Dsh, snapshot));
        assert!(!agent_running_in_snapshot(
            TargetAgentId::Opencode,
            snapshot
        ));
        // 空快照（命令失败）必须一律判为未运行，不能误报「已就绪」
        assert!(!agent_running_in_snapshot(TargetAgentId::CodexDesktop, ""));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn process_snapshot_matches_only_running_agents_on_windows() {
        let snapshot = "\"ChatGPT.exe\",\"501\",\"Console\",\"1\",\"120,000 K\"\r\n";
        assert!(agent_running_in_snapshot(
            TargetAgentId::CodexDesktop,
            snapshot
        ));
        assert!(!agent_running_in_snapshot(
            TargetAgentId::ClaudeDesktop,
            snapshot
        ));
        assert!(!agent_running_in_snapshot(TargetAgentId::Zcode, ""));
    }

    /// 回归：tasklist 输出的进程名大小写取决于磁盘实际文件名（安装包可能落地小写），
    /// 大小写敏感比较会漏判「已运行」，导致重启流程被跳过。
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn process_snapshot_match_is_case_insensitive() {
        let snapshot = "\"zcode.exe\",\"7\",\"Console\",\"1\",\"90,000 K\"\r\n";
        assert!(agent_running_in_snapshot(TargetAgentId::Zcode, snapshot));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn contains_ignore_ascii_case_handles_edges() {
        assert!(contains_ignore_ascii_case("ChatGPT.exe", "chatgpt.exe"));
        assert!(contains_ignore_ascii_case("a ZCode.exe b", "zcode.exe"));
        // 更长的必需串不能被子串误判
        assert!(!contains_ignore_ascii_case("Chat", "ChatGPT"));
        assert!(!contains_ignore_ascii_case("anything", ""));
    }

    #[test]
    fn jsonc_lenient_parses_trailing_comma_and_preserves_fields() {
        let v = parse_jsonc_lenient(
            "{\n  // user config\n  \"mcpServers\": { \"a\": {} },\n  \"theme\": \"dark\",\n}",
        )
        .expect("parse lenient");
        assert_eq!(v["mcpServers"]["a"], serde_json::json!({}));
        assert_eq!(v["theme"], "dark");
    }

    #[test]
    fn jsonc_lenient_rejects_unparseable_config_instead_of_emptying() {
        assert!(parse_jsonc_lenient("{ this is not json ").is_err());
    }

    fn setup_wiring_env() -> (QwitchDatabase, [u8; 32], PathBuf, PathBuf, Vec<AccountPool>) {
        let root = std::env::temp_dir().join(format!("qwitch-wiring-{}", Uuid::new_v4()));
        let db = QwitchDatabase::open(&root.join("qwitch.db")).expect("open db");
        let home_dir = root.join("agent-home");
        std::fs::create_dir_all(home_dir.join("codex")).unwrap();
        std::fs::create_dir_all(home_dir.join("dsh")).unwrap();
        std::fs::write(home_dir.join("codex/config.toml"), "model = \"gpt-5.4\"\n").unwrap();
        std::fs::write(home_dir.join("dsh/.env"), "DEEPSEEK_API_KEY=sk-orig\n").unwrap();
        std::fs::write(
            home_dir.join("dsh/.credentials.yaml"),
            "refs:\n  DEEPSEEK_API_KEY: \"sk-orig\"\n",
        )
        .unwrap();

        let key = [5u8; 32];
        let acc = db
            .upsert_account(
                &key,
                Some("WB"),
                &CredentialEnvelope {
                    tool_id: "workbuddy".into(),
                    uid: "wb_1".into(),
                    display_name: Some("WB".into()),
                    display_email: None,
                    payload: CredentialPayload::Token {
                        access_token: "tok".into(),
                        refresh_token: None,
                        expires_at: None,
                        extra: HashMap::new(),
                    },
                },
            )
            .unwrap();
        let pool = db
            .create_pool(
                &key,
                &CreatePoolInput {
                    id: None,
                    name: "Coding池".into(),
                    description: "".into(),
                    scene_tag: "Coding".into(),
                    strategy: PoolStrategy::QuotaFirst,
                    affinity_mode: AffinityMode::AutoCache,
                    skip_active_client_account: false,
                    cooldown_seconds: 60,
                    auto_refresh_on_401: true,
                    fallback_api_key_enabled: false,
                    members: vec![CreatePoolMemberInput {
                        account_id: acc.id,
                        supported_models: vec!["deepseek-v4-pro".into()],
                        tier: MemberTier::T1Primary,
                    }],
                    enabled_model_ids: None,
                },
            )
            .unwrap();
        (db, key, root, home_dir, vec![pool])
    }

    #[test]
    fn dsh_wiring_detects_missing_credentials_drift_and_restores_cleanly() {
        let (db, key, root, home_dir, pools) = setup_wiring_env();
        let pid = pools[0].id.clone();

        let wired = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::Dsh,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions::default(),
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire dsh");
        assert_eq!(wired.status, AgentBindingStatus::Bound);
        assert!(
            std::fs::read_to_string(home_dir.join("dsh/.credentials.yaml"))
                .unwrap()
                .contains("QWITCH_GATEWAY_KEY")
        );

        // 模拟外部删掉 .credentials.yaml 中的 Qwitch key -> 必须报配置漂移
        std::fs::write(
            home_dir.join("dsh/.credentials.yaml"),
            "refs:\n  DEEPSEEK_API_KEY: \"sk-orig\"\n",
        )
        .unwrap();
        let inspected =
            inspect_all_agent_bindings(&db, &pools, "127.0.0.1", 18425, false, Some(&home_dir))
                .unwrap();
        let dsh = inspected
            .iter()
            .find(|b| b.agent_id == TargetAgentId::Dsh)
            .unwrap();
        assert_eq!(dsh.status, AgentBindingStatus::Drifted);

        // 还原后恢复原始文件内容
        let unbound = unbind_and_restore_agent(
            &db,
            &key,
            TargetAgentId::Dsh,
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("unbind");
        assert_eq!(unbound.status, AgentBindingStatus::Unbound);
        assert_eq!(
            std::fs::read_to_string(home_dir.join("dsh/.env")).unwrap(),
            "DEEPSEEK_API_KEY=sk-orig\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_unbind_restores_top_model_while_preserving_qwitch_provider_table() {
        let (db, key, root, home_dir, pools) = setup_wiring_env();
        let pid = pools[0].id.clone();

        let wired = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::CodexDesktop,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions {
                codex_preserve_table: Some(true),
                ..Default::default()
            },
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire codex");
        assert_eq!(wired.status, AgentBindingStatus::NeedsRestart);

        let wired_toml = std::fs::read_to_string(home_dir.join("codex/config.toml")).unwrap();
        assert!(wired_toml.contains("model_catalog_json = "));
        let catalog_json: Value = serde_json::from_str(
            &std::fs::read_to_string(home_dir.join("codex/qwitch_models.json")).unwrap(),
        )
        .unwrap();
        let first_model = &catalog_json["models"][0];
        assert_eq!(first_model["shell_type"], "shell_command");
        assert_eq!(first_model["visibility"], "list");
        assert!(first_model["base_instructions"].as_str().is_some());
        assert!(first_model["supported_reasoning_levels"].is_array());
        assert!(first_model["truncation_policy"].is_object());

        unbind_and_restore_agent(
            &db,
            &key,
            TargetAgentId::CodexDesktop,
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("unbind codex");

        let restored = std::fs::read_to_string(home_dir.join("codex/config.toml")).unwrap();
        assert!(restored.contains("model = \"gpt-5.4\""));
        assert!(!restored.contains("model_provider = \"qwitch\""));
        assert!(!restored.contains("model_catalog_json ="));
        assert!(restored.contains("[model_providers.qwitch]"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn opencode_v2_writes_models_as_keyed_object_map() {
        let (db, key, root, home_dir, pools) = setup_wiring_env();
        let pid = pools[0].id.clone();
        std::fs::create_dir_all(home_dir.join("opencode")).unwrap();

        let wired = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::Opencode,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions {
                opencode_schema_version: Some("v2".into()),
                ..Default::default()
            },
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire opencode");
        assert_eq!(wired.status, AgentBindingStatus::Bound);

        let cfg: Value = serde_json::from_str(
            &std::fs::read_to_string(home_dir.join("opencode/opencode.jsonc")).unwrap(),
        )
        .unwrap();
        let models = &cfg["providers"]["qwitch"]["models"];
        assert!(
            models.is_object(),
            "OpenCode v2 models must be an object map, got: {models}"
        );
        let expected_key = format!("{}/deepseek-v4-pro", pools[0].slug);
        let entry = models.get(&expected_key).expect("model key");
        assert_eq!(entry["capabilities"]["tools"], true);
        assert_eq!(entry["limit"]["context"], 200000);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dsh_wiring_repairs_inline_empty_refs_and_preserves_ui_patches() {
        let repaired_existing = upsert_dsh_credentials_yaml(
            "version: 1\nrefs: {}\n  QWITCH_GATEWAY_KEY: \"old-token\"\n",
            "QWITCH_GATEWAY_KEY",
            "qwitch-bind-dsh",
        );
        assert!(
            !repaired_existing.contains("refs: {}"),
            "expected refs: {{}} to be repaired, got:\n{repaired_existing}"
        );
        assert!(repaired_existing.contains("refs:\n  QWITCH_GATEWAY_KEY: \"qwitch-bind-dsh\""));

        let (db, key, root, home_dir, pools) = setup_wiring_env();
        let pid = pools[0].id.clone();
        std::fs::create_dir_all(home_dir.join("dsh/profiles/desktop")).unwrap();
        std::fs::write(
            home_dir.join("dsh/.credentials.yaml"),
            "version: 1\nrecords:\n  deepseek/default/apiKey:\n    enc: \"abc\"\nrefs: {}\n",
        )
        .unwrap();
        std::fs::write(
            home_dir.join("dsh/profiles/desktop/cordis.patch.yml"),
            "- id: llm-pi-ai\n  config:\n    baseUrl: \"http://old\"\n- id: ui-settings-account\n  enabled: false\n- id: ui-chat\n  config:\n    showWelcomeTips: false\n",
        )
        .unwrap();

        let wired = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::Dsh,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions::default(),
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire dsh");
        assert_eq!(wired.status, AgentBindingStatus::Bound);

        let creds = std::fs::read_to_string(home_dir.join("dsh/.credentials.yaml")).unwrap();
        assert!(!creds.contains("refs: {}"));
        assert!(creds.contains(&format!(
            "refs:\n  QWITCH_GATEWAY_KEY: \"{}\"",
            agent_token(TargetAgentId::Dsh)
        )));
        assert!(creds.contains("deepseek/default/apiKey:"));

        let patch = std::fs::read_to_string(home_dir.join("dsh/profiles/desktop/cordis.patch.yml"))
            .unwrap();
        assert!(patch.contains("baseURL: \"http://127.0.0.1:18425/v1\""));
        assert!(patch.contains("api: openai-completions"));
        assert!(patch.contains("displayName: Qwitch"));
        assert!(!patch.contains("baseUrl:"));
        assert!(!patch.contains("agent-default-model:"));
        assert!(patch.contains("- id: ui-settings-account"));
        assert!(patch.contains("- id: ui-chat"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_wiring_writes_openai_auth_and_actor_header() {
        let (db, key, root, home_dir, pools) = setup_wiring_env();
        let pid = pools[0].id.clone();

        let wired = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::CodexDesktop,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions::default(),
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire codex");
        assert_eq!(wired.status, AgentBindingStatus::NeedsRestart);

        let toml = std::fs::read_to_string(home_dir.join("codex/config.toml")).unwrap();
        assert!(toml.contains("requires_openai_auth = true"));
        assert!(toml.contains("http_headers = { \"x-openai-actor-authorization\" = \"qwitch\" }"));
        assert!(toml.contains("wire_api = \"responses\""));
        assert!(toml.contains("base_url = \"http://127.0.0.1:18425/v1\""));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn claude_desktop_isolated_wiring_syncs_both_3p_and_twin_directories() {
        let (db, key, root, home_dir, pools) = setup_wiring_env();
        let pid = pools[0].id.clone();

        let wired = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::ClaudeDesktop,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions {
                claude_isolated_twin: Some(true),
                ..Default::default()
            },
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire claude");
        assert_eq!(wired.status, AgentBindingStatus::NeedsRestart);
        assert!(wired.drift_reason.is_none());

        for dir_name in ["Claude-3p", "Claude-Qwitch-3p"] {
            let desktop_cfg: Value = serde_json::from_str(
                &std::fs::read_to_string(
                    home_dir.join(dir_name).join("claude_desktop_config.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(desktop_cfg["deploymentMode"], "3p");

            let lib_dir = home_dir.join(dir_name).join("configLibrary");
            assert!(lib_dir.join("_meta.json").exists());
            let preset: Value = serde_json::from_str(
                &std::fs::read_to_string(lib_dir.join(format!("{CLAUDE_CONFIG_UUID}.json")))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(preset["inferenceProvider"], "gateway");
            assert_eq!(preset["inferenceGatewayBaseUrl"], "http://127.0.0.1:18425");
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn codex_wiring_preserves_builtin_models_from_cache_and_claude_defaults_to_3p() {
        assert_eq!(
            default_options_for_agent(TargetAgentId::ClaudeDesktop).claude_isolated_twin,
            Some(false)
        );
        let default_claude_paths = resolve_agent_paths(
            TargetAgentId::ClaudeDesktop,
            &default_options_for_agent(TargetAgentId::ClaudeDesktop),
            Some(Path::new("/tmp/qwitch-check")),
        );
        assert_eq!(
            default_claude_paths.primary_display,
            "~/Library/Application Support/Claude-3p/configLibrary/"
        );

        let (db, key, root, home_dir, pools) = setup_wiring_env();
        let pid = pools[0].id.clone();
        std::fs::create_dir_all(home_dir.join("codex")).unwrap();
        let builtin_cache = json!({
            "models": [
                {
                    "slug": "gpt-6-luna",
                    "display_name": "GPT-6-Luna",
                    "visibility": "list",
                    "priority": 2,
                    "supported_in_api": true
                },
                {
                    "slug": "codex-auto-review",
                    "display_name": "Codex Auto Review",
                    "visibility": "hide",
                    "priority": 5,
                    "supported_in_api": true
                }
            ]
        });
        std::fs::write(
            home_dir.join("codex/models_cache.json"),
            serde_json::to_string_pretty(&builtin_cache).unwrap(),
        )
        .unwrap();

        let wired = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::CodexDesktop,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions::default(),
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire codex with cache");
        assert_eq!(wired.status, AgentBindingStatus::NeedsRestart);

        let catalog_json: Value = serde_json::from_str(
            &std::fs::read_to_string(home_dir.join("codex/qwitch_models.json")).unwrap(),
        )
        .unwrap();
        let models = catalog_json["models"].as_array().unwrap();
        assert!(models.len() >= 3);
        assert_eq!(models[0]["slug"], "gpt-6-luna");
        assert_eq!(models[1]["slug"], "codex-auto-review");
        assert_eq!(models[2]["priority"], 2);
        assert!(models[2]["slug"]
            .as_str()
            .unwrap()
            .contains("deepseek-v4-pro"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn config_parse_failure_does_not_overwrite_user_file() {
        let dir = std::env::temp_dir().join(format!("qwitch-cfg-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude_desktop_config.json");
        let original = "{ /* not valid json */ broken ";
        std::fs::write(&path, original).unwrap();
        let res = write_json_deployment_mode_3p(&path);
        assert!(
            res.is_err(),
            "invalid config must be rejected, not overwritten"
        );
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, original, "user config must be preserved verbatim");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wires_mimo_and_zcode_providers() {
        let (db, key, root, home_dir, pools) = setup_wiring_env();
        let pid = pools[0].id.clone();
        std::fs::create_dir_all(home_dir.join("mimocode")).unwrap();
        std::fs::create_dir_all(home_dir.join("zcode/v2")).unwrap();

        let mimo = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::Mimo,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions::default(),
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire mimo");
        assert!(matches!(
            mimo.status,
            AgentBindingStatus::Bound | AgentBindingStatus::NeedsRestart
        ));
        let mimo_cfg = std::fs::read_to_string(home_dir.join("mimocode/mimocode.jsonc")).unwrap();
        assert!(mimo_cfg.contains("\"qwitch\""));
        assert!(mimo_cfg.contains("18425"));
        assert!(mimo_cfg.contains("@ai-sdk/openai-compatible"));

        let zcode = apply_agent_wiring(
            &db,
            &key,
            TargetAgentId::Zcode,
            std::slice::from_ref(&pid),
            None,
            &AgentBindingOptions::default(),
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
        )
        .expect("wire zcode");
        assert!(matches!(
            zcode.status,
            AgentBindingStatus::Bound | AgentBindingStatus::NeedsRestart
        ));
        let zc_cfg = std::fs::read_to_string(home_dir.join("zcode/v2/config.json")).unwrap();
        assert!(zc_cfg.contains("qwitch"));
        assert!(zc_cfg.contains("18425"));
        assert!(zc_cfg.contains("anthropic"));
        let zc_rules =
            std::fs::read_to_string(home_dir.join("zcode/v2/provider_config.json")).unwrap();
        assert!(zc_rules.contains("qwitch"));
        assert!(zc_rules.contains("18425"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn force_clear_removes_qwitch_but_keeps_other_providers() {
        let (db, key, root, home_dir, pools) = setup_wiring_env();
        std::fs::create_dir_all(home_dir.join("mimocode")).unwrap();
        std::fs::create_dir_all(home_dir.join("zcode/v2")).unwrap();
        std::fs::write(
            home_dir.join("mimocode/mimocode.jsonc"),
            r#"{"model":"qwitch/x","provider":{"qwitch":{"npm":"@ai-sdk/openai-compatible"},"other":{"npm":"keep"}}}"#,
        )
        .unwrap();
        std::fs::write(
            home_dir.join("zcode/v2/config.json"),
            r#"{"provider":{"qwitch":{"kind":"anthropic"},"other":{"kind":"anthropic"}}}"#,
        )
        .unwrap();
        std::fs::write(
            home_dir.join("zcode/v2/provider_config.json"),
            r#"{"schemaVersion":1,"config":{"providerConfigRules":{"providerRules":[{"providerId":"qwitch"},{"providerId":"other"}]},"modelConfigRules":{"providerModelRules":[{"providerId":"qwitch","modelId":"m"}],"manualProviderModelRules":[]}}}"#,
        )
        .unwrap();

        clear_agent_wiring(
            &db,
            &key,
            TargetAgentId::Mimo,
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
            "force",
        )
        .expect("clear mimo");
        let mimo = std::fs::read_to_string(home_dir.join("mimocode/mimocode.jsonc")).unwrap();
        assert!(!mimo.contains("qwitch"));
        assert!(mimo.contains("other"));

        clear_agent_wiring(
            &db,
            &key,
            TargetAgentId::Zcode,
            &pools,
            "127.0.0.1",
            18425,
            false,
            Some(&home_dir),
            "force",
        )
        .expect("clear zcode");
        let zc = std::fs::read_to_string(home_dir.join("zcode/v2/config.json")).unwrap();
        assert!(!zc.contains("qwitch"));
        assert!(zc.contains("other"));
        let zr = std::fs::read_to_string(home_dir.join("zcode/v2/provider_config.json")).unwrap();
        assert!(!zr.contains("qwitch"));
        assert!(zr.contains("other"));

        let _ = std::fs::remove_dir_all(root);
    }
}
