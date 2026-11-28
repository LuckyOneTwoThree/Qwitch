use async_trait::async_trait;
use keyring::Entry;
use rand::RngCore;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

const KEYRING_SERVICE: &str = "dev.qwitch.app";
const LEGACY_KEYRING_SERVICE: &str = "dev.quiver.app";
const KEYRING_MASTER_USER: &str = "master_key_v1";

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 为子进程隐藏控制台窗口（仅在 Windows 生效，其他平台为空操作）。
///
/// release 构建使用 `windows_subsystem = "windows"`，进程本身没有控制台。此时 spawn
/// 控制台程序（`tasklist` / `taskkill` / `node` / `bun` 等）会让 Windows 为子进程
/// **新建**一个控制台窗口并闪一下黑框。凡是只读取输出、不面向用户的探测型子进程，
/// 都必须经过本函数，否则会出现周期性闪窗。
///
/// 统一收口到 platform，避免各模块各自写 `creation_flags` 时漏掉某一处。
pub fn hide_console_std(cmd: &mut Command) -> &mut Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW)
    }
    #[cfg(not(target_os = "windows"))]
    {
        cmd
    }
}

/// [`hide_console_std`] 的 tokio 版本。
pub fn hide_console_tokio(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(target_os = "windows")]
    {
        // tokio::process::Command 自带 CreationFlags 支持，无需再导入 CommandExt
        cmd.creation_flags(CREATE_NO_WINDOW)
    }
    #[cfg(not(target_os = "windows"))]
    {
        cmd
    }
}

#[cfg(target_os = "macos")]
fn escape_pgrep_regex(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(
            ch,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 等待并（必要时）强制结束某 app 的全部辅助进程（Renderer/GPU/utility 等）。
/// 用于切号：主进程退出后仍残留的 helper 会占用单实例锁/用户数据，导致重启秒退。
#[cfg(target_os = "macos")]
async fn cleanup_bundle_leftovers(app_names: &[String], extra_ms: u64) {
    for app_name in app_names {
        let needle = format!("/{}/Contents/", escape_pgrep_regex(app_name));
        let deadline = Instant::now() + Duration::from_millis(extra_ms);
        loop {
            let running = tokio::process::Command::new("pgrep")
                .args(["-f", &needle])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            if !running {
                break;
            }
            if Instant::now() >= deadline {
                let _ = tokio::process::Command::new("pkill")
                    .args(["-KILL", "-f", &needle])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// 在给定预算内轮询「主进程是否已退出」，退出即返回 true。
async fn wait_until_stopped(
    primary_names: &[String],
    proc_names: &[String],
    budget_ms: u64,
) -> Result<bool, HostError> {
    let p_clone = primary_names.to_vec();
    let proc_clone = proc_names.to_vec();
    let deadline = Instant::now() + Duration::from_millis(budget_ms);
    loop {
        let p = p_clone.clone();
        let pr = proc_clone.clone();
        let running = tokio::task::spawn_blocking(move || {
            NativePlatformHost::is_client_running_names(&p, &pr)
        })
        .await
        .map_err(|e| HostError::OperationFailed(e.to_string()))??;
        if !running {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 终止客户端自带的常驻子进程（网关/本地服务）。
///
/// 这些进程不在 `.app` bundle 内，`cleanup_bundle_leftovers` 抓不到。客户端被
/// 外部终止后它们会变成孤儿并继续占用客户端数据目录，下一次启动的新实例会
/// 拒绝迁移（有道龙虾 `openclaw-gateway` → 「Another gateway already owns this
/// state directory」→ 客户端弹「AI 引擎网关启动失败」）。
#[cfg(target_os = "macos")]
async fn stop_mac_sidecar_processes(names: &[&str], budget_ms: u64) {
    for name in names {
        let needle = escape_pgrep_regex(name);
        let is_alive = || {
            let needle = needle.clone();
            async move {
                tokio::process::Command::new("pgrep")
                    .args(["-f", &needle])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false)
            }
        };
        if !is_alive().await {
            continue;
        }
        let _ = tokio::process::Command::new("pkill")
            .args(["-TERM", "-f", &needle])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        let deadline = Instant::now() + Duration::from_millis(budget_ms.max(500));
        while is_alive().await {
            if Instant::now() >= deadline {
                let _ = tokio::process::Command::new("pkill")
                    .args(["-KILL", "-f", &needle])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

#[derive(Debug, Error)]
pub enum HostError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Keyring error: {0}")]
    Keyring(String),
    #[error("Platform operation unsupported or failed: {0}")]
    OperationFailed(String),
}

#[derive(Debug, Clone)]
pub struct ClientSpec {
    pub mac_app_names: &'static [&'static str],
    pub mac_process_names: &'static [&'static str],
    /// 客户端自带的**常驻子进程**（网关/本地服务）。
    ///
    /// 这些进程不在 `.app` bundle 内，`cleanup_bundle_leftovers` 抓不到；
    /// 客户端被外部终止后会变成孤儿并继续占用它自己的数据目录，
    /// 下次启动的新实例会因此拒绝迁移（有道龙虾的 `openclaw-gateway` 就是这样：
    /// 日志 `Another gateway (pid …) already owns this state directory`，
    /// 客户端弹「AI 引擎网关启动失败」）。
    pub mac_sidecar_process_names: &'static [&'static str],
    pub win_exe_names: &'static [&'static str],
    pub win_folder_names: &'static [&'static str],
}

/// 校验「将被启动的客户端可执行文件」是否可信。
///
/// 背景：`custom_client_paths` 允许用户覆盖客户端路径，历史实现只校验
/// 「绝对路径且存在」，随后该路径被 `launch_client` 直接 `Command::new(...)`
/// 执行。这使一次渲染进程污染即可升级为任意代码执行（写入 `/bin/sh` 之类的
/// 路径再触发启动）。
///
/// 本函数不改变「自定义路径」这一合法能力，只在真正启动前复核三点：
/// 1. 名称必须匹配该工具的官方客户端名（`ClientSpec` 的 app/exe 白名单）；
/// 2. macOS 必须是 `.app` bundle，且其 `Contents/MacOS/<name>` 存在；
/// 3. 父目录必须位于受信任的安装位置（`/Applications`、`~/Applications`、
///    `%LOCALAPPDATA%\Programs`、`%ProgramFiles%`）或用户主目录之下。
///
/// 说明：此处**不做**代码签名校验（本项目暂不做 Apple 签名），因此它挡的是
/// 「指向系统二进制的劫持」，不是「用户主目录内被植入的恶意同名程序」。
pub fn validate_client_executable(tool_id: &str, executable: &Path) -> Result<(), String> {
    let spec = client_spec(tool_id).ok_or_else(|| format!("未知或不支持的工具: {tool_id}"))?;

    let Some(file_name) = executable.file_name().and_then(|n| n.to_str()) else {
        return Err(format!("客户端路径非法: {}", executable.display()));
    };

    #[cfg(target_os = "macos")]
    {
        let name_ok = spec.mac_app_names.iter().any(|n| n == &file_name);
        if !name_ok {
            return Err(format!(
                "客户端可执行文件名不受支持：{file_name}（{tool_id} 允许：{}）",
                spec.mac_app_names.join(" / ")
            ));
        }
        if executable.extension().and_then(|e| e.to_str()) != Some("app") {
            return Err(format!(
                "客户端路径必须是 .app bundle：{}",
                executable.display()
            ));
        }
        let macos_dir = executable.join("Contents/MacOS");
        let has_binary = std::fs::read_dir(&macos_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .any(|e| e.path().is_file() && e.path().extension().is_none())
            })
            .unwrap_or(false);
        if !has_binary {
            return Err(format!(
                "客户端 bundle 内缺少可执行文件：{}",
                macos_dir.display()
            ));
        }
    }

    #[cfg(target_os = "windows")]
    {
        let name_ok = spec
            .win_exe_names
            .iter()
            .any(|n| n.eq_ignore_ascii_case(file_name));
        if !name_ok {
            return Err(format!(
                "客户端可执行文件名不受支持：{file_name}（{tool_id} 允许：{}）",
                spec.win_exe_names.join(" / ")
            ));
        }
        if executable
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| !e.eq_ignore_ascii_case("exe"))
            .unwrap_or(true)
        {
            return Err(format!("客户端路径必须是 .exe：{}", executable.display()));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (spec, file_name);
        return Err("当前操作系统暂不支持客户端路径覆盖".to_string());
    }

    // 目录白名单：仅允许受信任的安装位置或用户主目录之下
    let canonical = std::fs::canonicalize(executable)
        .map_err(|e| format!("客户端路径无法解析（{}）：{e}", executable.display()))?;
    let mut allowed_roots: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        allowed_roots.push(PathBuf::from("/Applications"));
        if let Some(home) = std::env::var_os("HOME") {
            allowed_roots.push(PathBuf::from(&home).join("Applications"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            allowed_roots.push(PathBuf::from(&local).join("Programs"));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            allowed_roots.push(PathBuf::from(pf));
        }
        if let Some(pf86) = std::env::var_os("ProgramFiles(x86)") {
            allowed_roots.push(PathBuf::from(pf86));
        }
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        allowed_roots.push(PathBuf::from(home));
    }

    if allowed_roots.is_empty() {
        return Err("无法确定受信任的安装目录，已拒绝启动客户端".into());
    }
    if !allowed_roots.iter().any(|root| {
        std::fs::canonicalize(root)
            .map(|r| canonical.starts_with(&r))
            .unwrap_or(false)
    }) {
        // 保留白名单（这是防「设置里写入任意二进制即可执行」的关键约束），但把
        // 可操作指引写清楚：Windows 上装到 D:\Software 之类会命中这里，而 macOS
        // 上 /Applications 几乎天然覆盖，所以文案必须显式告诉用户该放哪里。
        return Err(format!(
            "客户端路径不在受信任的安装目录内：{}\n允许的根目录：{}\n\
             请把客户端安装在上述目录之一（Windows 默认安装位置通常是 \
             %LOCALAPPDATA%\\Programs），或将其移动到用户主目录下后重试。",
            canonical.display(),
            allowed_roots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }

    Ok(())
}

pub fn client_spec(tool_id: &str) -> Option<ClientSpec> {
    match tool_id {
        "trae" => Some(ClientSpec {
            mac_app_names: &["Trae.app"],
            mac_process_names: &["Trae"],
            win_exe_names: &["Trae.exe"],
            win_folder_names: &["Trae"],
            mac_sidecar_process_names: &[],
        }),
        "trae_cn" => Some(ClientSpec {
            mac_app_names: &["Trae CN.app"],
            mac_process_names: &["Trae CN"],
            win_exe_names: &["Trae CN.exe"],
            win_folder_names: &["Trae CN"],
            mac_sidecar_process_names: &[],
        }),
        "workbuddy" => Some(ClientSpec {
            mac_app_names: &["WorkBuddy.app"],
            mac_process_names: &["WorkBuddy"],
            win_exe_names: &["WorkBuddy.exe"],
            win_folder_names: &["WorkBuddy"],
            mac_sidecar_process_names: &[],
        }),
        "qoder" => Some(ClientSpec {
            mac_app_names: &["Qoder.app"],
            mac_process_names: &["Qoder"],
            win_exe_names: &["Qoder.exe"],
            win_folder_names: &["Qoder"],
            mac_sidecar_process_names: &[],
        }),
        "qoder_cn" => Some(ClientSpec {
            mac_app_names: &["Qoder CN.app", "Qoder.app"],
            mac_process_names: &["Qoder CN", "Qoder"],
            win_exe_names: &["Qoder CN.exe", "Qoder.exe"],
            win_folder_names: &["Qoder CN", "Qoder"],
            mac_sidecar_process_names: &[],
        }),
        "minimax_code_cn" => Some(ClientSpec {
            mac_app_names: &["MiniMax Code.app"],
            mac_process_names: &["MiniMax Code"],
            win_exe_names: &["MiniMax Code.exe"],
            win_folder_names: &["MiniMax Code", "MiniMax"],
            mac_sidecar_process_names: &[],
        }),
        "lobsterai" => Some(ClientSpec {
            mac_app_names: &["LobsterAI.app"],
            mac_process_names: &["LobsterAI"],
            win_exe_names: &["LobsterAI.exe"],
            win_folder_names: &["LobsterAI"],
            mac_sidecar_process_names: &["openclaw-gateway"],
        }),
        "doubaowork" => Some(ClientSpec {
            mac_app_names: &["DoubaoWork.app"],
            mac_process_names: &["DoubaoWork", "DoubaoWork Browser"],
            win_exe_names: &["DoubaoWork.exe"],
            win_folder_names: &["DoubaoWork"],
            mac_sidecar_process_names: &[],
        }),
        "codearts" => Some(ClientSpec {
            mac_app_names: &["CodeArts.app"],
            mac_process_names: &["CodeArts"],
            win_exe_names: &["CodeArts.exe"],
            win_folder_names: &["CodeArts"],
            mac_sidecar_process_names: &[],
        }),
        _ => None,
    }
}

#[async_trait]
pub trait PlatformHost: Send + Sync {
    fn qwitch_data_dir(&self) -> Result<PathBuf, HostError>;
    fn client_config_dir(&self, tool_id: &str) -> Result<PathBuf, HostError>;
    fn detect_client_executable(&self, tool_id: &str) -> Option<PathBuf>;
    fn is_client_running(&self, tool_id: &str) -> Result<bool, HostError>;
    /// 判断客户端是否有**任何**进程存在（不区分主进程与辅助进程）。
    ///
    /// 用于「客户端已拉起、但主进程未被确认存活」的兜底判定：
    /// 这种情况下**不应**回滚已经写入的会话快照（回滚会与正在运行的客户端
    /// 抢同一个会话文件）。默认实现复用 `is_client_running`。
    fn client_process_present(&self, tool_id: &str) -> Result<bool, HostError> {
        self.is_client_running(tool_id)
    }
    async fn terminate_client_gracefully(
        &self,
        tool_id: &str,
        timeout_ms: u64,
    ) -> Result<(), HostError>;
    fn launch_client(&self, executable: &Path, args: &[String]) -> Result<(), HostError>;
    fn get_or_create_master_key(&self, expectation: &KeyExpectation)
        -> Result<[u8; 32], HostError>;
    fn set_launch_at_login(&self, enabled: bool) -> Result<(), String>;
}

/// 多实例 profile 作用域宿主：仅覆盖目标工具的 `client_config_dir`，
/// 指向 `--user-data-dir` 指定的隔离 profile，用于“从某个 profile 导入并采集设备身份”。
/// 其余能力全部委托给本机真实宿主。
pub struct ScopedPlatformHost {
    inner: Arc<dyn PlatformHost>,
    tool_id: String,
    config_dir: PathBuf,
}

impl ScopedPlatformHost {
    pub fn new(inner: Arc<dyn PlatformHost>, tool_id: &str, config_dir: PathBuf) -> Self {
        Self {
            inner,
            tool_id: tool_id.to_string(),
            config_dir,
        }
    }
}

#[async_trait]
impl PlatformHost for ScopedPlatformHost {
    fn qwitch_data_dir(&self) -> Result<PathBuf, HostError> {
        self.inner.qwitch_data_dir()
    }

    fn client_config_dir(&self, tool_id: &str) -> Result<PathBuf, HostError> {
        if tool_id == self.tool_id {
            Ok(self.config_dir.clone())
        } else {
            self.inner.client_config_dir(tool_id)
        }
    }

    fn detect_client_executable(&self, tool_id: &str) -> Option<PathBuf> {
        self.inner.detect_client_executable(tool_id)
    }

    fn is_client_running(&self, tool_id: &str) -> Result<bool, HostError> {
        self.inner.is_client_running(tool_id)
    }

    fn client_process_present(&self, tool_id: &str) -> Result<bool, HostError> {
        self.inner.client_process_present(tool_id)
    }

    async fn terminate_client_gracefully(
        &self,
        tool_id: &str,
        timeout_ms: u64,
    ) -> Result<(), HostError> {
        self.inner
            .terminate_client_gracefully(tool_id, timeout_ms)
            .await
    }

    fn launch_client(&self, executable: &Path, args: &[String]) -> Result<(), HostError> {
        self.inner.launch_client(executable, args)
    }

    fn get_or_create_master_key(
        &self,
        expectation: &KeyExpectation,
    ) -> Result<[u8; 32], HostError> {
        self.inner.get_or_create_master_key(expectation)
    }

    fn set_launch_at_login(&self, enabled: bool) -> Result<(), String> {
        self.inner.set_launch_at_login(enabled)
    }
}

static CUSTOM_PATHS_CACHE: std::sync::OnceLock<
    std::sync::RwLock<Option<std::collections::HashMap<String, String>>>,
> = std::sync::OnceLock::new();

static LEGACY_MIGRATED: std::sync::Once = std::sync::Once::new();

fn custom_paths_cache(
) -> &'static std::sync::RwLock<Option<std::collections::HashMap<String, String>>> {
    CUSTOM_PATHS_CACHE.get_or_init(|| std::sync::RwLock::new(None))
}

/// 同步内存中的自定义客户端路径缓存，避免每次进程检测都重新打开 SQLite 连接。
pub fn sync_custom_client_paths(paths: &std::collections::HashMap<String, String>) {
    if let Ok(mut guard) = custom_paths_cache().write() {
        *guard = Some(paths.clone());
    }
}

pub struct NativePlatformHost;

impl NativePlatformHost {
    fn read_custom_client_path(&self, tool_id: &str) -> Option<PathBuf> {
        if let Ok(guard) = custom_paths_cache().read() {
            if let Some(map) = guard.as_ref() {
                let raw = map
                    .get(tool_id)
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())?;
                let path = PathBuf::from(raw);
                return (path.exists() && validate_client_executable(tool_id, &path).is_ok())
                    .then_some(path);
            }
        }

        let db_path = self.qwitch_data_dir().ok()?.join("qwitch.db");
        if !db_path.is_file() {
            return None;
        }
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()?;
        let _ = conn.busy_timeout(Duration::from_millis(2000));
        let json: String = conn
            .query_row(
                "SELECT value_json FROM app_settings WHERE key = 'app.settings'",
                [],
                |row| row.get(0),
            )
            .ok()?;
        let val: serde_json::Value = serde_json::from_str(&json).ok()?;
        let raw = val
            .get("customClientPaths")
            .and_then(|m| m.get(tool_id))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        let path = PathBuf::from(raw);
        if path.exists() && validate_client_executable(tool_id, &path).is_ok() {
            Some(path)
        } else {
            None
        }
    }

    fn resolve_target_process_names(&self, tool_id: &str) -> (Vec<String>, Vec<String>) {
        let mut mac_apps = Vec::new();
        let mut mac_procs = Vec::new();
        let mut win_exes = Vec::new();

        if let Some(spec) = client_spec(tool_id) {
            for a in spec.mac_app_names {
                mac_apps.push((*a).to_string());
            }
            for p in spec.mac_process_names {
                mac_procs.push((*p).to_string());
            }
            for e in spec.win_exe_names {
                win_exes.push((*e).to_string());
            }
        }

        if let Some(custom) = self.read_custom_client_path(tool_id) {
            if let Some(name) = custom.file_name().and_then(|n| n.to_str()) {
                #[cfg(target_os = "macos")]
                {
                    if name.ends_with(".app") {
                        if !mac_apps.iter().any(|x| x == name) {
                            mac_apps.push(name.to_string());
                        }
                        let stem = name.trim_end_matches(".app").to_string();
                        if !mac_procs.iter().any(|x| x == &stem) {
                            mac_procs.push(stem);
                        }
                    } else if !mac_procs.iter().any(|x| x == name) {
                        mac_procs.push(name.to_string());
                    }
                }
                #[cfg(target_os = "windows")]
                {
                    if name.to_lowercase().ends_with(".exe")
                        && !win_exes.iter().any(|x| x.eq_ignore_ascii_case(name))
                    {
                        win_exes.push(name.to_string());
                    }
                }
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                {
                    let _ = name;
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            (mac_apps, mac_procs)
        }
        #[cfg(target_os = "windows")]
        {
            let _ = (mac_apps, mac_procs);
            (win_exes, Vec::new())
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (mac_apps, mac_procs, win_exes);
            (Vec::new(), Vec::new())
        }
    }

    /// 按进程名判断是否运行。
    ///
    /// **注意**：`pgrep -x` 对 Electron 应用的 Native Messaging Host 同样会命中
    /// —— 该进程的 `comm` 字段就是主二进制路径（如
    /// `/Applications/Qoder CN.app/Contents/MacOS/Qoder CN`），只是 argv 多了
    /// 一个 `.cjs` 脚本参数。因此这里在命中后必须复核 argv，排除辅助进程。
    #[cfg(target_os = "macos")]
    fn is_macos_proc_running(proc_name: &str) -> Result<bool, HostError> {
        if proc_name.len() <= 15 {
            let output = Command::new("pgrep")
                .args(["-x", proc_name])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()?;
            if !output.status.success() {
                return Ok(false);
            }
            // 命中后复核：仅当存在非辅助进程的实例时才判定为运行中
            return Ok(Self::has_non_helper_pids(
                &String::from_utf8_lossy(&output.stdout),
                proc_name,
            ));
        }
        let needle = format!("/Contents/MacOS/{}", escape_pgrep_regex(proc_name));
        let output = Command::new("pgrep")
            .args(["-f", &needle])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()?;
        if output.status.success()
            && Self::has_non_helper_pids(&String::from_utf8_lossy(&output.stdout), proc_name)
        {
            return Ok(true);
        }
        let prefix: String = proc_name.chars().take(15).collect();
        let fallback_output = Command::new("pgrep")
            .args(["-x", &prefix])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()?;
        if !fallback_output.status.success() {
            return Ok(false);
        }
        Ok(Self::has_non_helper_pids(
            &String::from_utf8_lossy(&fallback_output.stdout),
            proc_name,
        ))
    }

    /// 从 `ps` 输出的完整命令行中剥离 `argv[0]`（兼容含空格的 macOS `.app` 路径）。
    #[cfg(target_os = "macos")]
    fn strip_macos_argv0<'a>(command: &'a str, proc_name: &str) -> &'a str {
        let trimmed = command.trim();
        let needle = format!("/Contents/MacOS/{proc_name}");
        if let Some(idx) = trimmed.find(&needle) {
            let after = &trimmed[idx + needle.len()..];
            if after.is_empty() || after.starts_with(char::is_whitespace) {
                return after.trim();
            }
        }
        match trimmed.find(' ') {
            Some(idx) => trimmed[idx + 1..].trim(),
            None => "",
        }
    }

    /// 在 `pgrep` 命中的 PID 列表中，筛选是否存在**非辅助**实例。
    ///
    /// 用于排除 Native Messaging Host / Renderer / GPU 等共享同名或截断名 `comm` 的辅助进程。
    #[cfg(target_os = "macos")]
    fn has_non_helper_pids(pids_stdout: &str, proc_name: &str) -> bool {
        for pid in pids_stdout.split_whitespace() {
            let ps_out = match Command::new("ps")
                .args(["-p", pid, "-o", "command="])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
            {
                Ok(out) => out,
                // `ps` 不可用（沙箱 / TCC 拒绝执行）时，旧实现直接按「运行中」处理，
                // 客户端退出后残留的 native-messaging-host 会让界面永远显示运行中。
                // 先退回 `pgrep -fl` 读该 pid 的完整命令行再判定，
                // 只有连命令行都读不到时才保守按运行中处理。
                Err(_) => match Self::command_line_for_pid(pid, proc_name) {
                    Some(command) => {
                        let rest = Self::strip_macos_argv0(&command, proc_name);
                        if rest.is_empty() || !Self::is_macos_helper_argv(rest) {
                            return true;
                        }
                        continue;
                    }
                    None => return true,
                },
            };
            let command = String::from_utf8_lossy(&ps_out.stdout);
            let command = command.trim();
            if command.is_empty() {
                continue;
            }
            let rest = Self::strip_macos_argv0(command, proc_name);
            if rest.is_empty() || !Self::is_macos_helper_argv(rest) {
                return true;
            }
        }
        false
    }

    /// 用 `pgrep -fl` 取指定 pid 的完整命令行。
    ///
    /// 仅在 `ps` 不可用时使用：`pgrep -fl` 的输出是「pid + 完整 argv」，
    /// 且不经过 `ps`，因此在沙箱 / TCC 受限环境里仍可读。
    #[cfg(target_os = "macos")]
    fn command_line_for_pid(pid: &str, pattern: &str) -> Option<String> {
        let output = Command::new("pgrep")
            .args(["-fl", &escape_pgrep_regex(pattern)])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let line = line.trim();
            let Some((candidate, cmd)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            if candidate.trim() == pid.trim() {
                return Some(cmd.trim().to_string());
            }
        }
        None
    }

    /// 解析 `ps -axo pid=,command=` 的输出，判断指定 `.app` bundle 是否存在非辅助主进程。
    ///
    /// **主二进制名不一定与 `.app` 同名**：WorkBuddy、Trae CN 等 Electron 打包
    /// 客户端把主可执行文件命名为 `Electron`（Electron 打包的默认产物名），
    /// 形如 `/Applications/WorkBuddy.app/Contents/MacOS/Electron`。
    /// 历史实现要求 argv[0] 严格等于 `/<App>.app/Contents/MacOS/<App>`，
    /// 对这类客户端**永远匹配不到** → 误判「客户端未运行」，连锁后果：
    /// ① 切号时跳过优雅关闭，旧进程仍持有会话文件；
    /// ② 切号后的存活轮询必然超时，UI 一直卡在「重启桌面端」，
    ///    最终还被判为失败并回滚快照 —— 而客户端其实已经起来了。
    ///
    /// 现改为：只要进程位于 `<App>.app/Contents/MacOS/` 目录下即视为候选主进程
    /// （辅助进程位于 `Contents/Frameworks/...`，天然不匹配该路径），
    /// 再沿用既有的 argv 辅助进程过滤排除 Native Messaging Host / Renderer / GPU。
    #[cfg(target_os = "macos")]
    fn parse_macos_ps_has_main_process(ps_stdout: &str, app_name: &str) -> bool {
        let bundle_dir = format!("/{app_name}/Contents/MacOS/");

        for line in ps_stdout.lines() {
            let mut parts = line.trim_start().splitn(2, char::is_whitespace);
            let Some(_pid) = parts.next() else { continue };
            let Some(command) = parts.next() else {
                continue;
            };
            let command = command.trim();
            let Some(idx) = command.find(&bundle_dir) else {
                continue;
            };
            let after = &command[idx + bundle_dir.len()..];
            // 紧跟路径的必须是可执行文件名（argv[0] 的 basename），不能是路径中段
            let argv0_end = after.find(char::is_whitespace).unwrap_or(after.len());
            let argv0 = &after[..argv0_end];
            if argv0.is_empty() || argv0.contains('/') {
                continue;
            }

            let rest = after[argv0_end..].trim();
            if rest.is_empty() || !Self::is_macos_helper_argv(rest) {
                return true;
            }
        }
        false
    }

    /// 判断指定 `.app` bundle 下是否存在**任意**进程（不区分主/辅助）。
    ///
    /// 与 `is_macos_app_bundle_running` 的差别只在于不做主进程识别，
    /// 用于「客户端确实已拉起」的最弱判定。
    #[cfg(target_os = "macos")]
    fn is_macos_bundle_process_present(app_name: &str) -> bool {
        let bundle_needle = format!("/{}/Contents/MacOS/", escape_pgrep_regex(app_name));
        Command::new("pgrep")
            .args(["-f", &bundle_needle])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// 判断 Electron 系 macOS 应用的**主进程**是否在运行。
    ///
    /// 不能用 `pgrep -f "/X.app/Contents/MacOS/"` 直接判定：该模式会同时匹配到
    /// 大量辅助进程，最典型的是 Chrome Native Messaging Host —— 官方客户端注册
    /// 的浏览器扩展配套进程，其命令行形如：
    ///
    /// ```text
    /// /Applications/Qoder CN.app/Contents/MacOS/Qoder CN \
    ///   /Applications/Qoder CN.app/Contents/Resources/native-messaging-host/qoder-app-host.cjs
    /// ```
    ///
    /// 只要 Chrome 在运行且装了对应扩展，该进程就常驻，导致 Qwitch 把「浏览器扩展
    /// 通道存活」误判为「客户端正在运行」，进而：UI 显示运行中、切号前触发不必要的
    /// 终止流程、`force_kill` 提示误导用户。
    ///
    /// 因此这里改为解析 `ps` 的完整 argv，排除已知辅助进程形态后，仅当存在
    /// 「argv[0] 即主二进制且无辅助标识」的进程时才判定为运行中。
    #[cfg(target_os = "macos")]
    fn is_macos_app_bundle_running(app_name: &str) -> bool {
        // 快速前置过滤：先用 pgrep 判断「是否存在任何同名进程」。
        // 绝大多数情况下（客户端未启动）在此直接返回，避免为一次探测付出
        // `ps -axo` 的全进程遍历开销（实测 ps ≈25ms vs pgrep ≈13ms，
        // 而该函数在切号弹窗、窗口聚焦等路径上会被反复调用）。
        let bundle_needle = format!("/{}/Contents/MacOS/", escape_pgrep_regex(app_name));
        if !Command::new("pgrep")
            .args(["-f", &bundle_needle])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return false;
        }

        let output = match Command::new("ps")
            .args(["-axo", "pid=,command="])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            Ok(out) => out,
            // 受限环境（沙箱 / TCC 拒绝执行 ps）下 `ps` 会直接失败。
            // 旧实现在这里假定「运行中」，于是客户端退出后残留的
            // native-messaging-host 会让界面永远显示「桌面端运行中」。
            // `pgrep -fl` 输出的「pid + 完整命令行」与 `ps -axo pid=,command=`
            // 同构，可直接复用同一套主/辅助进程判定，无需降级为猜测。
            Err(_) => {
                return match Command::new("pgrep").args(["-fl", &bundle_needle]).output() {
                    Ok(pg) if !pg.stdout.is_empty() => Self::parse_macos_ps_has_main_process(
                        &String::from_utf8_lossy(&pg.stdout),
                        app_name,
                    ),
                    // 连命令行都读不到时才保守按运行中处理：
                    // 误报「未运行」会让切号跳过优雅关闭、并在重启后空等超时回滚。
                    _ => true,
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        Self::parse_macos_ps_has_main_process(&stdout, app_name)
    }

    /// 判断 Electron 应用主二进制之后的 argv 是否属于**辅助进程**。
    ///
    /// 覆盖两类已知形态：
    /// - 显式脚本参数：`.../native-messaging-host/qoder-app-host.cjs`、
    ///   `--type=renderer`、`--type=gpu-process` 等；
    /// - 路径参数指向 Resources 下的辅助资源。
    #[cfg(target_os = "macos")]
    fn is_macos_helper_argv(rest: &str) -> bool {
        const HELPER_MARKERS: &[&str] = &[
            "native-messaging-host",
            "--type=",
            "Helper",
            "crashpad",
            "GPU Process",
        ];
        HELPER_MARKERS.iter().any(|marker| rest.contains(marker))
    }

    fn is_client_running_names(
        primary_names: &[String],
        proc_names: &[String],
    ) -> Result<bool, HostError> {
        if primary_names.is_empty() && proc_names.is_empty() {
            return Ok(false);
        }

        #[cfg(target_os = "macos")]
        {
            for app_name in primary_names {
                if Self::is_macos_app_bundle_running(app_name) {
                    return Ok(true);
                }
            }
            for proc_name in proc_names {
                if Self::is_macos_proc_running(proc_name)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        #[cfg(target_os = "windows")]
        {
            let _ = proc_names;
            for exe in primary_names {
                let mut cmd = Command::new("tasklist");
                hide_console_std(&mut cmd);
                let output = cmd
                    .args(["/FI", &format!("IMAGENAME eq {exe}"), "/FO", "CSV", "/NH"])
                    .output()?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
                let target_quoted = format!("\"{}\"", exe.to_lowercase());
                if stdout.contains(&target_quoted) {
                    return Ok(true);
                }
            }
            Ok(false)
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (primary_names, proc_names);
            Ok(false)
        }
    }

    fn resolve_data_dirs() -> Result<(PathBuf, PathBuf), HostError> {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| HostError::OperationFailed("HOME not set".into()))?;
            let app_support = home.join("Library/Application Support");
            Ok((app_support.join("qwitch"), app_support.join("quiver")))
        }
        #[cfg(target_os = "windows")]
        {
            let appdata = std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .ok_or_else(|| HostError::OperationFailed("APPDATA not set".into()))?;
            Ok((appdata.join("qwitch"), appdata.join("quiver")))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| HostError::OperationFailed("HOME not set".into()))?;
            let share = home.join(".local/share");
            Ok((share.join("qwitch"), share.join("quiver")))
        }
    }

    fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                Self::copy_dir_recursive(&src_path, &dst_path)?;
            } else if src_path.is_file() {
                let _ = std::fs::copy(&src_path, &dst_path);
            }
        }
        Ok(())
    }

    /// 首次从旧版 Quiver 数据目录平滑迁移本地数据库、主密钥文件与自定义扩展目录
    fn migrate_legacy_data_dir(legacy_dir: &Path, new_dir: &Path) {
        if !legacy_dir.is_dir() || legacy_dir == new_dir {
            return;
        }

        let legacy_db = legacy_dir.join("quiver.db");
        let new_db = new_dir.join("qwitch.db");
        if legacy_db.is_file() && !new_db.exists() {
            let tmp_db = new_dir.join(".qwitch.db.migrating");
            if std::fs::copy(&legacy_db, &tmp_db).is_ok() {
                let _ = std::fs::rename(&tmp_db, &new_db);
                for suffix in ["-wal", "-shm"] {
                    let old_side = legacy_dir.join(format!("quiver.db{suffix}"));
                    let new_side = new_dir.join(format!("qwitch.db{suffix}"));
                    if old_side.is_file() && !new_side.exists() {
                        let _ = std::fs::copy(&old_side, &new_side);
                    }
                }
            } else {
                let _ = std::fs::remove_file(&tmp_db);
            }
        }

        let legacy_key = legacy_dir.join("master.key");
        let new_key = new_dir.join("master.key");
        if legacy_key.is_file() && !new_key.exists() {
            let tmp_key = new_dir.join(".master.key.migrating");
            if std::fs::copy(&legacy_key, &tmp_key).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(&tmp_key, std::fs::Permissions::from_mode(0o600));
                }
                let _ = std::fs::rename(&tmp_key, &new_key);
            } else {
                let _ = std::fs::remove_file(&tmp_key);
            }
        }

        for subdir in ["adapters", "clients"] {
            let old_sub = legacy_dir.join(subdir);
            let new_sub = new_dir.join(subdir);
            if old_sub.is_dir() && !new_sub.exists() {
                let _ = Self::copy_dir_recursive(&old_sub, &new_sub);
            }
        }
    }

    fn keyring_persistence_ok() -> bool {
        matches!(
            keyring::default::default_credential_builder().persistence(),
            keyring::credential::CredentialPersistence::UntilDelete
        )
    }

    fn parse_hex_master_key(hex_str: &str) -> Option<[u8; 32]> {
        let bytes = hex::decode(hex_str.trim()).ok()?;
        if bytes.len() != 32 {
            return None;
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Some(key)
    }

    fn should_probe_legacy_keyring(
        keyring_hex: Option<&str>,
        persistent: bool,
        expectation: &KeyExpectation,
    ) -> bool {
        if !persistent {
            return false;
        }
        match keyring_hex.and_then(Self::parse_hex_master_key) {
            Some(key) => !Self::key_matches_expectation(&key, expectation),
            None => true,
        }
    }

    fn resolve_master_key_logic(
        keyring_hex: Option<&str>,
        legacy_hex: Option<&str>,
        file_hex: Option<&str>,
        keyring_persistent: bool,
        expectation: &KeyExpectation,
    ) -> Result<([u8; 32], KeySource), HostError> {
        // 收集全部候选（仅当钥匙串真正跨进程持久化时才信任其中的密钥），
        // 逐个用指纹/密文样本验证后选中匹配者；验证通过前不做任何删除/迁移
        let mut candidates: Vec<([u8; 32], KeySource)> = Vec::new();
        if keyring_persistent {
            if let Some(key) = keyring_hex.and_then(Self::parse_hex_master_key) {
                candidates.push((key, KeySource::Keyring));
            }
        }
        if let Some(key) = legacy_hex.and_then(Self::parse_hex_master_key) {
            candidates.push((key, KeySource::LegacyKeyring));
        }
        if let Some(key) = file_hex.and_then(Self::parse_hex_master_key) {
            candidates.push((key, KeySource::File));
        }

        let any_decrypts = !expectation.sample_blobs.is_empty()
            && candidates.iter().any(|(key, _)| {
                expectation.sample_blobs.iter().any(|(aad, blob)| {
                    crate::storage::crypto::decrypt_credential(key, aad, blob).is_ok()
                })
            });
        let any_fp_match = expectation.fingerprint.as_deref().is_some_and(|fp| {
            candidates
                .iter()
                .any(|(key, _)| crate::storage::crypto::key_fingerprint(key) == fp)
        });

        for &(key, source) in &candidates {
            if Self::key_matches_expectation(&key, expectation) {
                return Ok((key, source));
            }
        }

        if !expectation.requires_match() {
            if let Some(&(key, source)) = candidates.first() {
                return Ok((key, source));
            }
            let mut key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut key);
            return Ok((key, KeySource::Generated));
        }

        let message = if any_decrypts && expectation.fingerprint.is_some() {
            "检测到主密钥可以解密数据库凭据，但与本地 key_id 指纹文件冲突（可能是部分备份恢复遗留了旧 key_id）。\
             已保留全部密钥与数据文件并拒绝启动，请核对后清理错误的 key_id 或恢复一致的备份。"
        } else if any_fp_match && !expectation.sample_blobs.is_empty() {
            "检测到 key_id 指纹与候选主密钥匹配，但无法解密数据库中的抽样凭据（数据库可能与密钥备份来自不同时期或凭据已损坏）。\
             已保留全部文件并拒绝启动，请恢复对应时期的钥匙串条目或 master.key 备份。"
        } else {
            "检测到已有 Qwitch 加密数据，但没有可验证匹配的主密钥\
             （系统钥匙串、历史钥匙串及 master.key 均缺失或与数据库不匹配）。\
             已保留全部现有密钥文件并拒绝启动，请恢复正确的钥匙串条目或 master.key 备份后重试。"
        };
        Err(HostError::Keyring(message.into()))
    }

    fn key_matches_expectation(key: &[u8; 32], expectation: &KeyExpectation) -> bool {
        let fp_ok = expectation
            .fingerprint
            .as_deref()
            .map(|fp| crate::storage::crypto::key_fingerprint(key) == fp);
        let sample_ok = (!expectation.sample_blobs.is_empty()).then(|| {
            expectation.sample_blobs.iter().any(|(aad, blob)| {
                crate::storage::crypto::decrypt_credential(key, aad, blob).is_ok()
            })
        });
        match (fp_ok, sample_ok) {
            // 指纹与样本同时存在时必须两者都匹配，防止旧 key_id 选错钥
            (Some(f), Some(s)) => f && s,
            (Some(f), None) => f,
            (None, Some(s)) => s,
            // 无指纹也无样本（首次运行/空库）：接受候选
            (None, None) => true,
        }
    }

    #[cfg(test)]
    fn get_or_create_master_key_with(
        data_dir: &Path,
        persistent: bool,
    ) -> Result<[u8; 32], HostError> {
        Self::get_or_create_master_key_with_expectation(
            data_dir,
            persistent,
            &KeyExpectation::default(),
        )
    }

    fn get_or_create_master_key_with_expectation(
        data_dir: &Path,
        persistent: bool,
        expectation: &KeyExpectation,
    ) -> Result<[u8; 32], HostError> {
        let key_file = data_dir.join("master.key");

        // 原子写入明文密钥副本：临时文件（0600）→ fsync → rename。
        // 避免断电/崩溃留下截断的 master.key，进而让「唯一密钥源」不可用。
        let write_key_file = |path: &Path, hex_key: &str| -> Result<(), std::io::Error> {
            use std::io::Write;
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            let tmp = parent.join(format!(".master.key.tmp-{}", uuid::Uuid::new_v4()));
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let write_res = (|| -> std::io::Result<()> {
                let mut f = opts.open(&tmp)?;
                f.write_all(hex_key.as_bytes())?;
                f.sync_all()?;
                Ok(())
            })();
            if let Err(e) = write_res {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            if let Err(e) = std::fs::rename(&tmp, path) {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
            Ok(())
        };
        // 兜底镜像：钥匙串托管成功后**保留**一份 0600 明文副本，而不是删除。
        //
        // 原因：本项目暂不做 macOS 代码签名，每次覆盖升级都会让钥匙串 ACL 失效，
        // 用户若在弹窗中误点「拒绝」，或重装系统/清理钥匙串，主密钥将再无副本，
        // 全部账号凭据永久不可恢复（见 verify_or_init_key_fingerprint 的拒启分支）。
        // 保留镜像后，钥匙串失效时仍可正常启动；镜像本身仍受 0600 文件权限保护。
        let keep_backup_copy = |hex_key: &str| {
            if let Err(e) = write_key_file(&key_file, hex_key) {
                // 镜像写入失败不应阻断启动：钥匙串已是有效来源
                eprintln!("Qwitch 主密钥兜底副本写入失败（不影响本次启动）: {e}");
            }
        };
        let persist_in_keyring = |kr: &Entry, hex_key: &str| -> bool {
            if kr.set_password(hex_key.trim()).is_err() {
                return false;
            }
            kr.get_password()
                .ok()
                .map(|stored| stored.trim().eq_ignore_ascii_case(hex_key.trim()))
                .unwrap_or(false)
        };

        let entry = if persistent {
            Entry::new(KEYRING_SERVICE, KEYRING_MASTER_USER).ok()
        } else {
            None
        };
        let keyring_hex = entry.as_ref().and_then(|kr| kr.get_password().ok());
        // 当前钥匙串条目缺失、格式非法或无法通过指纹/样本验证时，探测历史条目（避免正确钥正常启动时多一次钥匙串访问）
        let legacy_hex =
            if Self::should_probe_legacy_keyring(keyring_hex.as_deref(), persistent, expectation) {
                Entry::new(LEGACY_KEYRING_SERVICE, KEYRING_MASTER_USER)
                    .ok()
                    .and_then(|kr| kr.get_password().ok())
            } else {
                None
            };
        let file_hex = std::fs::read_to_string(&key_file).ok();

        let (key, source) = Self::resolve_master_key_logic(
            keyring_hex.as_deref(),
            legacy_hex.as_deref(),
            file_hex.as_deref(),
            persistent,
            expectation,
        )?;
        let hex_key = hex::encode(key);

        match source {
            KeySource::Keyring => keep_backup_copy(&hex_key),
            KeySource::LegacyKeyring => {
                let migrated = entry
                    .as_ref()
                    .map(|kr| persist_in_keyring(kr, &hex_key))
                    .unwrap_or(false);
                if migrated {
                    keep_backup_copy(&hex_key);
                } else if write_key_file(&key_file, &hex_key).is_err() {
                    return Err(HostError::Keyring(
                        "历史钥匙串密钥迁移落盘失败，为避免重启后密钥不可用已中止启动".into(),
                    ));
                }
            }
            KeySource::File => {
                // 仅在钥匙串可靠持久化时才迁移并清理明文副本，否则保留文件作为唯一密钥源
                if let Some(kr) = entry.as_ref() {
                    if persist_in_keyring(kr, &hex_key) {
                        keep_backup_copy(&hex_key);
                    }
                }
            }
            KeySource::Generated => {
                let persisted = entry
                    .as_ref()
                    .map(|kr| persist_in_keyring(kr, &hex_key))
                    .unwrap_or(false);
                if persisted {
                    keep_backup_copy(&hex_key);
                } else if write_key_file(&key_file, &hex_key).is_err() {
                    return Err(HostError::Keyring(
                        "无法持久化主密钥到钥匙串或数据目录".into(),
                    ));
                }
            }
        }
        Ok(key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeySource {
    Keyring,
    LegacyKeyring,
    File,
    Generated,
}

/// 启动时用于验证候选主密钥是否匹配当前数据库的依据：
/// 优先 key_id 指纹 + 多条密文样本双验证；无指纹的旧库用样本试解密。
/// has_encrypted_data 区分"空库首装"与"已有加密数据"——
/// 若仅有孤儿 key_id 而数据库已空（无加密账号），允许恢复/重初始化而不阻塞启动
#[derive(Debug, Clone, Default)]
pub struct KeyExpectation {
    pub fingerprint: Option<String>,
    pub sample_blobs: Vec<crate::storage::crypto::AadBlob>,
    pub has_encrypted_data: bool,
}

impl KeyExpectation {
    fn requires_match(&self) -> bool {
        self.has_encrypted_data || !self.sample_blobs.is_empty()
    }
}

#[async_trait]
impl PlatformHost for NativePlatformHost {
    fn qwitch_data_dir(&self) -> Result<PathBuf, HostError> {
        let (dir, legacy_dir) = Self::resolve_data_dirs()?;
        if !dir.exists() {
            std::fs::create_dir_all(&dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        LEGACY_MIGRATED.call_once(|| {
            Self::migrate_legacy_data_dir(&legacy_dir, &dir);
        });
        Ok(dir)
    }

    fn client_config_dir(&self, tool_id: &str) -> Result<PathBuf, HostError> {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| HostError::OperationFailed("HOME not set".into()))?;
            let app_support = home.join("Library/Application Support");
            let path = match tool_id {
                "trae" => app_support.join("Trae/User/globalStorage"),
                "trae_cn" => app_support.join("Trae CN/User/globalStorage"),
                "workbuddy" => app_support.join("CodeBuddyExtension/Data/Public/auth"),
                "qoder" => find_latest_prefixed_dir(&app_support, "com.qoder.app")
                    .unwrap_or_else(|| app_support.join("com.qoder.app")),
                "qoder_cn" => find_latest_prefixed_dir(&app_support, "com.qodercn.app")
                    .unwrap_or_else(|| app_support.join("com.qodercn.app")),
                "minimax_code_cn" => app_support.join("MiniMax"),
                "lobsterai" => app_support.join("LobsterAI"),
                "doubaowork" => app_support.join("DoubaoWork"),
                // CodeArts 的 Electron userData 目录为小写 `codearts`
                "codearts" => app_support.join("codearts"),
                _ => self.qwitch_data_dir()?.join("clients").join(tool_id),
            };
            Ok(path)
        }
        #[cfg(target_os = "windows")]
        {
            let roaming = std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .ok_or_else(|| HostError::OperationFailed("APPDATA not set".into()))?;
            let local = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| roaming.clone());
            let path = match tool_id {
                "trae" => roaming.join("Trae").join("User").join("globalStorage"),
                "trae_cn" => roaming.join("Trae CN").join("User").join("globalStorage"),
                "workbuddy" => local
                    .join("CodeBuddyExtension")
                    .join("Data")
                    .join("Public")
                    .join("auth"),
                "qoder" => find_latest_prefixed_dir(&roaming, "com.qoder.app")
                    .unwrap_or_else(|| roaming.join("com.qoder.app")),
                "qoder_cn" => find_latest_prefixed_dir(&roaming, "com.qodercn.app")
                    .unwrap_or_else(|| roaming.join("com.qodercn.app")),
                "minimax_code_cn" => roaming.join("MiniMax"),
                "lobsterai" => roaming.join("LobsterAI"),
                "doubaowork" => local.join("DoubaoWork").join("User Data"),
                "codearts" => roaming.join("codearts"),
                _ => self.qwitch_data_dir()?.join("clients").join(tool_id),
            };
            Ok(path)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let base = self.qwitch_data_dir()?;
            Ok(base.join("clients").join(tool_id))
        }
    }

    fn detect_client_executable(&self, tool_id: &str) -> Option<PathBuf> {
        if let Some(custom) = self.read_custom_client_path(tool_id) {
            return Some(custom);
        }
        let spec = client_spec(tool_id)?;

        #[cfg(target_os = "macos")]
        {
            let mut roots = vec![PathBuf::from("/Applications")];
            if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
                roots.push(home.join("Applications"));
            }
            for root in roots {
                for app_name in spec.mac_app_names {
                    let bundle = root.join(app_name);
                    if bundle.is_dir() {
                        return Some(bundle);
                    }
                }
            }
            None
        }

        #[cfg(target_os = "windows")]
        {
            let mut bases = Vec::new();
            if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
                bases.push(local.join("Programs"));
            }
            if let Some(pf) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
                bases.push(pf);
            }
            if let Some(pf86) = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from) {
                bases.push(pf86);
            }
            for base in bases {
                for folder in spec.win_folder_names {
                    for exe in spec.win_exe_names {
                        let candidate = base.join(folder).join(exe);
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                    }
                }
            }
            None
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = spec;
            None
        }
    }

    fn is_client_running(&self, tool_id: &str) -> Result<bool, HostError> {
        let (primary_names, proc_names) = self.resolve_target_process_names(tool_id);
        Self::is_client_running_names(&primary_names, &proc_names)
    }

    /// 只要客户端 bundle 下存在任意进程即视为「进程存在」。
    ///
    /// 与 `is_client_running` 的区别是不要求能识别出主进程：
    /// Electron 打包客户端（WorkBuddy / Trae CN）的主二进制名为 `Electron`，
    /// 在极端情况下（如主进程刚启动尚未就绪）可能识别不出主进程，
    /// 但此时客户端进程确实已经拉起，不应据此回滚会话快照。
    fn client_process_present(&self, tool_id: &str) -> Result<bool, HostError> {
        #[cfg(target_os = "macos")]
        {
            let (primary_names, _) = self.resolve_target_process_names(tool_id);
            for app_name in &primary_names {
                if Self::is_macos_bundle_process_present(app_name) {
                    return Ok(true);
                }
            }
        }
        self.is_client_running(tool_id)
    }

    async fn terminate_client_gracefully(
        &self,
        tool_id: &str,
        timeout_ms: u64,
    ) -> Result<(), HostError> {
        let tid = tool_id.to_string();
        #[cfg(target_os = "macos")]
        let sidecar_names: Vec<&'static str> = client_spec(tool_id)
            .map(|s| s.mac_sidecar_process_names)
            .unwrap_or(&[])
            .to_vec();
        let (primary_names, proc_names) = tokio::task::spawn_blocking(move || {
            NativePlatformHost.resolve_target_process_names(&tid)
        })
        .await
        .map_err(|e| HostError::OperationFailed(e.to_string()))?;

        if primary_names.is_empty() && proc_names.is_empty() {
            return Ok(());
        }

        let check_running = |p_names: &[String], procs: &[String]| {
            let p_clone = p_names.to_vec();
            let proc_clone = procs.to_vec();
            async move {
                tokio::task::spawn_blocking(move || {
                    NativePlatformHost::is_client_running_names(&p_clone, &proc_clone)
                })
                .await
                .map_err(|e| HostError::OperationFailed(e.to_string()))?
            }
        };

        if !check_running(&primary_names, &proc_names).await? {
            // 主进程已不在，但可能仍残留 Renderer/GPU/utility 等辅助进程，
            // 它们会占用单实例锁/用户数据，导致随后的重启“秒退”，这里一并清理。
            #[cfg(target_os = "macos")]
            {
                cleanup_bundle_leftovers(&primary_names, 1200).await;
                // 主进程已不在，但它留下的网关子进程可能还占着数据目录
                stop_mac_sidecar_processes(&sidecar_names, 3000).await;
            }
            return Ok(());
        }

        // 是否有任何一次「优雅退出」请求真的发出去了。
        // osascript 在无 Automation 权限（或被沙箱拦截）时会立刻报 -10004，
        // 这时不该再傻等 3 秒优雅期，直接进入 TERM 阶段。
        #[allow(unused_mut)]
        let mut quit_requested = false;
        #[cfg(target_os = "macos")]
        {
            for app_name in &primary_names {
                let app_for_check = app_name.clone();
                let app_running = tokio::task::spawn_blocking(move || {
                    NativePlatformHost::is_macos_app_bundle_running(&app_for_check)
                })
                .await
                .unwrap_or(false);
                if !app_running {
                    continue;
                }
                let clean = app_name
                    .trim_end_matches(".app")
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"");
                if let Ok(status) = tokio::process::Command::new("osascript")
                    .args(["-e", &format!("tell application \"{clean}\" to quit")])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await
                {
                    if status.success() {
                        quit_requested = true;
                    }
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            for exe in &primary_names {
                let mut cmd = tokio::process::Command::new("taskkill");
                hide_console_tokio(&mut cmd);
                if let Ok(status) = cmd
                    .args(["/IM", exe])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await
                {
                    if status.success() {
                        quit_requested = true;
                    }
                }
            }
        }

        if quit_requested {
            // 优雅退出请求已送达：给它一段收尾时间（关闭窗口/落盘/销毁 WebView）
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1000));
            while Instant::now() < deadline {
                if !check_running(&primary_names, &proc_names).await? {
                    // 额外等待 300ms 确保文件锁完全释放
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    #[cfg(target_os = "macos")]
                    {
                        cleanup_bundle_leftovers(&primary_names, 2500).await;
                        stop_mac_sidecar_processes(&sidecar_names, 3000).await;
                    }
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }

        // 超时后尝试温和信号或强杀兜底
        #[cfg(target_os = "macos")]
        {
            for app_name in &primary_names {
                let needle = format!("/{}/Contents/MacOS/", escape_pgrep_regex(app_name));
                let _ = tokio::process::Command::new("pkill")
                    .args(["-TERM", "-f", &needle])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
            for proc_name in &proc_names {
                if proc_name.len() <= 15 {
                    let _ = tokio::process::Command::new("pkill")
                        .args(["-TERM", "-x", proc_name])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .await;
                } else {
                    let needle = format!("/Contents/MacOS/{}", escape_pgrep_regex(proc_name));
                    let _ = tokio::process::Command::new("pkill")
                        .args(["-TERM", "-f", &needle])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .await;
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            for exe in &primary_names {
                let mut cmd = tokio::process::Command::new("taskkill");
                hide_console_tokio(&mut cmd);
                let _ = cmd
                    .args(["/F", "/IM", exe])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
        }

        // TERM 之后不能只等固定 500ms：Electron 客户端收到 SIGTERM 还要走
        // 「关闭窗口 → 清理 → 退出」的收尾流程，实测豆包要 ~1.5s。
        // 旧实现 500ms 后就判定「仍在运行」直接报错，于是豆包/龙虾这类客户端
        // 每次切号都失败，UI 上表现为「切号卡死」。
        if wait_until_stopped(&primary_names, &proc_names, 2500).await? {
            #[cfg(target_os = "macos")]
            {
                cleanup_bundle_leftovers(&primary_names, 2500).await;
                stop_mac_sidecar_processes(&sidecar_names, 3000).await;
            }
            return Ok(());
        }

        // 最后兜底：强杀（SIGKILL 不可被忽略）
        #[cfg(target_os = "macos")]
        {
            for app_name in &primary_names {
                let needle = format!("/{}/Contents/", escape_pgrep_regex(app_name));
                let _ = tokio::process::Command::new("pkill")
                    .args(["-KILL", "-f", &needle])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
            for proc_name in &proc_names {
                let _ = tokio::process::Command::new("pkill")
                    .args([
                        "-KILL",
                        "-f",
                        &format!("/Contents/MacOS/{}", escape_pgrep_regex(proc_name)),
                    ])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
        }
        #[cfg(target_os = "windows")]
        {
            for exe in &primary_names {
                let mut cmd = tokio::process::Command::new("taskkill");
                hide_console_tokio(&mut cmd);
                let _ = cmd
                    .args(["/F", "/T", "/IM", exe])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
        }
        if wait_until_stopped(&primary_names, &proc_names, 2000).await? {
            #[cfg(target_os = "macos")]
            {
                cleanup_bundle_leftovers(&primary_names, 2500).await;
                stop_mac_sidecar_processes(&sidecar_names, 3000).await;
            }
            return Ok(());
        }
        Err(HostError::OperationFailed(format!(
            "无法关闭正在运行的 {tool_id} 客户端，请手动退出后重试"
        )))
    }

    fn launch_client(&self, executable: &Path, args: &[String]) -> Result<(), HostError> {
        // 关键：避免把宿主进程的 Electron/Node 环境泄漏给被启动的客户端。
        // 例如从某个 Electron/Node 环境（如开发工具终端）启动 Qwitch 时，
        // 环境里可能带有 ELECTRON_RUN_AS_NODE=1，若传给客户端的 Electron 主进程，
        // 它会以 Node 模式启动并立刻 exit(0)（表现为“重启后秒退/切号失败”）。
        #[cfg(target_os = "macos")]
        {
            if executable.extension().and_then(|e| e.to_str()) == Some("app") {
                let mut cmd = Command::new("open");
                cmd.env_remove("ELECTRON_RUN_AS_NODE");
                cmd.env_remove("NODE_OPTIONS");
                if !args.is_empty() {
                    cmd.arg("-n");
                }
                cmd.arg("-a").arg(executable);
                if !args.is_empty() {
                    cmd.arg("--args").args(args);
                }
                let status = cmd.status()?;
                if !status.success() {
                    return Err(HostError::OperationFailed(format!(
                        "启动客户端失败 (open exit {}): {}",
                        status,
                        executable.display()
                    )));
                }
                return Ok(());
            }
        }

        let mut child = Command::new(executable)
            .env_remove("ELECTRON_RUN_AS_NODE")
            .env_remove("NODE_OPTIONS")
            .args(args)
            .spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn get_or_create_master_key(
        &self,
        expectation: &KeyExpectation,
    ) -> Result<[u8; 32], HostError> {
        let data_dir = self
            .qwitch_data_dir()
            .map_err(|e| HostError::Keyring(format!("无法解析数据目录，不能托管主密钥: {e}")))?;
        Self::get_or_create_master_key_with_expectation(
            &data_dir,
            Self::keyring_persistence_ok(),
            expectation,
        )
    }

    fn set_launch_at_login(&self, enabled: bool) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| "无法读取 HOME 目录，不能配置 macOS 开机自启".to_string())?;
            let agents_dir = home.join("Library").join("LaunchAgents");
            let plist_path = agents_dir.join("dev.qwitch.app.plist");
            let bootout = || {
                if let Some(uid) = macos_current_uid() {
                    let _ = Command::new("launchctl")
                        .args(["bootout", &format!("gui/{uid}/dev.qwitch.app")])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            };
            if enabled {
                let exe =
                    std::env::current_exe().map_err(|e| format!("无法获取当前程序路径: {e}"))?;
                // 仅 .app 包支持真正的开机自启；开发模式(裸二进制、需 Vite)会拉起坏实例，
                // 直接拒绝并清理可能遗留的坏自启。
                if macos_app_bundle(&exe).is_none() {
                    let _ = std::fs::remove_file(&plist_path);
                    bootout();
                    return Err(
                        "当前为开发模式（非 .app 包），不支持开机自启；请用打包后的 Qwitch.app 开启"
                            .to_string(),
                    );
                }
                std::fs::create_dir_all(&agents_dir)
                    .map_err(|e| format!("创建 LaunchAgents 目录失败: {e}"))?;
                let plist = render_macos_launch_agent_plist(&exe);
                std::fs::write(&plist_path, plist.as_bytes())
                    .map_err(|e| format!("写入开机自启配置失败: {e}"))?;
                // 旧 job 若已加载，先卸下，使下次登录按新 plist 生效
                bootout();
                Ok(())
            } else {
                if plist_path.exists() {
                    std::fs::remove_file(&plist_path)
                        .map_err(|e| format!("移除开机自启配置失败: {e}"))?;
                }
                bootout();
                Ok(())
            }
        }

        #[cfg(target_os = "windows")]
        {
            const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
            if enabled {
                let exe =
                    std::env::current_exe().map_err(|e| format!("无法获取当前程序路径: {e}"))?;
                let value = format!("\"{}\" --autostart", exe.to_string_lossy());
                let mut cmd = Command::new("reg");
                hide_console_std(&mut cmd);
                let output = cmd
                    .args([
                        "add", RUN_KEY, "/v", "Qwitch", "/t", "REG_SZ", "/d", &value, "/f",
                    ])
                    .output()
                    .map_err(|e| format!("执行 reg add 失败: {e}"))?;
                if !output.status.success() {
                    return Err(format!(
                        "写入 Windows 开机自启注册表失败: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
            } else {
                let mut cmd = Command::new("reg");
                hide_console_std(&mut cmd);
                let _ = cmd
                    .args(["delete", RUN_KEY, "/v", "Qwitch", "/f"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            Ok(())
        }

        #[cfg(target_os = "linux")]
        {
            let base = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .ok_or_else(|| "无法定位用户配置目录，不能配置 Linux 开机自启".to_string())?;
            let autostart_dir = base.join("autostart");
            let desktop_path = autostart_dir.join("dev.qwitch.app.desktop");
            if enabled {
                let exe =
                    std::env::current_exe().map_err(|e| format!("无法获取当前程序路径: {e}"))?;
                std::fs::create_dir_all(&autostart_dir)
                    .map_err(|e| format!("创建 autostart 目录失败: {e}"))?;
                let escaped_exe = exe
                    .to_string_lossy()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('`', "\\`")
                    .replace('$', "\\$")
                    .replace('%', "%%");
                let content = format!(
                    "[Desktop Entry]\nType=Application\nName=Qwitch\nExec=\"{escaped_exe}\" --autostart\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
                );
                std::fs::write(&desktop_path, content.as_bytes())
                    .map_err(|e| format!("写入开机自启配置失败: {e}"))?;
            } else if desktop_path.exists() {
                std::fs::remove_file(&desktop_path)
                    .map_err(|e| format!("移除开机自启配置失败: {e}"))?;
            }
            Ok(())
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = enabled;
            Ok(())
        }
    }
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn render_macos_launch_agent_plist(exe_path: &Path) -> String {
    // 若运行在 .app 包内，用 `open -a <bundle> --args --autostart` 启动：
    // 走 LaunchServices 正常唤起 App（正确的激活策略/Dock/托盘），且 App 升级/移动后
    // 只要 bundle 路径不变仍有效；否则退回直接执行当前可执行文件（仅开发/直跑场景）。
    let program_args = match macos_app_bundle(exe_path) {
        Some(bundle) => format!(
            "    <string>/usr/bin/open</string>\n    <string>-a</string>\n    <string>{}</string>\n    <string>--args</string>\n    <string>--autostart</string>",
            escape_xml(&bundle.to_string_lossy())
        ),
        None => format!(
            "    <string>{}</string>\n    <string>--autostart</string>",
            escape_xml(&exe_path.to_string_lossy())
        ),
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.qwitch.app</string>
  <key>ProgramArguments</key>
  <array>
{program_args}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>ProcessType</key>
  <string>Interactive</string>
</dict>
</plist>
"#
    )
}

/// 从可执行文件路径向上查找所属的 `.app` bundle（macOS）。
fn macos_app_bundle(exe_path: &Path) -> Option<PathBuf> {
    for anc in exe_path.ancestors() {
        if anc.extension().and_then(|e| e.to_str()) == Some("app") {
            return Some(anc.to_path_buf());
        }
    }
    None
}

/// 当前登录用户的 uid（用于 launchctl gui/<uid> 域操作）。
#[cfg(target_os = "macos")]
fn macos_current_uid() -> Option<String> {
    let out = Command::new("id").arg("-u").output().ok()?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!uid.is_empty()).then_some(uid)
}

fn find_latest_prefixed_dir(parent: &Path, prefix: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(parent).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with(prefix) {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        match &best {
            Some((best_time, _)) if *best_time >= mtime => {}
            _ => best = Some((mtime, path)),
        }
    }

    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// 空进程名列表必须立刻判定「已停止」，不能进轮询空转。
    #[tokio::test]
    async fn wait_until_stopped_returns_immediately_without_processes() {
        let started = std::time::Instant::now();
        assert!(wait_until_stopped(&[], &[], 2_000).await.unwrap());
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn macos_launch_agent_plist_bundle_aware_and_escaped() {
        // .app 包内：走 `open -a <bundle> --args --autostart`，bundle 路径做 XML 转义
        let exe = PathBuf::from("/Applications/Qwitch & Tools.app/Contents/MacOS/qwitch");
        let plist = render_macos_launch_agent_plist(&exe);
        assert!(plist.contains("<string>dev.qwitch.app</string>"));
        assert!(plist.contains("<string>/usr/bin/open</string>"));
        assert!(plist.contains("<string>/Applications/Qwitch &amp; Tools.app</string>"));
        assert!(plist.contains("<string>--autostart</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(plist.contains("<key>LimitLoadToSessionType</key>\n  <string>Aqua</string>"));

        // 非 bundle（开发/直跑）：退回直接执行当前可执行文件
        let dev = PathBuf::from("/Users/x/dev/qwitch");
        let p2 = render_macos_launch_agent_plist(&dev);
        assert!(p2.contains("<string>/Users/x/dev/qwitch</string>"));
        assert!(p2.contains("<string>--autostart</string>"));
    }

    #[test]
    fn master_key_resolution_decision_matrix() {
        let key = [9u8; 32];
        let hex_key = hex::encode(key);
        let wrong_key = [8u8; 32];
        let wrong_hex = hex::encode(wrong_key);
        let no_exp = KeyExpectation::default();
        let resolve = |kr: Option<&str>,
                       legacy: Option<&str>,
                       file: Option<&str>,
                       db: bool,
                       persistent: bool,
                       exp: &KeyExpectation| {
            let mut e = exp.clone();
            e.has_encrypted_data = e.has_encrypted_data || db;
            NativePlatformHost::resolve_master_key_logic(kr, legacy, file, persistent, &e)
        };

        // 已有数据库且所有密钥源不可用：绝不生成新密钥
        assert!(resolve(None, None, None, true, true, &no_exp).is_err());
        assert!(resolve(None, None, None, true, false, &no_exp).is_err());
        // 首次运行才允许生成
        let (_, src) = resolve(None, None, None, false, true, &no_exp).expect("first run");
        assert_eq!(src, KeySource::Generated);
        // 无验证依据时持久化钥匙串优先
        let (k, src) = resolve(Some(&hex_key), None, None, true, true, &no_exp).expect("keyring");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::Keyring);
        // 非持久化（mock）钥匙串不可信，即使有值也回退文件
        let (k, src) =
            resolve(Some(&hex_key), None, Some(&hex_key), true, false, &no_exp).expect("file");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::File);
        // 旧版 Quiver 钥匙串迁移
        let (_, src) = resolve(None, Some(&hex_key), None, true, true, &no_exp).expect("legacy");
        assert_eq!(src, KeySource::LegacyKeyring);
        // 文件迁移
        let (_, src) =
            resolve(None, None, Some(&hex_key), true, true, &no_exp).expect("file migrate");
        assert_eq!(src, KeySource::File);
        // 非法 hex 长度跳过，落到下一来源
        let (_, src) =
            resolve(Some("abcd"), None, Some(&hex_key), true, true, &no_exp).expect("skip bad");
        assert_eq!(src, KeySource::File);

        // 冲突来源：钥匙串是旧钥、文件才是匹配 key_id 的正确钥 → 选文件
        let fp_exp = KeyExpectation {
            fingerprint: Some(crate::storage::crypto::key_fingerprint(&key)),
            sample_blobs: Vec::new(),
            has_encrypted_data: true,
        };
        let (k, src) = resolve(Some(&wrong_hex), None, Some(&hex_key), true, true, &fp_exp)
            .expect("conflict picks file");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::File);

        // 冲突来源：钥匙串匹配指纹 → 选钥匙串（即使文件也存在）
        let (k, src) = resolve(Some(&hex_key), None, Some(&wrong_hex), true, true, &fp_exp)
            .expect("conflict picks keyring");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::Keyring);

        // 全部候选都不匹配指纹 → 拒绝（保留文件交由调用方处理）
        assert!(resolve(
            Some(&wrong_hex),
            None,
            Some(&wrong_hex),
            true,
            true,
            &fp_exp
        )
        .is_err());

        // 孤儿 key_id + 空库（无任何加密账号）：不阻塞启动，复用候选密钥
        let orphan_fp_empty = KeyExpectation {
            fingerprint: Some(crate::storage::crypto::key_fingerprint(&wrong_key)),
            sample_blobs: Vec::new(),
            has_encrypted_data: false,
        };
        let (k, src) =
            resolve(Some(&hex_key), None, None, false, true, &orphan_fp_empty).expect("orphan fp");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::Keyring);

        // 无指纹旧库：用多条密文样本验证（第一条损坏时第二条能解密仍应匹配）
        let env = crate::domain::CredentialEnvelope {
            tool_id: "qoder_cn".to_string(),
            uid: "sample_uid".to_string(),
            display_name: None,
            display_email: None,
            payload: crate::domain::CredentialPayload::Token {
                access_token: "sample-token".to_string(),
                refresh_token: None,
                expires_at: None,
                extra: std::collections::HashMap::new(),
            },
        };
        let aad = crate::storage::crypto::credential_aad("qoder_cn", "acc-sample");
        let blob = crate::storage::crypto::encrypt_credential(&key, &aad, &env).expect("encrypt");
        let corrupt_blob = vec![1u8; 32];
        let sample_exp = KeyExpectation {
            fingerprint: None,
            sample_blobs: vec![(aad.clone(), corrupt_blob), (aad.clone(), blob.clone())],
            has_encrypted_data: true,
        };
        let (k, src) = resolve(
            Some(&wrong_hex),
            None,
            Some(&hex_key),
            true,
            true,
            &sample_exp,
        )
        .expect("multi-sample picks file even when first blob corrupt");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::File);
        assert!(resolve(Some(&wrong_hex), None, None, true, true, &sample_exp).is_err());

        // 双验证：指纹与样本都指向正确钥 B 时，旧钥 A（钥匙串）落选、B（文件）选中
        let dual_exp = KeyExpectation {
            fingerprint: Some(crate::storage::crypto::key_fingerprint(&key)),
            sample_blobs: vec![(aad.clone(), blob.clone())],
            has_encrypted_data: true,
        };
        let (k, src) = resolve(
            Some(&wrong_hex),
            None,
            Some(&hex_key),
            true,
            true,
            &dual_exp,
        )
        .expect("dual verification picks file");
        assert_eq!(k, key);
        assert_eq!(src, KeySource::File);
        // 只有旧钥 A（指纹/样本都不匹配）→ 拒绝
        assert!(resolve(Some(&wrong_hex), None, None, true, true, &dual_exp).is_err());

        // 指纹与样本互相矛盾（部分备份恢复）：任何候选都无法同时通过 → Fail-Closed
        let split_exp = KeyExpectation {
            fingerprint: Some(crate::storage::crypto::key_fingerprint(&wrong_key)),
            sample_blobs: sample_exp.sample_blobs.clone(),
            has_encrypted_data: true,
        };
        assert!(resolve(
            Some(&wrong_hex),
            None,
            Some(&hex_key),
            true,
            true,
            &split_exp
        )
        .is_err());
    }

    struct FakeCredential {
        id: String,
    }

    fn fake_store() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
        static STORE: std::sync::OnceLock<
            std::sync::Mutex<std::collections::HashMap<String, String>>,
        > = std::sync::OnceLock::new();
        STORE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
    }

    impl keyring::credential::CredentialApi for FakeCredential {
        fn set_secret(&self, password: &[u8]) -> keyring::Result<()> {
            fake_store().lock().unwrap().insert(
                self.id.clone(),
                String::from_utf8_lossy(password).to_string(),
            );
            Ok(())
        }
        fn get_secret(&self) -> keyring::Result<Vec<u8>> {
            fake_store()
                .lock()
                .unwrap()
                .get(&self.id)
                .map(|s| s.clone().into_bytes())
                .ok_or(keyring::Error::NoEntry)
        }
        fn delete_credential(&self) -> keyring::Result<()> {
            fake_store()
                .lock()
                .unwrap()
                .remove(&self.id)
                .map(|_| ())
                .ok_or(keyring::Error::NoEntry)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct FakePersistentBuilder;

    impl keyring::credential::CredentialBuilderApi for FakePersistentBuilder {
        fn build(
            &self,
            _target: Option<&str>,
            service: &str,
            user: &str,
        ) -> keyring::Result<Box<keyring::Credential>> {
            Ok(Box::new(FakeCredential {
                id: format!("{service}:{user}"),
            }))
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn persistence(&self) -> keyring::credential::CredentialPersistence {
            keyring::credential::CredentialPersistence::UntilDelete
        }
    }

    fn temp_data_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qwitch_keytest_{tag}_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    // 全局 credential builder 是进程级状态，所有涉及 builder 切换的场景
    // 必须在单个测试内串行执行，避免与其他测试竞态。
    // 注意：set_default_credential_builder 只影响 Entry::new，不影响
    // default::default_credential_builder()，因此持久化标志经参数显式传入。
    #[test]
    fn master_key_restart_regression_and_persistence_guards() {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert!(
            NativePlatformHost::keyring_persistence_ok(),
            "启用原生 feature 后平台默认钥匙串必须跨进程持久化"
        );

        // --- 场景 0：生产顺序首装回归 — open 先创建空 DB，随后选钥必须成功生成 ---
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let dir = temp_data_dir("firstboot");
        let db =
            crate::storage::QwitchDatabase::open(&dir.join("qwitch.db")).expect("open creates db");
        assert!(dir.join("qwitch.db").exists());
        let samples = db.credential_sample_blobs(5).expect("query ok");
        assert!(samples.is_empty());
        let first_exp = KeyExpectation {
            fingerprint: None,
            sample_blobs: samples,
            has_encrypted_data: false,
        };
        let first_key =
            NativePlatformHost::get_or_create_master_key_with_expectation(&dir, false, &first_exp)
                .expect("空库首装必须成功生成密钥");
        assert!(dir.join("master.key").exists());
        db.verify_or_init_key_fingerprint(&dir, &first_key)
            .expect("bless first key");
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 1：mock（非持久化）钥匙串下重启不得丢钥、不得删明文副本 ---
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());

        let dir = temp_data_dir("mock");
        let key1 =
            NativePlatformHost::get_or_create_master_key_with(&dir, false).expect("first boot");
        assert!(
            dir.join("master.key").exists(),
            "mock 环境必须保留明文密钥文件"
        );

        // 模拟已有数据库
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        // 模拟重启：新 Entry 的 mock 存储为空
        let key2 = NativePlatformHost::get_or_create_master_key_with(&dir, false).expect("restart");
        assert_eq!(key1, key2, "重启后必须取回同一主密钥");
        assert!(dir.join("master.key").exists(), "重启后明文副本仍须保留");
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 2：已有加密数据但密钥源全失 → 拒绝启动而非生成新钥 ---
        let dir = temp_data_dir("lost");
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        let lost_exp = KeyExpectation {
            has_encrypted_data: true,
            ..Default::default()
        };
        assert!(
            NativePlatformHost::get_or_create_master_key_with_expectation(&dir, false, &lost_exp)
                .is_err()
        );
        assert!(!dir.join("master.key").exists());
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 3：真正持久化的钥匙串 → 托管成功且不落明文 ---
        fake_store().lock().unwrap().clear();
        keyring::set_default_credential_builder(Box::new(FakePersistentBuilder));

        let dir = temp_data_dir("persistent");
        let key1 =
            NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("first boot");
        // 钥匙串托管成功后仍保留 0600 兜底镜像：本项目不做代码签名，
        // 覆盖升级会使钥匙串 ACL 失效，无镜像则密钥丢失即凭据永久不可恢复。
        assert!(
            dir.join("master.key").exists(),
            "持久化钥匙串应保留 0600 兜底镜像"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("master.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "兜底镜像必须是 0600，实际 {mode:o}");
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("master.key"))
                .unwrap()
                .trim(),
            hex::encode(key1),
            "兜底镜像必须与钥匙串中的密钥一致"
        );
        let key2 = NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("restart");
        assert_eq!(key1, key2);
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 4：遗留 master.key 迁移到可靠钥匙串后才清理 ---
        fake_store().lock().unwrap().clear();
        let dir = temp_data_dir("migrate");
        let legacy_key = [5u8; 32];
        std::fs::write(dir.join("master.key"), hex::encode(legacy_key)).unwrap();
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        let key = NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("migrate");
        assert_eq!(key, legacy_key);
        assert_eq!(
            std::fs::read_to_string(dir.join("master.key"))
                .unwrap()
                .trim(),
            hex::encode(legacy_key),
            "迁移成功后兜底镜像内容须与迁移的密钥一致"
        );
        let key_again =
            NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("restart");
        assert_eq!(key_again, legacy_key);
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 4b：钥匙串丢失（模拟覆盖升级后 ACL 失效/重装系统）→ 兜底镜像救回启动 ---
        fake_store().lock().unwrap().clear();
        let dir = temp_data_dir("keyring_lost_backup");
        let key_first =
            NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("first boot");
        let backup_hex = std::fs::read_to_string(dir.join("master.key"))
            .expect("兜底镜像必须存在")
            .trim()
            .to_string();
        assert_eq!(backup_hex, hex::encode(key_first));

        // 模拟钥匙串条目消失（用户误点拒绝 / 系统重装 / ACL 失效）
        fake_store().lock().unwrap().clear();

        let key_recovered = NativePlatformHost::get_or_create_master_key_with(&dir, true)
            .expect("recover from backup");
        assert_eq!(
            key_recovered, key_first,
            "钥匙串丢失后必须从兜底镜像恢复同一主密钥，而不是生成新钥"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 5：旧版 Quiver 钥匙串迁移 ---
        fake_store().lock().unwrap().clear();
        let dir = temp_data_dir("quiver");
        let quiver_key = [6u8; 32];
        fake_store().lock().unwrap().insert(
            format!("{LEGACY_KEYRING_SERVICE}:{KEYRING_MASTER_USER}"),
            hex::encode(quiver_key),
        );
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        let key =
            NativePlatformHost::get_or_create_master_key_with(&dir, true).expect("quiver migrate");
        assert_eq!(key, quiver_key);
        let stored = fake_store()
            .lock()
            .unwrap()
            .get(&format!("{KEYRING_SERVICE}:{KEYRING_MASTER_USER}"))
            .cloned();
        assert_eq!(stored, Some(hex::encode(quiver_key)));
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 6：钥匙串残留旧钥 + master.key 是正确钥 + key_id 指向正确钥 ---
        // 必须选中文件密钥、修复钥匙串，且不得在验证前删除正确文件
        fake_store().lock().unwrap().clear();
        let dir = temp_data_dir("conflict");
        let correct_key = [7u8; 32];
        let stale_key = [4u8; 32];
        fake_store().lock().unwrap().insert(
            format!("{KEYRING_SERVICE}:{KEYRING_MASTER_USER}"),
            hex::encode(stale_key),
        );
        std::fs::write(dir.join("master.key"), hex::encode(correct_key)).unwrap();
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        std::fs::write(
            dir.join("key_id"),
            crate::storage::crypto::key_fingerprint(&correct_key),
        )
        .unwrap();
        let expectation = KeyExpectation {
            fingerprint: Some(crate::storage::crypto::key_fingerprint(&correct_key)),
            sample_blobs: Vec::new(),
            has_encrypted_data: true,
        };
        let key =
            NativePlatformHost::get_or_create_master_key_with_expectation(&dir, true, &expectation)
                .expect("conflict resolves to file key");
        assert_eq!(key, correct_key);
        let repaired = fake_store()
            .lock()
            .unwrap()
            .get(&format!("{KEYRING_SERVICE}:{KEYRING_MASTER_USER}"))
            .cloned();
        assert_eq!(
            repaired,
            Some(hex::encode(correct_key)),
            "钥匙串应被修复为正确钥"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 7：所有候选都不匹配 → 拒绝启动且保留全部密钥文件 ---
        fake_store().lock().unwrap().clear();
        let dir = temp_data_dir("allwrong");
        fake_store().lock().unwrap().insert(
            format!("{KEYRING_SERVICE}:{KEYRING_MASTER_USER}"),
            hex::encode(stale_key),
        );
        std::fs::write(dir.join("master.key"), hex::encode(stale_key)).unwrap();
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        std::fs::write(
            dir.join("key_id"),
            crate::storage::crypto::key_fingerprint(&correct_key),
        )
        .unwrap();
        assert!(
            NativePlatformHost::get_or_create_master_key_with_expectation(&dir, true, &expectation)
                .is_err()
        );
        assert!(
            dir.join("master.key").exists(),
            "不匹配时不得删除任何候选密钥文件"
        );

        let _ = std::fs::remove_dir_all(&dir);

        // --- 场景 8：新钥匙串存有错钥，但历史 Quiver 钥匙串存有正确钥 → 触发回退并选出历史钥 ---
        let legacy_hex = hex::encode(correct_key);
        let stale_hex = hex::encode(stale_key);
        assert!(
            !NativePlatformHost::should_probe_legacy_keyring(Some(&legacy_hex), true, &expectation),
            "已匹配当前钥匙串条目时不应额外探测历史条目"
        );
        assert!(
            NativePlatformHost::should_probe_legacy_keyring(Some(&stale_hex), true, &expectation),
            "当前钥匙串条目不匹配时必须允许探测历史条目"
        );
        fake_store().lock().unwrap().clear();
        let dir = temp_data_dir("legacy_fallback");
        fake_store().lock().unwrap().insert(
            format!("{KEYRING_SERVICE}:{KEYRING_MASTER_USER}"),
            stale_hex.clone(),
        );
        fake_store().lock().unwrap().insert(
            format!("{LEGACY_KEYRING_SERVICE}:{KEYRING_MASTER_USER}"),
            legacy_hex.clone(),
        );
        std::fs::write(dir.join("qwitch.db"), b"placeholder").unwrap();
        std::fs::write(
            dir.join("key_id"),
            crate::storage::crypto::key_fingerprint(&correct_key),
        )
        .unwrap();
        let recovered =
            NativePlatformHost::get_or_create_master_key_with_expectation(&dir, true, &expectation)
                .expect("legacy keyring fallback should recover correct key");
        assert_eq!(recovered, correct_key);
        let _ = std::fs::remove_dir_all(&dir);

        // 恢复 mock，避免影响后续可能使用 keyring 的测试
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    }
}

#[cfg(test)]
mod client_executable_validation_tests {
    use super::validate_client_executable;
    use std::path::{Path, PathBuf};

    #[test]
    fn rejects_paths_outside_trusted_install_roots_and_wrong_names() {
        // 指向系统二进制：名称不匹配即拒绝
        assert!(validate_client_executable("trae_cn", Path::new("/bin/sh")).is_err());
        assert!(validate_client_executable("workbuddy", Path::new("/bin/echo")).is_err());

        // 未知工具
        assert!(
            validate_client_executable("not_a_tool", Path::new("/Applications/X.app")).is_err()
        );

        // 名称正确但不在受信任目录：拒绝
        let tmp_app = std::env::temp_dir().join("Trae CN.app");
        let _ = std::fs::create_dir_all(tmp_app.join("Contents/MacOS"));
        let _ = std::fs::write(tmp_app.join("Contents/MacOS/Trae CN"), b"#!/bin/sh\n");
        // /tmp 不在 /Applications、~/Applications 或 HOME 之下（macOS 上 /tmp 是 /private/tmp）
        assert!(
            validate_client_executable("trae_cn", &tmp_app).is_err(),
            "非受信任安装目录内的同名 bundle 必须被拒绝"
        );
        let _ = std::fs::remove_dir_all(&tmp_app);
    }

    #[test]
    fn accepts_real_installed_clients_when_present() {
        // 白名单不得过严：本机真实安装的客户端必须通过校验
        let cases: &[(&str, &str)] = &[
            ("trae_cn", "/Applications/Trae CN.app"),
            ("workbuddy", "/Applications/WorkBuddy.app"),
            ("qoder_cn", "/Applications/Qoder CN.app"),
            ("minimax_code_cn", "/Applications/MiniMax Code.app"),
            ("lobsterai", "/Applications/LobsterAI.app"),
            ("doubaowork", "/Applications/DoubaoWork.app"),
        ];
        let mut checked = 0usize;
        for (tool, path) in cases {
            let p = PathBuf::from(path);
            if !p.exists() {
                continue;
            }
            checked += 1;
            validate_client_executable(tool, &p)
                .unwrap_or_else(|e| panic!("真实客户端 {path} 应通过校验，实际被拒: {e}"));
        }
        if checked == 0 {
            eprintln!("跳过：本机未安装任何受支持客户端");
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_process_detection_tests {
    use super::{NativePlatformHost, PlatformHost};

    /// 复刻线上误判场景：Qoder 官方客户端注册的 Chrome Native Messaging Host
    /// 命令行形如 `<主二进制> <Resources>/native-messaging-host/qoder-app-host.cjs`。
    /// 该进程在 Chrome 运行期间常驻，曾被误判为「客户端正在运行」。
    #[test]
    fn native_messaging_host_is_treated_as_helper_not_main_process() {
        let rest = "/Applications/Qoder CN.app/Contents/Resources/native-messaging-host/qoder-app-host.cjs";
        assert!(
            NativePlatformHost::is_macos_helper_argv(rest),
            "Native Messaging Host 必须被识别为辅助进程"
        );
    }

    #[test]
    fn electron_renderer_and_gpu_helpers_are_excluded() {
        for rest in [
            "--type=renderer --enable-sandbox",
            "--type=gpu-process",
            "--type=utility --utility-sub-type=network.mojom.NetworkService",
            "/Applications/X.app/Contents/Frameworks/X Helper.app/Contents/MacOS/X Helper",
            "--product-version=1.2.3 --type=crashpad-handler",
        ] {
            assert!(
                NativePlatformHost::is_macos_helper_argv(rest),
                "辅助进程参数应被排除：{rest}"
            );
        }
    }

    #[test]
    fn main_process_argv_forms_are_not_excluded() {
        // 无额外参数：标准主进程
        assert!(!NativePlatformHost::is_macos_helper_argv(""));
        // 常见 Electron 启动参数，仍属主进程
        assert!(!NativePlatformHost::is_macos_helper_argv("--no-sandbox"));
        // 打开工程路径（Trae/Qoder 常见），仍属主进程
        assert!(!NativePlatformHost::is_macos_helper_argv(
            "/Users/me/project"
        ));
        assert!(!NativePlatformHost::is_macos_helper_argv(
            "--no-sandbox /Users/me/my-workspace"
        ));
    }

    /// `ps` 不可用时退回复用 `pgrep -fl` 的输出。它的格式是
    /// 「pid + 完整命令行」，与 `ps -axo pid=,command=` 同构，
    /// 因此必须走同一套判定：残留的 native-messaging-host 不能被当成主进程。
    #[test]
    fn pgrep_fl_output_is_judged_like_ps_output() {
        let pgrep_fl = "22324 /Applications/Qoder CN.app/Contents/MacOS/Qoder CN /Applications/Qoder CN.app/Contents/Resources/native-messaging-host/qoder-app-host.cjs";
        assert!(
            !NativePlatformHost::parse_macos_ps_has_main_process(pgrep_fl, "Qoder CN.app"),
            "仅剩 Native Messaging Host 时必须判定为未运行"
        );

        let pgrep_main = "22325 /Applications/Qoder CN.app/Contents/MacOS/Qoder CN";
        assert!(
            NativePlatformHost::parse_macos_ps_has_main_process(pgrep_main, "Qoder CN.app"),
            "主进程存在时应判定为运行中"
        );
    }

    #[test]
    fn parse_ps_output_handles_install_root_prefix_and_spaces() {
        // 标准 /Applications 与 ~/Applications 安装路径应能正确识别主进程
        let ps_main = "\
  101 /Applications/Trae CN.app/Contents/MacOS/Trae CN
  102 /Applications/Qoder CN.app/Contents/MacOS/Qoder CN --type=renderer
";
        assert!(NativePlatformHost::parse_macos_ps_has_main_process(
            ps_main,
            "Trae CN.app"
        ));
        assert!(!NativePlatformHost::parse_macos_ps_has_main_process(
            ps_main,
            "Qoder CN.app"
        ));

        let ps_custom = "\
  201 /Users/alice/Applications/Qoder CN.app/Contents/MacOS/Qoder CN --no-sandbox
";
        assert!(NativePlatformHost::parse_macos_ps_has_main_process(
            ps_custom,
            "Qoder CN.app"
        ));

        let ps_helper_only = "\
  301 /Applications/Qoder CN.app/Contents/MacOS/Qoder CN /Applications/Qoder CN.app/Contents/Resources/native-messaging-host/qoder-app-host.cjs
  302 /Applications/Qoder CN.app/Contents/Frameworks/Qoder CN Helper.app/Contents/MacOS/Qoder CN Helper --type=gpu-process
";
        assert!(!NativePlatformHost::parse_macos_ps_has_main_process(
            ps_helper_only,
            "Qoder CN.app"
        ));

        // 含空格可执行文件路径的参数剥离不得在空格处误截断
        assert_eq!(
            NativePlatformHost::strip_macos_argv0(
                "/Applications/Qoder CN.app/Contents/MacOS/Qoder CN",
                "Qoder CN"
            ),
            ""
        );
        assert_eq!(
            NativePlatformHost::strip_macos_argv0(
                "/Applications/Qoder CN.app/Contents/MacOS/Qoder CN /Applications/Qoder CN.app/Contents/Resources/native-messaging-host/qoder-app-host.cjs",
                "Qoder CN"
            ),
            "/Applications/Qoder CN.app/Contents/Resources/native-messaging-host/qoder-app-host.cjs"
        );
    }

    /// 回归：Electron 打包客户端的主二进制名为 `Electron`，与 `.app` 同名假设不成立。
    ///
    /// WorkBuddy / Trae CN 的主进程形如
    /// `/Applications/WorkBuddy.app/Contents/MacOS/Electron`。
    /// 历史实现要求 argv[0] 严格等于 `/<App>.app/Contents/MacOS/<App>`，
    /// 对这类客户端永远匹配不到 → 误判「客户端未运行」→ 切号后卡死在「重启桌面端」。
    #[test]
    fn electron_named_main_binary_is_detected() {
        let ps_workbuddy = "\
26126 /Applications/WorkBuddy.app/Contents/MacOS/Electron
27365 /Applications/WorkBuddy.app/Contents/MacOS/Electron --disable-warning=ExperimentalWarning /Applications/WorkBuddy.app/Contents/Resources/app.asar/main/daemon-app-server-entry.js --stdio
26758 /Applications/WorkBuddy.app/Contents/Frameworks/WorkBuddy Helper.app/Contents/MacOS/WorkBuddy Helper --type=gpu-process
";
        assert!(
            NativePlatformHost::parse_macos_ps_has_main_process(ps_workbuddy, "WorkBuddy.app"),
            "主二进制名为 Electron 时也必须判定为运行中"
        );

        let ps_trae = "101 /Applications/Trae CN.app/Contents/MacOS/Electron\n";
        assert!(NativePlatformHost::parse_macos_ps_has_main_process(
            ps_trae,
            "Trae CN.app"
        ));
    }

    /// 放宽主二进制名匹配后，辅助进程仍不得被误判为主进程。
    #[test]
    fn helper_only_processes_are_still_excluded() {
        let ps_helpers = "\
301 /Applications/WorkBuddy.app/Contents/Frameworks/WorkBuddy Helper.app/Contents/MacOS/WorkBuddy Helper --type=gpu-process
302 /Applications/Trae CN.app/Contents/Frameworks/Trae CN Helper.app/Contents/MacOS/Trae CN Helper --type=renderer
303 /Applications/Qoder CN.app/Contents/MacOS/Qoder CN /Applications/Qoder CN.app/Contents/Resources/native-messaging-host/qoder-app-host.cjs
";
        assert!(!NativePlatformHost::parse_macos_ps_has_main_process(
            ps_helpers,
            "WorkBuddy.app"
        ));
        assert!(!NativePlatformHost::parse_macos_ps_has_main_process(
            ps_helpers,
            "Trae CN.app"
        ));
        assert!(!NativePlatformHost::parse_macos_ps_has_main_process(
            ps_helpers,
            "Qoder CN.app"
        ));
    }

    /// 端到端：本机若真的跑着 WorkBuddy，必须判定为运行中。
    ///
    /// 这条断言直接覆盖用户遇到的现场：客户端明明在运行，
    /// 却被判为未运行 → 切号跳过优雅关闭 → 拉起后存活确认必然超时。
    #[test]
    fn detection_reports_running_for_live_electron_client() {
        let ps_out = std::process::Command::new("ps")
            .args(["-axo", "command="])
            .output();
        let Ok(o) = ps_out else { return };
        let s = String::from_utf8_lossy(&o.stdout);
        if s.lines()
            .any(|l| l.contains("/WorkBuddy.app/Contents/MacOS/"))
        {
            let host = NativePlatformHost;
            assert!(
                host.is_client_running("workbuddy").expect("检测不应失败"),
                "WorkBuddy 正在运行时必须判定为运行中"
            );
        }
    }

    /// 端到端：真实探测本机 Qoder 主进程。
    ///
    /// 该断言只验证「不 panic 且结果稳定」，因为 CI 环境未必安装 Qoder；
    /// 关键在于当仅有 Native Messaging Host 存活时必须返回 false。
    #[test]
    fn detection_does_not_report_running_for_helper_only_state() {
        // 仅当本机确实存在该 host 进程时验证；否则跳过（CI 环境无客户端）
        let has_helper_only = std::process::Command::new("ps")
            .args(["-axo", "command="])
            .output()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains("native-messaging-host")
            })
            .unwrap_or(false);

        let host = NativePlatformHost;
        // 只要不 panic 即视为通过；结果取决于本机是否真的开着 Qoder 主进程。
        // 核心断言在上面的 `is_macos_helper_argv` 单元测试中已精确覆盖。
        let _running = host
            .is_client_running("qoder_cn")
            .expect("detection should not fail");

        // 仅当本机存在 host 且**不存在**真实主进程时，才要求判定为未运行
        if has_helper_only {
            let has_real_main = std::process::Command::new("ps")
                .args(["-axo", "command="])
                .output()
                .map(|o| {
                    let s = String::from_utf8_lossy(&o.stdout);
                    s.lines().any(|l| {
                        let t = l.trim();
                        t.starts_with("/Applications/Qoder CN.app/Contents/MacOS/Qoder CN")
                            && !t.contains("native-messaging-host")
                            && !t.contains("--type=")
                    })
                })
                .unwrap_or(false);
            if !has_real_main {
                assert!(!_running, "仅有 Native Messaging Host 时不得判定为运行中");
            }
        }
    }
}
