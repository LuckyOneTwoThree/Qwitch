use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::adapter::write_private_file_atomic;
use crate::domain::{CredentialEnvelope, PoolMember};
use crate::gateway::ir::IrRequest;
use crate::plugins::egress::{EgressTransport, StandardHttpEgress};
use crate::plugins::ingress::IngressDetectInfo;
use crate::plugins::manifest::{DeclarativeWiringSpec, PluginManifest, PluginTransport};

pub struct DeclarativeEgressPlugin {
    inner: StandardHttpEgress,
}

impl DeclarativeEgressPlugin {
    pub fn from_manifest(manifest: &PluginManifest) -> Result<Self, String> {
        if manifest.transport != PluginTransport::Declarative {
            return Err(format!("插件 {} transport 不是 declarative", manifest.id));
        }
        let profile = manifest
            .egress_profile
            .as_ref()
            .ok_or_else(|| format!("声明式出口插件 {} 缺少 egressProfile", manifest.id))?;
        let http_spec = profile
            .declarative_http
            .as_ref()
            .ok_or_else(|| format!("声明式出口插件 {} 缺少 declarativeHttp", manifest.id))?;
        let headers: Vec<(String, String)> = http_spec
            .extra_headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let inner = StandardHttpEgress::new(manifest.id.clone(), http_spec.base_url.clone())
            .with_headers(headers);
        Ok(Self { inner })
    }
}

#[async_trait]
impl EgressTransport for DeclarativeEgressPlugin {
    fn id(&self) -> &str {
        self.inner.id()
    }

    async fn send_stream(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<reqwest::Response, String> {
        self.inner
            .send_stream(member, cred, ir_req, upstream_model)
            .await
    }

    async fn call_buffered(
        &self,
        member: &PoolMember,
        cred: &CredentialEnvelope,
        ir_req: &IrRequest,
        upstream_model: &str,
    ) -> Result<(u16, String), String> {
        self.inner
            .call_buffered(member, cred, ir_req, upstream_model)
            .await
    }
}

#[derive(Debug, Clone)]
pub struct TemplateContext {
    pub base_url: String,
    pub port: u16,
    pub gateway_token: String,
    pub default_model: String,
    pub models_map: Value,
}

pub struct DeclarativeIngressPlugin {
    id: String,
    name: String,
    spec: DeclarativeWiringSpec,
}

impl DeclarativeIngressPlugin {
    pub fn from_manifest(manifest: &PluginManifest) -> Result<Self, String> {
        if manifest.transport != PluginTransport::Declarative {
            return Err(format!("插件 {} transport 不是 declarative", manifest.id));
        }
        let profile = manifest
            .ingress_profile
            .as_ref()
            .ok_or_else(|| format!("声明式入口插件 {} 缺少 ingressProfile", manifest.id))?;
        let spec = profile
            .declarative_wiring
            .clone()
            .ok_or_else(|| format!("声明式入口插件 {} 缺少 declarativeWiring", manifest.id))?;
        Ok(Self {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            spec,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn resolve_path(raw: &str, base_dir_override: Option<&Path>) -> PathBuf {
        let normalized = raw.replace('\\', "/");
        if let Some(base) = base_dir_override {
            let rel = normalized
                .trim_start_matches("~/")
                .trim_start_matches("%USERPROFILE%/")
                .trim_start_matches("%APPDATA%/")
                .trim_start_matches("%LOCALAPPDATA%/");
            return base.join(rel);
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        if let Some(rest) = normalized.strip_prefix("~/") {
            return home.join(rest);
        }
        if let Some(rest) = normalized.strip_prefix("%USERPROFILE%/") {
            return home.join(rest);
        }
        if let Some(rest) = normalized.strip_prefix("%APPDATA%/") {
            let appdata = std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData/Roaming"));
            return appdata.join(rest);
        }
        if let Some(rest) = normalized.strip_prefix("%LOCALAPPDATA%/") {
            let local = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData/Local"));
            return local.join(rest);
        }
        PathBuf::from(raw)
    }

    fn configured_paths(&self, base_dir_override: Option<&Path>) -> Vec<PathBuf> {
        #[cfg(target_os = "windows")]
        let raw_paths = &self.spec.detect.windows_config_paths;
        #[cfg(not(target_os = "windows"))]
        let raw_paths = &self.spec.detect.macos_config_paths;

        raw_paths
            .iter()
            .map(|p| Self::resolve_path(p, base_dir_override))
            .collect()
    }

    pub fn detect(&self, base_dir_override: Option<&Path>) -> IngressDetectInfo {
        let files = self.configured_paths(base_dir_override);
        let installed = base_dir_override.is_some()
            || files
                .iter()
                .any(|p| p.exists() || p.parent().is_some_and(|dir| dir.exists()));
        let primary_display = files
            .first()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        IngressDetectInfo {
            primary_display,
            files,
            installed,
            version: "declarative".into(),
        }
    }

    pub fn apply_patch_to_disk(
        &self,
        ctx: &TemplateContext,
        base_dir_override: Option<&Path>,
    ) -> Result<PathBuf, String> {
        let files = self.configured_paths(base_dir_override);
        let idx = self.spec.patch.target_file_index;
        let target = files
            .get(idx)
            .ok_or_else(|| format!("声明式入口 {} 缺少目标配置文件索引 {idx}", self.id))?
            .clone();

        let format_lower = self.spec.patch.format.trim().to_ascii_lowercase();
        if !matches!(format_lower.as_str(), "json" | "jsonc") {
            return Err(format!(
                "声明式入口 {} 暂不支持格式 {}（目前支持 json / jsonc）",
                self.id, self.spec.patch.format
            ));
        }

        let mut doc = if target.exists() {
            let raw = std::fs::read_to_string(&target)
                .map_err(|e| format!("读取配置 {} 失败: {e}", target.display()))?;
            let stripped = strip_jsonc_comments(&raw);
            if stripped.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str::<Value>(&stripped)
                    .map_err(|e| format!("解析 JSON/JSONC 配置 {} 失败: {e}", target.display()))?
            }
        } else {
            json!({})
        };

        for (dotted_key, tpl_val) in &self.spec.patch.set_keys {
            let rendered = render_template_value(tpl_val, ctx);
            apply_dotted_json_patch(&mut doc, dotted_key, rendered);
        }

        let serialized =
            serde_json::to_string_pretty(&doc).map_err(|e| format!("序列化配置失败: {e}"))?;
        write_private_file_atomic(&target, serialized.as_bytes()).map_err(|e| e.to_string())?;
        Ok(target)
    }

    pub fn check_drift(&self, port: u16, base_dir_override: Option<&Path>) -> Option<String> {
        let files = self.configured_paths(base_dir_override);
        let target = files.get(self.spec.patch.target_file_index)?;
        let content = std::fs::read_to_string(target).unwrap_or_default();
        let port_str = port.to_string();
        for assertion in &self.spec.patch.drift_assert_contains {
            let expected = assertion.replace("{{port}}", &port_str);
            if !content.contains(&expected) {
                return Some(format!(
                    "检测到 {} 配置缺少 `{expected}`，建议重新同步写入",
                    self.name
                ));
            }
        }
        None
    }

    pub fn is_running_in_snapshot(&self, snapshot: &str) -> bool {
        #[cfg(target_os = "windows")]
        let patterns = &self.spec.detect.windows_exe_names;
        #[cfg(not(target_os = "windows"))]
        let patterns = &self.spec.detect.macos_process_patterns;

        let lower = snapshot.to_ascii_lowercase();
        patterns
            .iter()
            .any(|p| !p.is_empty() && lower.contains(&p.to_ascii_lowercase()))
    }
}

pub fn render_template_value(tpl: &Value, ctx: &TemplateContext) -> Value {
    match tpl {
        Value::String(s) => {
            if s.trim() == "{{modelsMap}}" {
                return ctx.models_map.clone();
            }
            let replaced = s
                .replace("{{baseUrl}}", &ctx.base_url)
                .replace("{{port}}", &ctx.port.to_string())
                .replace("{{gatewayToken}}", &ctx.gateway_token)
                .replace("{{defaultModel}}", &ctx.default_model);
            Value::String(replaced)
        }
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|item| render_template_value(item, ctx))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), render_template_value(v, ctx));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

pub fn strip_jsonc_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }

        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    for nc in chars.by_ref() {
                        if nc == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    while let Some(nc) = chars.next() {
                        if nc == '*' && chars.peek() == Some(&'/') {
                            chars.next();
                            break;
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }

        out.push(c);
    }

    out
}

pub fn apply_dotted_json_patch(doc: &mut Value, dotted_key: &str, value: Value) {
    let parts: Vec<&str> = dotted_key.split('.').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return;
    }
    if !doc.is_object() {
        *doc = json!({});
    }
    let mut cur = doc;
    for part in &parts[..parts.len() - 1] {
        let obj = cur.as_object_mut().expect("object");
        let entry = obj.entry((*part).to_string()).or_insert_with(|| json!({}));
        if !entry.is_object() {
            *entry = json!({});
        }
        cur = entry;
    }
    if let Some(obj) = cur.as_object_mut() {
        obj.insert(parts[parts.len() - 1].to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::manifest::{
        DeclarativeWiringDetect, DeclarativeWiringPatch, IngressProfile, PluginBrand, PluginKind,
    };
    use std::collections::HashMap;

    fn sample_declarative_ingress() -> PluginManifest {
        let mut set_keys = HashMap::new();
        set_keys.insert(
            "providers.qwitch.baseURL".to_string(),
            json!("{{baseUrl}}/v1"),
        );
        set_keys.insert(
            "providers.qwitch.apiKey".to_string(),
            json!("{{gatewayToken}}"),
        );
        set_keys.insert(
            "providers.qwitch.models".to_string(),
            json!("{{modelsMap}}"),
        );

        PluginManifest {
            schema_version: 2,
            id: "qoder_ide".into(),
            name: "Qoder IDE".into(),
            version: "1.0.0".into(),
            kind: PluginKind::Ingress,
            transport: PluginTransport::Declarative,
            platforms: Vec::new(),
            brand: PluginBrand::default(),
            entry: None,
            egress_profile: None,
            ingress_profile: Some(IngressProfile {
                declarative_wiring: Some(DeclarativeWiringSpec {
                    detect: DeclarativeWiringDetect {
                        macos_config_paths: vec!["~/.qoder/config.jsonc".into()],
                        windows_config_paths: vec!["%USERPROFILE%\\.qoder\\config.jsonc".into()],
                        macos_process_patterns: vec!["/Applications/Qoder.app/".into()],
                        windows_exe_names: vec!["Qoder.exe".into()],
                    },
                    patch: DeclarativeWiringPatch {
                        format: "jsonc".into(),
                        target_file_index: 0,
                        set_keys,
                        drift_assert_contains: vec!["qwitch".into(), ":{{port}}".into()],
                    },
                }),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn strips_jsonc_comments_preserving_urls_and_patches_nested_keys() {
        let raw = r#"{
            // comment with http://example.com
            "existing": "https://example.com/v1", /* block comment */
            "keep": true
        }"#;
        let stripped = strip_jsonc_comments(raw);
        let mut val: Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(val["existing"], "https://example.com/v1");
        apply_dotted_json_patch(
            &mut val,
            "providers.qwitch.baseURL",
            json!("http://127.0.0.1:8187/v1"),
        );
        assert_eq!(val["keep"], true);
        assert_eq!(
            val["providers"]["qwitch"]["baseURL"],
            "http://127.0.0.1:8187/v1"
        );
    }

    #[test]
    fn declarative_ingress_patches_checks_drift_and_matches_process() {
        let root =
            std::env::temp_dir().join(format!("qwitch_decl_ingress_{}", uuid::Uuid::new_v4()));
        let cfg_dir = root.join(".qoder");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("config.jsonc"),
            "// initial comment\n{\"theme\": \"dark\"}\n",
        )
        .unwrap();

        let plugin =
            DeclarativeIngressPlugin::from_manifest(&sample_declarative_ingress()).unwrap();
        let detect = plugin.detect(Some(&root));
        assert!(detect.installed);

        let ctx = TemplateContext {
            base_url: "http://127.0.0.1:8187".into(),
            port: 8187,
            gateway_token: "qwitch-bind-qoder-dev".into(),
            default_model: "gpt-5.4".into(),
            models_map: json!({ "gpt-5.4": { "name": "GPT-5.4" } }),
        };
        let written = plugin.apply_patch_to_disk(&ctx, Some(&root)).unwrap();
        let updated: Value =
            serde_json::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
        assert_eq!(updated["theme"], "dark");
        assert_eq!(
            updated["providers"]["qwitch"]["baseURL"],
            "http://127.0.0.1:8187/v1"
        );
        assert_eq!(
            updated["providers"]["qwitch"]["models"]["gpt-5.4"]["name"],
            "GPT-5.4"
        );
        assert!(plugin.check_drift(8187, Some(&root)).is_none());
        assert!(plugin.check_drift(9999, Some(&root)).is_some());

        #[cfg(target_os = "macos")]
        assert!(plugin.is_running_in_snapshot("123 /Applications/Qoder.app/Contents/MacOS/Qoder"));
        #[cfg(not(target_os = "macos"))]
        assert!(plugin.is_running_in_snapshot("Qoder.exe 1234"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
