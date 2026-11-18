# Qwitch 代码安全与架构审查报告（最终版）

> 本文档基于对 `src-tauri/` 全部 Rust 源码、Cargo 依赖及 Tauri 配置的逐行静态核验编写，覆盖安全边界、密钥托管、OAuth 协议、防风控幂等调度、切号事务可靠性、存储性能与外部适配器沙箱治理。

---

## 一、审查范围与总体结论

* **审查范围**：[src-tauri/src/](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src) 全部模块（`adapter/`、`commands/`、`domain/`、`platform/`、`scheduler/`、`storage/`、`lib.rs`）及 [tauri.conf.json](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/tauri.conf.json)。
* **现有安全与工程基线优点**：
  * 核心层分层清晰（`PlatformHost` / `ToolAdapter` / `AntiRiskScheduler` / `QwitchDatabase`）；
  * SQLite 读写主路径采用参数化绑定（无字符串拼接注入），业务凭据经 **AES-256-GCM**（随机 12 字节 Nonce）加密落盘；
  * HTTP 客户端均配置了超时（15s–20s）并使用 `rustls-tls`；
  * 调度器通过 `tokio::sync::Mutex` 实现单队列串行及随机 Jitter 延迟。
* **核心风险聚焦**：当前最亟待加固的是**凭据外发目标未校验、主密钥明文镜像落盘、OAuth 回调缺少流程绑定、防风控签到状态失败开放（Fail-Open）**；其次是**切号原子性、自定义客户端路径穿透缺失、跨窗口事件漏发、SQLite N+1 与并发配置**。

---

## 二、源码核验与技术细节细化

经逐行源码核对，原《代码与架构审查》中的 **P1-1 ~ P1-4、P2-5 ~ P2-10 及 P3** 全部核验属实，其中针对 **P2-10（SQLite 明文范围）** 作如下精确补充与细化：

1. **P2-10 关于 SQLite 账号信息明文范围的细化**：
   * 核验 [storage/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/storage/mod.rs#L52-L90) 与 [crypto.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/storage/crypto.rs#L57-L86) 可知，`accounts` 表中的 `uid_masked`（如 `u_8f****1c`）与 `email_masked`（如 `al***@example.com`）已做前置脱敏，完整明文 UID 与邮箱仅存于 AES-256-GCM 加密的 `encrypted_credential` BLOB 中；此举是为避免每次列表渲染全表解密。
   * **仍需加固的点**：
     * [tauri.conf.json](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/tauri.conf.json#L38) 将 `"csp"` 设为 `null`，削弱了对 XSS / 脚本注入的纵深防御；
     * `uid_hash` 目前采用无盐 `SHA-256(tool_id + ":" + uid)`，若改为由主密钥派生的 `HMAC-SHA256` 可消除离线彩虹表撞库风险；
     * 账号备注 `label` 与脱敏操作日志 `action_logs` 仍为明文，如追求极高本地隐私需评估 SQLCipher 或字段级加密。

---

## 三、源码深度考究新增补漏清单（B-1 ~ B-4）

在原审查报告的 10 项问题基础上，代码级走查发现以下 **4 项关键后端架构与可靠性遗漏**：

### B-1（P1）｜`custom_client_paths` 配置链路在适配器与进程管理层断裂
* **源码依据**：
  * [commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs#L27-L44) 的 `resolve_client_executable` 读取了 `settings.custom_client_paths`，但仅作用于 `detect_client_status` 与 `switch_desktop_account` 的重启步骤；
  * [workbuddy.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/workbuddy.rs#L1003-L1011) 在 `inspect_quota` 和 `claim_daily` 中直接实例化了无状态的 `NativePlatformHost`，通过 `host.detect_client_executable("workbuddy")` 寻找 `ELECTRON_RUN_AS_NODE` 解密运行时，**完全绕过了 `settings.custom_client_paths`**；
  * [platform/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/platform/mod.rs#L290-L407) 的 `is_client_running` 与 `terminate_client_gracefully` 仅依赖硬编码 `client_spec` 进程名。
* **后果**：用户将 WorkBuddy 安装在自定义目录并在设置页配置覆盖路径后，查额与签到解密加密信封仍报错 `CLIENT_NOT_FOUND`；若客户修改二进制名称或独立目录运行，运行态检测与退出逻辑也无法适配。

### B-2（P1）｜批量刷新额度未纳入 `AntiRiskScheduler` 防风控串行队列
* **源码依据**：[commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs#L306-L360) 仅暴露单账号 `refresh_account_quota`，后端无 `refresh_quota_batch` 命令；前端 [QwitchContext.tsx](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src/context/QwitchContext.tsx#L426-L441) 只能以 `0ms` 间隔循环连续发出请求，且不经过 `AntiRiskScheduler::queue_lock`。
* **后果**：与 [架构文档](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/docs/ARCHITECTURE.md) 倡导的「单队列串行 + 错峰 Jitter + 防风控」背道而驰，多账号点击「刷新全部额度」时并发/连发易触发 `429 RATE_LIMITED` 或 `RISK_TRIGGERED`。

### B-3（P1）｜`delete_account` 与 `update_account_status` 缺少跨窗口事件广播与日志审计
* **源码依据**：[commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs#L453-L471) 中，`delete_account` 和 `update_account_status` 直接操作数据库后即返回，**既未调用 `app.emit("qwitch://account-updated", ...)`，也未写入 `action_logs`**。
* **后果**：在 `main` + `tray` 双窗口架构下，主窗口删除账号或变更状态后，托盘窗口依然保留陈旧账号数据，点击切换会直接报错 `Record not found`，且操作审计不完整。

### B-4（P2）｜SQLite 未开启 `WAL` / `busy_timeout`，且同步 `std::process::Command` 阻塞异步运行时
* **源码依据**：
  * [storage/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/storage/mod.rs#L46-L50) 初始化 SQLite 仅执行了 `PRAGMA foreign_keys = ON;`，未开启 `PRAGMA journal_mode = WAL;`、`PRAGMA synchronous = NORMAL;` 与 `PRAGMA busy_timeout = 5000;`（但 [platform/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/platform/mod.rs#L138-L143) 的旧版迁移代码已考虑 `-wal`/`-shm`）；
  * [workbuddy.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/workbuddy.rs#L220-L239)（Node 解密子进程）、[qoder.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/qoder.rs#L104-L107)（Windows PowerShell DPAPI 解密）及 [platform/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/platform/mod.rs#L298-L320)（`pgrep`/`tasklist`/`osascript`/`taskkill`）均在 `async fn` 中同步阻塞调用 `std::process::Command`，未移交 `tokio::task::spawn_blocking` 或使用 `tokio::process::Command`。

---

## 四、定稿问题全表

### 4.1 P1 — 高危安全与核心可靠性（共 7 项）

| 编号 | 问题 | 核心源码定位 |
|---|---|---|
| **BE-P1-1** | WorkBuddy 凭据中的 `auth.endpoint` / `extra.endpoint` 未校验 HTTPS 与官方域名白名单，存在 Bearer Token 任意地址外泄（SSRF / 凭据窃取）风险 | [workbuddy.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/workbuddy.rs#L519-L525), [workbuddy.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/workbuddy.rs#L569-L595) |
| **BE-P1-2** | OS Keychain 中的 256-bit 主密钥同时被明文镜像写入同目录 `master.key`（Windows 无 ACL 限制），削弱离线加密库隔离 | [platform/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/platform/mod.rs#L427-L518) |
| **BE-P1-3** | Trae OAuth WebView 回调仅检查 `host == qwitch.local`，缺少一次性 `state/nonce` 绑定与路径校验，且将完整 Token 放入 URL Query | [commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs#L793), [commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs#L1091-L1139) |
| **BE-P1-4** | 签到状态解析「失败开放（Fail-Open）」：字段缺失默认 `Available` 且 `scheduler` 放行 `Unknown`；领取响应解析失败时默认成功并虚报 100 积分 | [trae.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/trae.rs#L523-L545), [workbuddy.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/workbuddy.rs#L668-L675), [scheduler/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/scheduler/mod.rs#L155-L189), [qoder.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/qoder.rs#L779-L796) |
| **B-1** | `custom_client_paths` 未贯穿至 `WorkBuddyAdapter` 运行时解密及进程检测/退出逻辑 | [workbuddy.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/workbuddy.rs#L1003-L1011), [commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs#L27-L44) |
| **B-2** | 批量查额缺少后端串行 Jitter 调度入口，前端 0ms 连发绕过防风控保护 | [commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs#L306-L360), [scheduler/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/scheduler/mod.rs) |
| **B-3** | `delete_account` 与 `update_account_status` 未发送 `qwitch://account-updated` 事件且未写入审计日志 | [commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs#L453-L471) |

### 4.2 P2 — 可靠性、性能与隔离加固（共 7 项）

| 编号 | 问题 | 核心源码定位 |
|---|---|---|
| **BE-P2-5** | 切号在客户端启动前提交活跃状态且忽略未找到/启动失败；Trae 写 `state.vscdb` 吞掉 SQL 错误导致状态不一致 | [commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs#L416-L433), [trae.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/trae.rs#L359-L386) |
| **BE-P2-6** | OAuth 轮询仅判断窗口 label `oauth-login` 是否存在，重开授权窗口时旧轮询继续执行并可能串绑或误关新窗口 | [commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs#L1010-L1221) |
| **BE-P2-7** | 批量同步事件放大与 `list_accounts` N+1 SQL 查询；WorkBuddy/Qoder 轮询每轮新建 `reqwest::Client` | [storage/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/storage/mod.rs#L300-L333), [workbuddy.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/workbuddy.rs#L355), [qoder.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/qoder.rs#L389) |
| **BE-P2-8** | `quota_snapshots` 只插不删无清理/上限策略，长期运行数据库持续膨胀 | [storage/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/storage/mod.rs#L428-L451) |
| **BE-P2-9** | 外部适配器自动扫描无信任启用确认、无 `timeout_ms` 上限、无 stdout/stderr 读取大小限制 | [external.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/external.rs#L107-L182), [external.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/external.rs#L299-L334) |
| **BE-P2-10** | `tauri.conf.json` 将 `csp` 设为 `null`；`uid_hash` 未加密钥盐；非凭据表字段为明文 | [tauri.conf.json](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/tauri.conf.json#L38), [crypto.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/storage/crypto.rs#L57-L63) |
| **B-4** | SQLite 缺 `WAL` + `busy_timeout`；异步上下文中同步调用 `std::process::Command` 阻塞 Tokio 线程 | [storage/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/storage/mod.rs#L48), [workbuddy.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/workbuddy.rs#L220), [qoder.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/adapter/qoder.rs#L104) |

### 4.3 P3 — 可维护性与测试覆盖（共 2 项）

| 编号 | 问题 | 核心源码定位 |
|---|---|---|
| **BE-P3-1** | 关键业务测试缺失：目前仅 5 个 Rust 单测（集中在 domain/crypto/storage），适配器解析、调度器熔断、切号回滚与 OAuth 取消均零测试 | [domain/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/domain/mod.rs#L366), [crypto.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/storage/crypto.rs#L88), [storage/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/storage/mod.rs#L828) |
| **BE-P3-2** | [commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs)（1225 行）职责过重，耦合了账户/设置/日志/切号/OAuth 注入脚本；三个适配器存在重复的 HTTP 状态映射与 UID Hash 逻辑 | [commands/mod.rs](file:///Users/lucky/Desktop/HelloWorld/Git_Project/Qwitch/src-tauri/src/commands/mod.rs), `adapter/*.rs` |

---

## 五、后端安全与可靠性加固方案

1. **端点白名单校验（WorkBuddy）**：只允许 `https://` 协议且主机严格匹配官方白名单（`copilot.tencent.com`、`www.workbuddy.cn` 及受控子域）；企业私有化端点必须由用户在「系统设置」中显式登记信任后才放行，拒绝来自本地导入文件或手动 JSON 的未校验 `endpoint`。
2. **主密钥托管收口**：优先并信任系统 Keychain（macOS Keychain / Windows Credential Manager）为唯一主密钥持久化源；仅当 Keychain 访问明确失败且用户处于无头/受限环境时才降级提示或采用平台加密存储；提供一次性平滑迁移清理逻辑，在确认 Keychain 校验写入成功后安全擦除明文 `master.key`。
3. **OAuth 流程隔离与绑定**：每次 `start_webview_oauth` 生成唯一 `flow_id` + 随机 `nonce` 并注册 `Arc<AtomicBool>` / `CancellationToken` 到 `AppState`；Trae 回调改为注册自定义 `tauri::command` 或校验精确回调 URL + 一次性 `nonce`（通过 POST/Invoke 或一次性内存通道交换凭据，不在 URL Query 暴露 Token）。
4. **严格 Fail-Closed 防风控状态机**：
   * 签到状态查询缺失核心字段或 `code != 0` 时返回 `ClaimStatus::Unknown` 或错误，**调度器仅当明确为 `ClaimStatus::Available` 时才发起领取请求**；
   * 领取接口返回 HTTP 非 2xx 或业务码异常、或缺少明确成功标识时，禁止伪造 `claimed: true` 或默认 `100` 积分，必须再次查询权威状态或显式返回错误/待核实。
5. **切号事务与补偿机制**：写入前先备份当前客户端会话文件到临时恢复点；确保可执行文件存在且进程已退出后再写回新快照并严查 `state.vscdb` 写入结果；若 `launch_client` 失败则恢复原会话文件或中止提交 `set_active_on_client` 并向前端返回精确错误码。

---

## 六、后端专项优化计划与任务安排（BE-Phase 1 ~ BE-Phase 4）

### BE-Phase 1｜安全红线与防风控止血（最高优先级，约 3–4 天）

| 任务 ID | 映射问题 | 涉及文件 | 具体改动内容 | 测试与验收标准 |
|---|---|---|---|---|
| **BE-1.1** | BE-P1-1 | `workbuddy.rs`, `domain/mod.rs` | 新增 `validate_workbuddy_endpoint()`：强制 `https` 且校验白名单域名；若 `settings` 未配置受信任企业端点，拒绝将 Bearer Token 发往未知域名并落审计日志 | 单测覆盖 `http://`、非白名单域名、合法 `copilot.tencent.com` 三类场景 |
| **BE-1.2** | BE-P1-2 | `platform/mod.rs`, `lib.rs` | Keychain 读取或写入成功后不再镜像写入明文 `master.key`；若存在旧版遗留 `master.key` 且 Keychain 校验可读，自动安全删除明文文件；对确实无 Keychain 环境的平台保留受限兜底并记录安全警告 | 正常 macOS/Windows 启动后数据目录不再保留明文 `master.key`；重启仍可正常解密历史数据 |
| **BE-1.3** | BE-P1-3 | `commands/mod.rs` | 为 `start_webview_oauth` 生成随机 `flow_nonce` 并存入活跃 OAuth 状态；回调严格校验精确路径 `/oauth-callback` 与 `nonce` 匹配；尽量将凭据通过注入桥或受限 PostMessage/Invoke 传给 Rust 后端，不再拼入 URL Query | 构造伪造 `https://qwitch.local/?payload=...` 跳转被直接拦截拒绝 |
| **BE-1.4** | BE-P1-4 | `trae.rs`, `workbuddy.rs`, `qoder.rs`, `scheduler/mod.rs` | 改写状态解析为 Fail-Closed：缺失签到字段返回 `ClaimStatus::Unknown`；`scheduler` 仅对 `ClaimStatus::Available` 发起领取；移除三家适配器在响应残缺时 `.unwrap_or(100)` 的默认成功逻辑 | 构造缺字段/非零业务码/空响应体 JSON 的 Mock 测试，确认绝不误发领取且不虚报成功 |
| **BE-1.5** | B-3 | `commands/mod.rs` | 在 `delete_account` 与 `update_account_status` 成功后补充 `app.emit("qwitch://account-updated", &account_id)` 与脱敏 `append_action_log`，并支持新增 `rename_account` 命令 | 主窗口删号或变更状态后，托盘窗口同步刷新且审计日志有记录 |

### BE-Phase 2｜切号原子性、路径穿透与 OAuth 生命周期（约 4–5 天）

| 任务 ID | 映射问题 | 涉及文件 | 具体改动内容 | 测试与验收标准 |
|---|---|---|---|---|
| **BE-2.1** | B-1 | `platform/mod.rs`, `adapter/mod.rs`, `workbuddy.rs`, `commands/mod.rs` | 将用户配置或解析后的 `ExecutableOverrides` 传入 `PlatformHost` 或适配器上下文，使 `WorkBuddyAdapter` 的 `sym-v1` 解密、运行检测和进程退出都能正确识别 `custom_client_paths` | 将 WorkBuddy 移至自定义目录并配置路径覆盖后，本机导入、查额、签到、切号全链路成功 |
| **BE-2.2** | BE-P2-5 | `commands/mod.rs`, `trae.rs` | 切号前先校验可执行文件可用性；`apply_session` 前备份现有文件以便异常回滚；`TraeAdapter::apply_vscdb_auth_entries` 改为返回 `Result` 并在失败时阻断切号；仅在 `launch_client` 成功后提交 `set_active_on_client` | 模拟无效客户端路径或 `state.vscdb` 锁死，确认返回明确错误、活跃标记不被污染且原会话未被破坏 |
| **BE-2.3** | BE-P2-6 | `commands/mod.rs`, `lib.rs` | 在 `AppState` 中维护 `active_oauth_flow: Mutex<Option<OAuthFlowHandle>>`（含递增 `flow_id` 与取消信号）；新授权或窗口关闭立即取消旧轮询任务；复用 `WorkBuddyAdapter` / `QoderAdapter` 静态或共享 `reqwest::Client` | 连续两次打开关闭授权窗口，旧轮询 100% 退出，不会写入旧目标或关闭新窗口 |
| **BE-2.4** | B-2 | `commands/mod.rs`, `scheduler/mod.rs` | 新增 `refresh_quota_batch(account_ids)` 命令并接入 `AntiRiskScheduler::queue_lock`，复用单队列串行 + Jitter 延时 + 实时事件（`qwitch://batch-progress`），遇到 `401/403/429` 自动熔断 | 批量查额严格串行错峰且与批量签到互斥，并发点击不重叠执行 |

### BE-Phase 3｜存储性能、插件安全与运行态加固（约 4–5 天）

| 任务 ID | 映射问题 | 涉及文件 | 具体改动内容 | 测试与验收标准 |
|---|---|---|---|---|
| **BE-3.1** | BE-P2-7, BE-P2-8, B-4 | `storage/mod.rs` | 开启 SQLite `WAL`、`NORMAL`、`busy_timeout = 5000`；重构 `list_accounts` 使用窗口函数或 `LEFT JOIN` 单条 SQL 批量获取最新额度消除 N+1；为 `quota_snapshots` 引入单账号保留最新 N 条（如保留最近 30 条）的自动清理 | 100 个账号下 `list_accounts` SQL 执行次数降为 1 次；反复查额数据库行数保持受控 |
| **BE-3.2** | B-4 | `workbuddy.rs`, `qoder.rs`, `platform/mod.rs` | 将同步 `std::process::Command` 全面迁移为 `tokio::process::Command`（带超时控制）或 `tokio::task::spawn_blocking` | 进程探测或子进程解密慢时不再阻塞 Tokio 异步调度线程与 UI 事件推送 |
| **BE-3.3** | BE-P2-9 | `adapter/external.rs`, `storage/mod.rs` | 为外部适配器增加 `timeout_ms` 上限封顶（如 60,000ms）与流式读取上限（如 stdout/stderr 各限 1MB）；增加用户显式授权/启用开关（`trusted_adapters` 设置），未授权外部插件仅显示待信任状态不自动接收凭据 | 超大输出或死循环外部脚本被安全截断并超时终止；新放入的未信任适配器不会静默拿到解密凭据 |
| **BE-3.4** | BE-P2-10 | `tauri.conf.json`, `storage/crypto.rs` | 为主窗口与托盘窗口配置最小权限 `csp`（禁止内联脚本与任意外域加载），仅保留必须的资源策略；评估将 `uid_hash` 升级为 `HMAC-SHA256(master_key, ...)`（含历史行平滑兼容迁移） | Tauri 安全配置审计通过，内嵌脚本注入被 CSP 拦截 |

### BE-Phase 4｜模块拆解与自动化测试基线（约 4 天）

| 任务 ID | 映射问题 | 涉及文件 | 具体改动内容 | 测试与验收标准 |
|---|---|---|---|---|
| **BE-4.1** | BE-P3-2 | `commands/*.rs`, `adapter/common.rs` | 将 `commands/mod.rs` 按领域拆分为 `account.rs`、`oauth.rs`、`settings.rs`、`logs.rs`、`oauth_capture.js`；抽取各适配器通用的 HTTP 状态码映射、JWT 解析与错误构造到 `adapter/common.rs` | 代码模块化，单文件控制在 300–500 行内 |
| **BE-4.2** | BE-P3-1 | `src-tauri/tests/*` 及各模块 `#[cfg(test)]` | 搭建基于 `MockPlatformHost` 与本地 Mock HTTP Server（如 `wiremock` / `tokio::net::TcpListener`）的集成测试，覆盖：1）三家适配器 Fail-Open/边界解析；2）`AntiRiskScheduler` 串行+幂等+熔断；3）切号失败回滚；4）OAuth 取消与端点白名单；5）SQLite 批量查询与快照清理 | `cargo test` 测试数量从 5 个扩展至 25+ 个核心场景，纳入 `.github/workflows/ci.yml` 强制门禁 |
