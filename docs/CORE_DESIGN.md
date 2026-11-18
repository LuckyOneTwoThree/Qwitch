# Qwitch 核心接口契约与数据模型设计

> 本文档对应项目推进路线 **Step 1**，定义前后端共享的领域模型、Rust 核心 Trait、外部适配器 JSON-RPC 协议、本地加密 SQLite Schema 以及 Tauri IPC 契约。

---

## 1. 统一领域模型与状态机

所有内置适配器（Rust）、外部扩展适配器（JSON-RPC）与前端 UI（TypeScript）共用同一套状态语义与错误码。

### 1.1 核心枚举与数据结构（TypeScript / JSON 对照）

```typescript
export type BuiltinToolId = "trae" | "trae_cn" | "workbuddy" | "qoder" | "qoder_cn";
export type ToolId = BuiltinToolId | (string & {});

export type AuthMethod = "webview_oauth" | "local_import" | "token_paste";

export type AccountStatus =
  | "active"       // 凭据有效，可正常查询/领取/切换
  | "expired"      // 401 或本地校验过期，需人工重新登录/导入
  | "risk_paused"  // 403 / 验证码 / 疑似风控，已自动熔断暂停
  | "disabled";    // 用户手动停用，跳过批量调度

export type ClaimStatus =
  | "unknown"      // 尚未查询
  | "available"    // 今日可领取
  | "claimed"      // 今日已领取
  | "unsupported"; // 该工具无每日领取机制

export type UnifiedErrorCode =
  | "AUTH_EXPIRED"
  | "RISK_TRIGGERED"
  | "RATE_LIMITED"
  | "NETWORK_ERROR"
  | "CLIENT_RUNNING"
  | "CLIENT_NOT_FOUND"
  | "UNSUPPORTED_CAPABILITY"
  | "INVALID_CREDENTIAL"
  | "INTERNAL_ERROR";

export interface QuotaBucket {
  key: string;           // 如 "daily_credits" | "monthly_tokens"
  label: string;         // 展示名称，如 "每日 Credits"
  used: number;
  total: number;
  remaining: number;
  unit: string;          // 如 "credits" | "USD" | "requests"
  resetAt?: string;      // ISO-8601 时间戳
}

export interface QuotaSnapshot {
  planName?: string;     // 如 "Free" | "Pro"
  claimStatus: ClaimStatus;
  buckets: QuotaBucket[];
  fetchedAt: string;     // ISO-8601
}

export interface AccountSummary {
  id: string;
  toolId: ToolId;
  label: string;
  uidMasked: string;     // 脱敏展示，如 "u_9f2a****c1"
  emailMasked?: string;
  status: AccountStatus;
  isActiveOnClient: boolean;
  latestQuota?: QuotaSnapshot;
  lastClaimedAt?: string;
  updatedAt: string;
}
```

### 1.2 防风控状态流转原则
* **正常流转**：`active` 状态下的账号才允许参与 `claim_daily_batch` 串行队列。
* **自动熔断**：一旦适配器返回 `AUTH_EXPIRED`，状态自动置为 `expired`；返回 `RISK_TRIGGERED` 或 `RATE_LIMITED`，状态自动置为 `risk_paused`，立即中止该账号后续请求，等待用户人工干预（重新登录或手动解除暂停）。

---

## 2. Rust 核心抽象层（`ToolAdapter` & `PlatformHost`）

### 2.1 凭据信封（`CredentialEnvelope`）
为兼容明文 Token、Cookie 集合以及 WorkBuddy 这类整体本地加密文件快照，统一使用 `CredentialEnvelope` 封装高敏凭据（仅在加密 SQLite 内解密后短暂存在于内存）：

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialPayload {
    /// Bearer Token / PAT / Access + Refresh Token
    Token {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<i64>,
        extra: HashMap<String, String>,
    },
    /// 浏览器会话 Cookie
    Cookies {
        cookies: HashMap<String, String>,
        user_agent: Option<String>,
    },
    /// 本机客户端会话文件/目录快照（保留原始加密或结构化内容）
    LocalSnapshot {
        /// key 为相对目标客户端配置目录的文件路径，value 为 Base64 编码内容
        files: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialEnvelope {
    pub tool_id: String,
    pub uid: String,           // 用于生成 uid_hash 防重复绑定
    pub display_email: Option<String>,
    pub payload: CredentialPayload,
}
```

### 2.2 `ToolAdapter` Trait
```rust
use async_trait::async_trait;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterCapabilities {
    pub auth_methods: Vec<AuthMethod>,
    pub supports_quota_query: bool,
    pub supports_daily_claim: bool,
    pub supports_client_switch: bool,
    pub supports_multi_instance: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub is_builtin: bool,
    pub capabilities: AdapterCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimResult {
    pub claimed: bool,          // false 表示幂等跳过（今日已领）
    pub reward_summary: String, // 如 "成功领取 100 Credits" 或 "今日已签过"
    pub updated_quota: Option<QuotaSnapshot>,
}

#[async_trait]
pub trait ToolAdapter: Send + Sync {
    fn manifest(&self) -> &AdapterManifest;

    /// 从本机已登录的官方客户端读取当前会话并打包为快照
    async fn import_from_local(
        &self,
        host: &dyn PlatformHost,
    ) -> Result<CredentialEnvelope, AdapterError>;

    /// 解析用户手动粘贴的 Token 或 JSON
    async fn parse_manual_token(
        &self,
        raw_input: &str,
    ) -> Result<CredentialEnvelope, AdapterError>;

    /// 查询当前额度与今日领取状态
    async fn inspect_quota(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<QuotaSnapshot, AdapterError>;

    /// 手动每日领取（实现必须满足幂等：先查状态，未领再领）
    async fn claim_daily(
        &self,
        cred: &CredentialEnvelope,
    ) -> Result<ClaimResult, AdapterError>;

    /// 将指定账号会话写入本机官方客户端目录（调用前由协调器确保客户端已关闭）
    async fn apply_session(
        &self,
        host: &dyn PlatformHost,
        cred: &CredentialEnvelope,
    ) -> Result<(), AdapterError>;
}
```

### 2.3 `PlatformHost` Trait（macOS / Windows 双平台宿主抽象）
```rust
use std::path::{Path, PathBuf};

#[async_trait]
pub trait PlatformHost: Send + Sync {
    /// macOS: ~/Library/Application Support/qwitch
    /// Windows: %APPDATA%\qwitch
    fn qwitch_data_dir(&self) -> Result<PathBuf, HostError>;

    /// 获取目标工具在本机的标准配置/会话目录
    fn client_config_dir(&self, tool_id: &str) -> Result<PathBuf, HostError>;

    /// 自动探测目标桌面应用可执行文件路径（macOS .app / Windows .exe）
    fn detect_client_executable(&self, tool_id: &str) -> Option<PathBuf>;

    /// 检测目标桌面应用当前是否正在运行
    fn is_client_running(&self, tool_id: &str) -> Result<bool, HostError>;

    /// 优雅退出目标桌面应用并等待文件锁释放
    async fn terminate_client_gracefully(
        &self,
        tool_id: &str,
        timeout_ms: u64,
    ) -> Result<(), HostError>;

    /// 重新拉起目标桌面应用
    fn launch_client(&self, executable: &Path, args: &[String]) -> Result<(), HostError>;

    /// 从系统钥匙串读取或初始化 256-bit 主密钥
    fn get_or_create_master_key(&self) -> Result<[u8; 32], HostError>;
}
```

---

## 3. 外部适配器扩展协议（预留）

当核心引擎启动时，会扫描 `<qwitch_data_dir>/adapters/<adapter_id>/manifest.json`。所有外部适配器通过标准 `stdin/stdout` 单行 JSON-RPC 2.0 通信，无需重新编译 Qwitch 主程序。

### 3.1 `manifest.json` 规范
```json
{
  "id": "custom_ai_tool",
  "name": "Custom AI Tool",
  "version": "0.1.0",
  "platforms": ["macos", "windows"],
  "entry": {
    "command": "python3",
    "args": ["adapter.py"]
  },
  "timeout_ms": 15000,
  "capabilities": {
    "auth_methods": ["local_import", "token_paste"],
    "supports_quota_query": true,
    "supports_daily_claim": true,
    "supports_client_switch": false,
    "supports_multi_instance": false
  }
}
```

### 3.2 JSON-RPC 请求与响应示例
* **请求（Qwitch 主程序 -> 子进程 `stdin`）**：
```json
{"jsonrpc":"2.0","id":"req_1","method":"claim_daily","params":{"credential":{...}}}
```
* **成功响应（子进程 `stdout` -> Qwitch 主程序）**：
```json
{"jsonrpc":"2.0","id":"req_1","result":{"claimed":true,"reward_summary":"成功领取 100 Credits","updated_quota":{...}}}
```
* **错误响应（映射为 `UnifiedErrorCode`）**：
```json
{"jsonrpc":"2.0","id":"req_1","error":{"code":"AUTH_EXPIRED","message":"登录凭据已失效，请重新导入"}}
```

---

## 4. 本地加密 SQLite 表结构

数据库文件位于 `<qwitch_data_dir>/qwitch.db`。所有高敏凭据在应用层使用从系统钥匙串获取的主密钥进行 **AES-256-GCM** 加密后再写入 `BLOB` 字段。

```sql
-- 1. 账号主表
CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,                    -- UUID v4
    tool_id TEXT NOT NULL,                  -- 如 'trae', 'workbuddy', 'qoder_cn'
    label TEXT NOT NULL,                    -- 用户自定义备注名，如 '主力号'
    uid_masked TEXT NOT NULL,               -- 脱敏 UID，仅用于 UI 展示
    uid_hash TEXT NOT NULL,                 -- SHA-256(tool_id + ":" + uid)，防止同账号重复绑定
    email_masked TEXT,                      -- 脱敏邮箱
    status TEXT NOT NULL DEFAULT 'active',  -- active | expired | risk_paused | disabled
    is_active_on_client INTEGER NOT NULL DEFAULT 0, -- 1 表示当前本机桌面端活跃账号
    encrypted_credential BLOB NOT NULL,     -- AES-256-GCM(nonce || ciphertext)
    last_claimed_at TEXT,                   -- 最近一次成功领取时间 (ISO-8601)
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(tool_id, uid_hash)
);

-- 2. 额度快照表（保留最近一次或历史水位）
CREATE TABLE IF NOT EXISTS quota_snapshots (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    plan_name TEXT,
    claim_status TEXT NOT NULL DEFAULT 'unknown',
    buckets_json TEXT NOT NULL,             -- JSON 序列化的 Vec<QuotaBucket>
    fetched_at TEXT NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts(id) ON DELETE CASCADE
);

-- 3. 脱敏操作日志表（严禁写入任何完整 Token 或 Cookie）
CREATE TABLE IF NOT EXISTS action_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT,
    tool_id TEXT NOT NULL,
    action_type TEXT NOT NULL,              -- import | query | claim | switch
    result_status TEXT NOT NULL,            -- success | skipped | failed
    error_code TEXT,                        -- 对应 UnifiedErrorCode
    message_sanitized TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- 4. 应用配置表
CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY,                   -- 如 'client_path.trae', 'scheduler.jitter_ms'
    value_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_accounts_tool ON accounts(tool_id);
CREATE INDEX IF NOT EXISTS idx_quota_account ON quota_snapshots(account_id, fetched_at DESC);
CREATE INDEX IF NOT EXISTS idx_logs_created ON action_logs(created_at DESC);
```

---

## 5. 前后端 Tauri IPC 契约

### 5.1 核心 Commands（前端调用后端）
| Command 名称 | 入参摘要 | 返回摘要 | 说明 |
| --- | --- | --- | --- |
| `list_adapters` | 无 | `AdapterManifest[]` | 获取所有内置与外部已加载适配器 |
| `list_accounts` | `toolId?: ToolId` | `AccountSummary[]` | 获取脱敏后的账号列表与最新额度 |
| `import_local_account` | `toolId, label` | `AccountSummary` | 从本机客户端导入当前登录态 |
| `add_account_by_token` | `toolId, label, rawInput` | `AccountSummary` | 通过粘贴 Token/JSON 添加账号 |
| `refresh_account_quota` | `accountId` | `QuotaSnapshot` | 查询单个账号最新额度与签到状态 |
| `claim_daily_batch` | `accountIds: string[]` | `BatchTaskAccepted` | 投递至防风控串行队列，异步逐个执行 |
| `switch_desktop_account` | `accountId, forceKill: bool` | `SwitchResult` | 协调冷切换：关进程 -> 写会话 -> 重启客户端 |
| `get_action_logs` | `limit, offset` | `ActionLogItem[]` | 分页查看脱敏操作日志 |

### 5.2 核心 Events（后端推送前端，同步主窗口与托盘）
* `qwitch://batch-progress`：推送串行领取队列进度（当前执行账号、随机抖动等待状态、单条结果）。
* `qwitch://account-updated`：账号状态、额度或本机活跃标记变更时触发，主窗口与托盘同步刷新。

---

## 6. 实现增补（2026-11-18，以代码为准）

本节记录契约文档首发后已落地的接口与 Schema 变更，修复批次见 git 历史。

### 6.1 新增/调整 Commands

| Command | 参数 | 返回 | 说明 |
| --- | --- | --- | --- |
| `refresh_quota_batch` | `accountIds: string[]`（≤200） | `()` | 防风控批量查额，与批量领取共用单队列锁；重入直接报错 |
| `claim_daily_batch` | `accountIds: string[]`（≤200） | `()` | 串行领取；重入直接报错；工具级风控熔断 |
| `rename_account` | `accountId, label` | `AccountSummary` | 修改备注，广播 account-updated |
| `update_account_status` | `accountId, status` | `AccountSummary` | 停用/启用/恢复，广播 account-updated |
| `quit_app` | — | `()` | 主窗口显式退出 |
| `list_external_adapters` | — | `ExternalAdapterInfo[]` | 外部插件清单（含 trusted/reserved 标记，仅解析 manifest 不执行） |
| `export_action_logs` | `scope, archiveAfterExport` | `ExportLogsResult` | 上限 10,000 条 |

### 6.2 Schema 增补

* `accounts` 增加 `has_local_snapshot`、`has_api_token` 能力标记列；`label` 重复导入默认不覆盖用户自定义备注；`disabled`/`risk_paused` 状态不被重复导入复活。
* `action_logs` 增加 `archived_at` 归档列；删除请求未指定 scope 时默认仅活跃日志。
* 数据目录新增 `key_id` 主密钥指纹文件（SHA-256 前 16 字节），启动时校验，不匹配拒绝启动。

### 6.3 适配器 trait 变更

* `claim_daily(cred, precheck: Option<&QuotaSnapshot>)`：调度器前置快照复用，避免重复查状态。
* 新增 `backup_session` / `restore_session`：切号写入前备份、失败回滚（Trae/Qoder/WorkBuddy 已实现）。
* 外部适配器 RPC `claim_daily` 请求体新增可选 `precheck` 字段；信任门禁由 `trustedAdapters` 设置控制，保留 ID 禁止冒名。

### 6.4 设置契约增补

`AppSettingsDto` 新增 `trustedAdapters: string[]`；`autoRefreshOnStartup` 默认 `false`、`autoRefreshIntervalMinutes` 默认 `0`（关闭）；Jitter 后端强制下限 500ms。
