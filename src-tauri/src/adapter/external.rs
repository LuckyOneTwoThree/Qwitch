use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use uuid::Uuid;

const MAX_ADAPTER_OUTPUT_BYTES: u64 = 1_048_576; // 1 MiB 上限
const MAX_ADAPTER_TIMEOUT_MS: u64 = 60_000; // 60s 上限
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_HASH_FILES: usize = 500;
const MAX_HASH_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_HASH_DEPTH: usize = 4;

const ALLOWED_PLUGIN_ENV_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "TMPDIR",
    "TEMP",
    "TMP",
    "LANG",
    "LC_ALL",
    "SystemRoot",
    "PATHEXT",
    "WINDIR",
    "COMSPEC",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];

use crate::adapter::{
    AdapterCapabilities, AdapterError, AdapterManifest, ClaimResult, ToolAdapter,
};
use crate::domain::{AuthMethod, CredentialEnvelope, QuotaSnapshot, UnifiedErrorCode};
use crate::platform::PlatformHost;

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalCapabilities {
    #[serde(default)]
    pub auth_methods: Vec<AuthMethod>,
    #[serde(default)]
    pub supports_quota_query: bool,
    #[serde(default)]
    pub supports_daily_claim: bool,
    #[serde(default)]
    pub supports_client_switch: bool,
    #[serde(default)]
    pub supports_multi_instance: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalManifestFile {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub platforms: Vec<String>,
    pub entry: ExternalEntry,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    pub capabilities: ExternalCapabilities,
}

fn default_timeout_ms() -> u64 {
    15_000
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    id: String,
    method: &'a str,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    id: Option<Value>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

pub struct ExternalAdapter {
    manifest: AdapterManifest,
    adapter_dir: PathBuf,
    entry: ExternalEntry,
    timeout_ms: u64,
    pinned_hash: Option<String>,
}

impl ExternalAdapter {
    pub fn from_manifest_file(adapter_dir: PathBuf, raw: ExternalManifestFile) -> Self {
        Self {
            manifest: AdapterManifest {
                id: raw.id,
                name: raw.name,
                version: raw.version,
                is_builtin: false,
                capabilities: AdapterCapabilities {
                    auth_methods: raw.capabilities.auth_methods,
                    supports_quota_query: raw.capabilities.supports_quota_query,
                    supports_daily_claim: raw.capabilities.supports_daily_claim,
                    supports_client_switch: raw.capabilities.supports_client_switch,
                    supports_multi_instance: raw.capabilities.supports_multi_instance,
                },
            },
            adapter_dir,
            entry: raw.entry,
            timeout_ms: raw.timeout_ms.clamp(1000, MAX_ADAPTER_TIMEOUT_MS),
            pinned_hash: None,
        }
    }

    pub fn with_pinned_hash(mut self, hash: String) -> Self {
        self.pinned_hash = Some(hash);
        self
    }

    pub(crate) fn current_platform_tag() -> &'static str {
        #[cfg(target_os = "macos")]
        {
            "macos"
        }
        #[cfg(target_os = "windows")]
        {
            "windows"
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            "linux"
        }
    }

    fn map_error_code(code: Option<&str>) -> UnifiedErrorCode {
        match code {
            Some("AUTH_EXPIRED") => UnifiedErrorCode::AuthExpired,
            Some("RISK_TRIGGERED") => UnifiedErrorCode::RiskTriggered,
            Some("RATE_LIMITED") => UnifiedErrorCode::RateLimited,
            Some("DEVICE_DAILY_LIMIT_REACHED") => UnifiedErrorCode::DeviceDailyLimitReached,
            Some("NETWORK_ERROR") => UnifiedErrorCode::NetworkError,
            Some("CLIENT_RUNNING") => UnifiedErrorCode::ClientRunning,
            Some("CLIENT_NOT_FOUND") => UnifiedErrorCode::ClientNotFound,
            Some("UNSUPPORTED_CAPABILITY") => UnifiedErrorCode::UnsupportedCapability,
            Some("INVALID_CREDENTIAL") => UnifiedErrorCode::InvalidCredential,
            _ => UnifiedErrorCode::InternalError,
        }
    }

    fn validate_envelope_tool_id(
        &self,
        env: CredentialEnvelope,
    ) -> Result<CredentialEnvelope, AdapterError> {
        if env.tool_id != self.manifest.id {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!(
                    "外部适配器 {} 返回的 tool_id ({}) 与插件 ID 不匹配，已拒绝导入",
                    self.manifest.id, env.tool_id
                ),
            });
        }
        Ok(env)
    }

    async fn call_rpc(&self, method: &str, params: Value) -> Result<Value, AdapterError> {
        if let Some(expected_hash) = &self.pinned_hash {
            if !is_valid_entry(&self.adapter_dir, &self.entry) {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!("外部适配器 {} 入口校验失败，已拒绝执行", self.manifest.id),
                });
            }
            let actual_hash =
                adapter_content_hash(&self.adapter_dir).map_err(|e| AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!("外部适配器 {} 完整性校验失败: {e}", self.manifest.id),
                })?;
            if !actual_hash.eq_ignore_ascii_case(expected_hash) {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!(
                        "外部适配器 {} 内容在启动后发生变更（哈希不一致），已拒绝执行",
                        self.manifest.id
                    ),
                });
            }
        }

        let req_id = Uuid::new_v4().to_string();
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: req_id.clone(),
            method,
            params,
        };
        let mut req_line = serde_json::to_vec(&req).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("序列化 JSON-RPC 请求失败: {e}"),
        })?;
        req_line.push(b'\n');

        let exec_cmd = resolve_exec_command(&self.adapter_dir, &self.entry.command);
        let mut cmd = Command::new(&exec_cmd);
        cmd.args(&self.entry.args)
            .current_dir(&self.adapter_dir)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for key in ALLOWED_PLUGIN_ENV_VARS {
            if let Some(val) = std::env::var_os(key) {
                cmd.env(key, val);
            }
        }
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x0800_0000);

        let mut child = cmd.spawn().map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("启动外部适配器 {} 失败: {e}", self.manifest.id),
        })?;

        let timeout = Duration::from_millis(self.timeout_ms);
        let mut stdin_pipe = child.stdin.take();
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let (stdout_bytes, stderr_bytes, exit_status) = match tokio::time::timeout(timeout, async {
            let write_stdin = async {
                if let Some(mut stdin) = stdin_pipe.take() {
                    let _ = stdin.write_all(&req_line).await;
                    let _ = stdin.shutdown().await;
                }
            };
            let read_stdout = async {
                let mut buf = Vec::new();
                if let Some(stdout) = stdout_pipe {
                    let mut reader = stdout.take(MAX_ADAPTER_OUTPUT_BYTES + 1);
                    reader.read_to_end(&mut buf).await?;
                }
                Ok::<Vec<u8>, std::io::Error>(buf)
            };
            let read_stderr = async {
                let mut buf = Vec::new();
                if let Some(mut stderr) = stderr_pipe {
                    let mut chunk = [0u8; 4096];
                    loop {
                        match stderr.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if buf.len() < 65_536 {
                                    let take = n.min(65_536 - buf.len());
                                    buf.extend_from_slice(&chunk[..take]);
                                }
                            }
                        }
                    }
                }
                buf
            };
            let (_, out_res, err_buf, wait_res) =
                tokio::join!(write_stdin, read_stdout, read_stderr, child.wait());
            let out_buf = out_res?;
            let status = wait_res?;
            Ok::<(Vec<u8>, Vec<u8>, std::process::ExitStatus), std::io::Error>((
                out_buf, err_buf, status,
            ))
        })
        .await
        {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => {
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::InternalError,
                    message: format!("外部适配器执行异常: {e}"),
                });
            }
            Err(_) => {
                let _ = child.start_kill();
                return Err(AdapterError::Unified {
                    code: UnifiedErrorCode::NetworkError,
                    message: format!(
                        "外部适配器 {} 执行超时 ({}ms)",
                        self.manifest.id, self.timeout_ms
                    ),
                });
            }
        };

        if stdout_bytes.len() as u64 > MAX_ADAPTER_OUTPUT_BYTES {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("外部适配器 {} 输出超过 1MB 上限，已中止", self.manifest.id),
            });
        }

        let stderr_snippet = || -> String {
            let text = String::from_utf8_lossy(&stderr_bytes);
            let trimmed = text.trim();
            if trimmed.is_empty() {
                String::new()
            } else {
                let sanitized = crate::storage::crypto::sanitize_log_message(trimmed);
                let clipped: String = sanitized.chars().take(200).collect();
                format!("（stderr: {clipped}）")
            }
        };

        let stdout_str = String::from_utf8_lossy(&stdout_bytes);
        let Some(last_line) = stdout_str.lines().rev().find(|l| !l.trim().is_empty()) else {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!(
                    "外部适配器 {} 未返回 JSON-RPC 响应 (exit: {}){}",
                    self.manifest.id,
                    exit_status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "signal".to_string()),
                    stderr_snippet()
                ),
            });
        };

        let resp: JsonRpcResponse =
            serde_json::from_str(last_line).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!("解析外部适配器响应失败: {e}{}", stderr_snippet()),
            })?;

        let matches = resp
            .id
            .as_ref()
            .and_then(|v| v.as_str())
            .is_some_and(|id| id == req_id);
        if !matches {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!(
                    "外部适配器 {} 返回的 JSON-RPC id 缺失或不匹配",
                    self.manifest.id
                ),
            });
        }

        if let Some(err) = resp.error {
            return Err(AdapterError::Unified {
                code: Self::map_error_code(err.code.as_deref()),
                message: err
                    .message
                    .unwrap_or_else(|| "外部适配器返回错误".to_string()),
            });
        }

        if !exit_status.success() && resp.result.is_none() {
            return Err(AdapterError::Unified {
                code: UnifiedErrorCode::InternalError,
                message: format!(
                    "外部适配器 {} 异常退出 (code: {:?}){}",
                    self.manifest.id,
                    exit_status.code(),
                    stderr_snippet()
                ),
            });
        }

        Ok(resp.result.unwrap_or(Value::Null))
    }
}

#[async_trait]
impl ToolAdapter for ExternalAdapter {
    fn manifest(&self) -> &AdapterManifest {
        &self.manifest
    }

    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let config_dir = host
            .client_config_dir(&self.manifest.id)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let res = self
            .call_rpc(
                "import_local",
                serde_json::json!({ "client_config_dir": config_dir }),
            )
            .await?;
        let env: CredentialEnvelope =
            serde_json::from_value(res).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("外部适配器返回凭据格式无效: {e}"),
            })?;
        self.validate_envelope_tool_id(env)
    }

    async fn parse_manual_token(
        &self,
        raw_input: &str,
    ) -> Result<CredentialEnvelope, AdapterError> {
        let res = self
            .call_rpc(
                "parse_manual_token",
                serde_json::json!({ "raw_input": raw_input }),
            )
            .await?;
        let env: CredentialEnvelope =
            serde_json::from_value(res).map_err(|e| AdapterError::Unified {
                code: UnifiedErrorCode::InvalidCredential,
                message: format!("外部适配器解析 Token 失败: {e}"),
            })?;
        self.validate_envelope_tool_id(env)
    }

    async fn inspect_quota(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<QuotaSnapshot, AdapterError> {
        let res = self
            .call_rpc("inspect_quota", serde_json::json!({ "credential": cred }))
            .await?;
        serde_json::from_value(res).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("外部适配器返回额度格式无效: {e}"),
        })
    }

    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
        precheck: Option<&QuotaSnapshot>,
    ) -> Result<ClaimResult, AdapterError> {
        let res = self
            .call_rpc(
                "claim_daily",
                serde_json::json!({ "credential": cred, "precheck": precheck }),
            )
            .await?;
        serde_json::from_value(res).map_err(|e| AdapterError::Unified {
            code: UnifiedErrorCode::InternalError,
            message: format!("外部适配器返回领取结果格式无效: {e}"),
        })
    }

    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError> {
        let config_dir = host
            .client_config_dir(&self.manifest.id)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        self.call_rpc(
            "apply_session",
            serde_json::json!({
                "credential": cred,
                "client_config_dir": config_dir
            }),
        )
        .await?;
        Ok(())
    }
}

/// 判断命令串是否「看起来是插件目录内的文件路径」而非裸命令名。
///
/// 裸命令名（`python3` / `node` / `ruby`）由 `PATH` 解析，按协议保留；
/// 但带脚本扩展名的（`adapter.py` / `run.js`）显然指向目录内文件，
/// 必须按路径校验，否则不存在的脚本名会被误当作 PATH 命令放行。
fn looks_like_adapter_file(command: &str) -> bool {
    const SCRIPT_EXTS: &[&str] = &[
        "py", "js", "mjs", "cjs", "ts", "sh", "bash", "zsh", "rb", "pl", "php", "lua", "jar",
        "exe", "bat", "cmd", "ps1",
    ];
    if command.contains('/') || command.contains('\\') || command.starts_with('.') {
        return true;
    }
    match command.rsplit_once('.') {
        Some((_, ext)) => SCRIPT_EXTS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// 校验 manifest 声明的入口命令。
///
/// 安全目标：入口命令指向的**实际执行体**必须受 `adapter_content_hash` 约束，
/// 否则「内容哈希固定」只保证清单目录未被改动，却拦不住把执行体指向目录外的
/// 任意二进制（如 `/bin/sh`），使信任门禁形同虚设。
///
/// 规则：
/// * 控制字符、父目录穿越 → 拒绝。
/// * **绝对路径** → 仅当规范化后仍位于 `adapter_dir` 内才允许（历史实现无条件放行，
///   可指向任意系统二进制，已收紧）。
/// * **文件形态的相对路径**（含分隔符、以 `.` 开头、或带脚本扩展名）→ 必须存在，
///   且规范化后仍位于 `adapter_dir` 内（历史实现「文件不存在也放行」为 fail-open，
///   已改为 fail-closed）。
/// * **裸命令名**（如 `python3`）→ 保留，这是 `docs/CORE_DESIGN.md` 3.1 公开的
///   插件协议（`entry.command` 可为解释器名）。残余风险：解析依赖继承的 `PATH`，
///   若用户 `PATH` 中存在同名恶意程序仍可能被选中；信任外部插件时应确认其入口
///   为自带脚本，或改用插件目录内的相对路径。
const ALLOWED_BARE_INTERPRETERS: &[&str] = &["python3", "python", "node", "deno", "bun", "ruby"];

fn is_path_within_adapter_dir(adapter_dir: &Path, candidate: &Path) -> bool {
    let Ok(canon_dir) = std::fs::canonicalize(adapter_dir) else {
        return false;
    };
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        adapter_dir.join(candidate)
    };
    let Ok(canon_cmd) = std::fs::canonicalize(resolved) else {
        return false;
    };
    canon_cmd.starts_with(&canon_dir) && canon_cmd.is_file()
}

fn is_valid_entry_command(adapter_dir: &Path, command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) {
        return false;
    }
    let cmd_path = Path::new(trimmed);
    if cmd_path
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return false;
    }

    if cmd_path.is_absolute() || looks_like_adapter_file(trimmed) {
        return is_path_within_adapter_dir(adapter_dir, cmd_path);
    }

    // 裸命令仅允许受控解释器白名单（禁止 sh / bash / cmd / osascript 等系统壳）
    ALLOWED_BARE_INTERPRETERS.contains(&trimmed.to_ascii_lowercase().as_str())
}

fn is_valid_entry_args(adapter_dir: &Path, command: &str, args: &[String]) -> bool {
    let cmd_trimmed = command.trim();
    let is_bare_interpreter =
        !Path::new(cmd_trimmed).is_absolute() && !looks_like_adapter_file(cmd_trimmed);
    let mut has_local_script = false;

    for raw in args {
        let arg = raw.trim();
        if raw.chars().any(|c| c.is_control()) {
            return false;
        }
        let arg_path = Path::new(arg);
        if arg_path
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return false;
        }
        if arg_path.is_absolute() && !is_path_within_adapter_dir(adapter_dir, arg_path) {
            return false;
        }

        if is_bare_interpreter {
            let lower = arg.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "-c" | "-e" | "--eval" | "-p" | "--print" | "--command"
            ) || lower.starts_with("-c=")
                || lower.starts_with("-e=")
                || lower.starts_with("--eval=")
                || lower.starts_with("-p=")
                || lower.starts_with("--print=")
                || lower.starts_with("--command=")
            {
                return false;
            }
            if !arg.is_empty()
                && !arg.starts_with('-')
                && is_path_within_adapter_dir(adapter_dir, arg_path)
            {
                has_local_script = true;
            }
        }
    }

    !is_bare_interpreter || has_local_script
}

fn is_valid_entry(adapter_dir: &Path, entry: &ExternalEntry) -> bool {
    is_valid_entry_command(adapter_dir, &entry.command)
        && is_valid_entry_args(adapter_dir, &entry.command, &entry.args)
}

fn resolve_exec_command(adapter_dir: &Path, command: &str) -> PathBuf {
    let trimmed = command.trim();
    let p = Path::new(trimmed);
    if p.is_absolute() {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    } else if looks_like_adapter_file(trimmed) {
        let joined = adapter_dir.join(p);
        std::fs::canonicalize(&joined).unwrap_or(joined)
    } else {
        PathBuf::from(trimmed)
    }
}

fn collect_adapter_files_for_hash(
    root: &Path,
    canon_root: &Path,
    dir: &Path,
    depth: usize,
    total_bytes: &mut u64,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("读取插件目录失败 {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("遍历插件目录失败: {e}"))?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("读取插件文件元数据失败 {}: {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "外部插件目录不允许包含符号链接: {}",
                path.display()
            ));
        }
        if meta.is_dir() {
            if depth >= MAX_HASH_DEPTH {
                return Err("外部插件目录层级过深（超过 4 层）".into());
            }
            collect_adapter_files_for_hash(root, canon_root, &path, depth + 1, total_bytes, out)?;
        } else if meta.is_file() {
            if out.len() >= MAX_HASH_FILES {
                return Err("外部插件文件数量超过 500 个上限".into());
            }
            let len = meta.len();
            *total_bytes = total_bytes
                .checked_add(len)
                .ok_or_else(|| "外部插件总大小溢出".to_string())?;
            if *total_bytes > MAX_HASH_TOTAL_BYTES {
                return Err("外部插件目录总大小超过 32MB 上限".into());
            }
            let canon_file = std::fs::canonicalize(&path)
                .map_err(|e| format!("解析插件文件路径失败 {}: {e}", path.display()))?;
            if !canon_file.starts_with(canon_root) {
                return Err(format!("外部插件文件逃逸根目录: {}", path.display()));
            }
            let rel = path
                .strip_prefix(root)
                .map_err(|_| "计算插件相对路径失败".to_string())?;
            let rel_norm = rel
                .components()
                .filter_map(|c| match c {
                    Component::Normal(s) => s.to_str(),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/");
            let bytes = std::fs::read(&path)
                .map_err(|e| format!("读取插件文件失败 {}: {e}", path.display()))?;
            out.push((rel_norm, bytes));
        } else {
            return Err(format!("外部插件目录包含非常规文件: {}", path.display()));
        }
    }
    Ok(())
}

/// 计算外部插件目录的内容 SHA-256 哈希，用于信任固定与篡改检测。
pub fn adapter_content_hash(adapter_dir: &Path) -> Result<String, String> {
    let canon_root = std::fs::canonicalize(adapter_dir)
        .map_err(|e| format!("无法解析外部插件目录 {}: {e}", adapter_dir.display()))?;
    let mut files = Vec::new();
    let mut total_bytes = 0u64;
    collect_adapter_files_for_hash(
        adapter_dir,
        &canon_root,
        adapter_dir,
        0,
        &mut total_bytes,
        &mut files,
    )?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (rel, bytes) in files {
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 扫描 <qwitch_data_dir>/adapters/<adapter_id>/manifest.json（仅解析清单，不执行插件）
pub fn scan_external_manifests(data_dir: &Path) -> Vec<(PathBuf, ExternalManifestFile)> {
    let mut out = Vec::new();
    let adapters_root = data_dir.join("adapters");
    let _ = std::fs::create_dir_all(&adapters_root);

    let Ok(entries) = std::fs::read_dir(&adapters_root) else {
        return out;
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            std::fs::symlink_metadata(p)
                .map(|m| m.is_dir() && !m.file_type().is_symlink())
                .unwrap_or(false)
        })
        .collect();
    dirs.sort();

    for dir in dirs {
        let Some(dir_name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let manifest_path = dir.join("manifest.json");
        let Ok(manifest_meta) = std::fs::symlink_metadata(&manifest_path) else {
            continue;
        };
        if !manifest_meta.is_file()
            || manifest_meta.file_type().is_symlink()
            || manifest_meta.len() > MAX_MANIFEST_BYTES
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(&manifest_path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_slice::<ExternalManifestFile>(&bytes) else {
            continue;
        };
        // 目录名必须与 manifest.id 一致，且通过安全 ID 与入口校验，防止同名覆盖或目录外执行
        if dir_name != parsed.id
            || !crate::adapter::is_valid_adapter_id(&parsed.id)
            || !is_valid_entry(&dir, &parsed.entry)
        {
            continue;
        }
        out.push((dir, parsed));
    }
    out
}

/// 加载适配当前平台的外部适配器与目录路径（信任及哈希门禁由调用方 create_all_adapters 负责）
pub fn discover_external_adapters(data_dir: &Path) -> HashMap<String, (ExternalAdapter, PathBuf)> {
    let current_os = ExternalAdapter::current_platform_tag();
    scan_external_manifests(data_dir)
        .into_iter()
        .filter(|(_, m)| m.platforms.is_empty() || m.platforms.iter().any(|p| p == current_os))
        .map(|(dir, m)| {
            let id = m.id.clone();
            let dir_clone = dir.clone();
            let adapter = ExternalAdapter::from_manifest_file(dir, m);
            (id, (adapter, dir_clone))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ext(id: &str, command: &str, args: Vec<String>) -> ExternalAdapter {
        ExternalAdapter::from_manifest_file(
            std::env::temp_dir(),
            ExternalManifestFile {
                id: id.to_string(),
                name: format!("Ext {id}"),
                version: "0.1.0".to_string(),
                platforms: vec![],
                entry: ExternalEntry {
                    command: command.to_string(),
                    args,
                },
                timeout_ms: 5000,
                capabilities: ExternalCapabilities {
                    auth_methods: vec![AuthMethod::TokenPaste],
                    supports_quota_query: true,
                    supports_daily_claim: false,
                    supports_client_switch: false,
                    supports_multi_instance: false,
                },
            },
        )
    }

    #[test]
    fn external_adapter_rejects_spoofed_tool_id() {
        let adapter = make_ext("my_plugin", "echo", vec![]);
        let spoofed = CredentialEnvelope {
            tool_id: "trae_cn".to_string(),
            uid: "u_1".to_string(),
            display_name: None,
            display_email: None,
            payload: crate::domain::CredentialPayload::Token {
                access_token: "tok".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        assert!(adapter.validate_envelope_tool_id(spoofed).is_err());

        let valid = CredentialEnvelope {
            tool_id: "my_plugin".to_string(),
            uid: "u_1".to_string(),
            display_name: None,
            display_email: None,
            payload: crate::domain::CredentialPayload::Token {
                access_token: "tok".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: HashMap::new(),
            },
        };
        assert!(adapter.validate_envelope_tool_id(valid).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn call_rpc_handles_large_stderr_without_deadlock_and_checks_id() {
        // 1. 超过 100KB 的 stderr 与合法 JSON-RPC stdout 并发读取不死锁
        let script = r#"
            read req
            i=0
            while [ "$i" -lt 2000 ]; do
              printf 'stderr noise line padding 0123456789abcdefghijklmnopqrstuvwxyz\n' >&2
              i=$((i + 1))
            done
            id=$(printf '%s' "$req" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
            printf '{"jsonrpc":"2.0","id":"%s","result":{"ok":true}}\n' "$id"
        "#;
        let adapter = make_ext("my_plugin", "/bin/sh", vec!["-c".into(), script.into()]);
        let res = adapter
            .call_rpc("ping", serde_json::json!({}))
            .await
            .expect("large stderr must not deadlock");
        assert_eq!(res.get("ok").and_then(|v| v.as_bool()), Some(true));

        // 2. JSON-RPC id 不匹配或缺失拒绝
        let bad_id = make_ext(
            "my_plugin",
            "/bin/sh",
            vec![
                "-c".into(),
                r#"read _; printf '{"jsonrpc":"2.0","id":"wrong-id","result":{}}\n'"#.into(),
            ],
        );
        let err = bad_id
            .call_rpc("ping", serde_json::json!({}))
            .await
            .expect_err("id mismatch must fail");
        assert!(err.to_string().contains("id 缺失或不匹配"));

        let missing_id = make_ext(
            "my_plugin",
            "/bin/sh",
            vec![
                "-c".into(),
                r#"read _; printf '{"jsonrpc":"2.0","result":{"ok":true}}\n'"#.into(),
            ],
        );
        let missing_err = missing_id
            .call_rpc("ping", serde_json::json!({}))
            .await
            .expect_err("missing id must fail");
        assert!(missing_err.to_string().contains("id 缺失或不匹配"));

        // 3. 外部插件尝试返回内置 tool_id 时被 parse_manual_token 拦截
        let spoof = make_ext(
            "my_plugin",
            "/bin/sh",
            vec![
                "-c".into(),
                r#"read req; id=$(printf '%s' "$req" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p'); printf '{"jsonrpc":"2.0","id":"%s","result":{"tool_id":"trae_cn","uid":"u1","display_email":null,"payload":{"kind":"token","access_token":"t","refresh_token":null,"expires_at":null,"extra":{}}}}\n' "$id""#.into(),
            ],
        );
        let spoof_err = spoof
            .parse_manual_token("t")
            .await
            .expect_err("spoofed tool_id must fail");
        assert!(spoof_err.to_string().contains("tool_id"));

        // 4. 大 stdin 负载（>128 KiB）面对不读 stdin 直接挂起的子进程，必须在超时后退出而不死锁
        let mut slow = make_ext("my_plugin", "/bin/sh", vec!["-c".into(), "sleep 5".into()]);
        slow.timeout_ms = 300;
        let big_payload = "x".repeat(256 * 1024);
        let timeout_err = slow
            .parse_manual_token(&big_payload)
            .await
            .expect_err("large stdin on slow child must time out without deadlock");
        assert!(timeout_err.to_string().contains("超时"));
    }

    #[test]
    fn scan_rejects_dir_id_mismatch_and_content_hash_detects_tampering() {
        let root = std::env::temp_dir().join(format!("qwitch_ext_scan_{}", Uuid::new_v4()));
        let mismatch_dir = root.join("adapters").join("wrong_dir");
        std::fs::create_dir_all(&mismatch_dir).unwrap();
        std::fs::write(
            mismatch_dir.join("manifest.json"),
            serde_json::json!({
                "id": "spoofed_id",
                "name": "Spoof",
                "version": "0.1.0",
                "entry": { "command": "python3" },
                "capabilities": {}
            })
            .to_string(),
        )
        .unwrap();
        assert!(scan_external_manifests(&root).is_empty());

        let valid_dir = root.join("adapters").join("valid_plugin");
        std::fs::create_dir_all(&valid_dir).unwrap();
        std::fs::write(
            valid_dir.join("manifest.json"),
            serde_json::json!({
                "id": "valid_plugin",
                "name": "Valid",
                "version": "0.1.0",
                "entry": { "command": "run.py" },
                "capabilities": {}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(valid_dir.join("run.py"), b"print('v1')").unwrap();
        let scanned = scan_external_manifests(&root);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].1.id, "valid_plugin");

        let hash1 = adapter_content_hash(&valid_dir).expect("hash1");
        std::fs::write(valid_dir.join("run.py"), b"print('v2')").unwrap();
        let hash2 = adapter_content_hash(&valid_dir).expect("hash2");
        assert_ne!(hash1, hash2);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn entry_command_rejects_absolute_outside_paths_and_missing_relative_paths() {
        let root = std::env::temp_dir().join(format!("qwitch_entry_cmd_{}", Uuid::new_v4()));
        let dir = root.join("adapters").join("plugin");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.py"), b"print(1)").unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("helper"), b"#!/bin/sh\n").unwrap();

        // 绝对路径指向插件目录之外：必须拒绝（历史实现无条件放行）
        assert!(!is_valid_entry_command(&dir, "/bin/sh"));
        assert!(!is_valid_entry_command(&dir, "/bin/echo"));
        assert!(!is_valid_entry_command(&dir, "/etc/passwd"));

        // 绝对路径位于插件目录内：允许
        let inside_abs = dir.join("run.py");
        assert!(is_valid_entry_command(&dir, &inside_abs.to_string_lossy()));

        // 相对路径存在且在目录内：允许
        assert!(is_valid_entry_command(&dir, "run.py"));
        assert!(is_valid_entry_command(&dir, "./run.py"));
        assert!(is_valid_entry_command(&dir, "bin/helper"));

        // 相对路径不存在：拒绝（历史实现 fail-open 放行）
        assert!(!is_valid_entry_command(&dir, "missing.py"));
        assert!(!is_valid_entry_command(&dir, "./missing.py"));
        assert!(!is_valid_entry_command(&dir, "bin/missing"));

        // 父目录穿越：拒绝
        assert!(!is_valid_entry_command(&dir, "../escape.py"));
        assert!(!is_valid_entry_command(&dir, "bin/../../escape.py"));

        // 带脚本扩展名但不存在：同样拒绝（不得被当作 PATH 命令放行）
        assert!(!is_valid_entry_command(&dir, "missing.sh"));
        assert!(!is_valid_entry_command(&dir, "adapter.py"));

        // 裸命令名（无扩展名）按协议视为解释器，由 PATH 解析
        assert!(is_valid_entry_command(&dir, "python3"));
        assert!(is_valid_entry_command(&dir, "node"));

        // 空串与控制字符：拒绝
        assert!(!is_valid_entry_command(&dir, ""));
        assert!(!is_valid_entry_command(&dir, "   "));
        assert!(!is_valid_entry_command(&dir, "run.py\0"));
        assert!(!is_valid_entry_command(&dir, "run\n.py"));

        // 裸壳命令必须拒绝，裸解释器参数必须落地在插件目录内
        assert!(!is_valid_entry_command(&dir, "sh"));
        assert!(!is_valid_entry_command(&dir, "osascript"));
        assert!(!is_valid_entry(
            &dir,
            &ExternalEntry {
                command: "python3".into(),
                args: vec!["-c".into(), "print(1)".into()],
            }
        ));
        assert!(!is_valid_entry(
            &dir,
            &ExternalEntry {
                command: "python3".into(),
                args: vec!["/etc/passwd".into()],
            }
        ));
        assert!(is_valid_entry(
            &dir,
            &ExternalEntry {
                command: "python3".into(),
                args: vec!["run.py".into()],
            }
        ));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn call_rpc_rechecks_pinned_hash_against_runtime_tampering() {
        let root = std::env::temp_dir().join(format!("qwitch_ext_toctou_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("run.py"), b"print('{}')").unwrap();
        let manifest = ExternalManifestFile {
            id: "toctou_demo".into(),
            name: "TOCTOU Demo".into(),
            version: "0.1.0".into(),
            platforms: vec![],
            entry: ExternalEntry {
                command: "run.py".into(),
                args: vec![],
            },
            timeout_ms: 3000,
            capabilities: ExternalCapabilities {
                auth_methods: vec![AuthMethod::TokenPaste],
                supports_quota_query: true,
                supports_daily_claim: false,
                supports_client_switch: false,
                supports_multi_instance: false,
            },
        };
        let hash = adapter_content_hash(&root).expect("hash");
        let adapter =
            ExternalAdapter::from_manifest_file(root.clone(), manifest).with_pinned_hash(hash);

        // 启动后篡改脚本，call_rpc 必须在 spawn 前拒绝执行
        std::fs::write(root.join("run.py"), b"print('evil')").unwrap();
        let err = adapter
            .call_rpc("ping", serde_json::json!({}))
            .await
            .expect_err("tampered plugin must be rejected at call_rpc");
        assert!(err.to_string().contains("哈希不一致"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
