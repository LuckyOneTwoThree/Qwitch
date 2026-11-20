# Qwitch 项目认知总结（二次开发基线）

> 基于当前 `main` 分支源码（v0.2.3）逐文件核验形成，作为后续优化与功能迭代的“先读这份”基线。
> 核验事实：`npm test` 29 例通过；`cargo test --locked --all-targets` 82 例通过；前端 Vite 6.4.6 / React 18 / TS strict；后端 Rust + Tauri v2。

---

## 1. 项目定位与边界

- **一句话**：本地优先（Local-First）的 **AI 桌面工具多账号会话 + 每日额度/签到管理** 桌面应用（Tauri v2 = Rust 后端 + React 前端）。
- **核心价值**：集中查看多账号额度水位与签到状态、一键（串行+抖动）领取每日积分、将选定账号登录态**冷切换**进本机官方客户端。
- **明确非目标**：无云端同步/遥测；不做无人值守高频挂机（自动刷新/自动签到默认关闭，自动签到需二次确认）；不篡改设备指纹；不引入闭源核心依赖。
- **支持的工具（已注册内置适配器）**：`trae_cn`（Trae CN）、`workbuddy`（WorkBuddy/腾讯）、`qoder_cn`（Qoder CN）、`minimax_code_cn`（MiniMax Code CN）。`trae`/`qoder` 国际版为保留 ID，未实现。

---

## 2. 技术栈

| 层 | 技术 |
| --- | --- |
| 桌面框架 | Tauri v2（`tray-icon`、`macos-private-api`、`single-instance`） |
| 前端 | React 18、TypeScript（`strict`）、Tailwind CSS 4（`@tailwindcss/vite`）、Vite 6、lucide-react 图标、Vitest 5 |
| 后端 | Rust 2021、tokio（rt-multi-thread/macros/sync/time/process/io-util，MSRV 1.88）、rusqlite（bundled SQLite）、reqwest（rustls-tls）、keyring v3（apple/windows native）、aes-gcm 0.10、sha2、uuid、chrono、base64 |
| 质量门禁 | `tsc` + `vite build`、Vitest、`cargo fmt --check`、`cargo clippy -D warnings`、`cargo test`、`npm audit --omit=dev` |
| 平台 | macOS 10.15+（arm64 / x64 独立包）、Windows 10+（x64 NSIS/MSI） |

---

## 3. 目录结构

```
Qwitch/
├─ index.html / vite.config.ts / tsconfig.json / package.json
├─ public/                 theme-init.js（首帧防白闪）、icons/
├─ src/                    # 前端
│  ├─ App.tsx              # 双窗口分流（main / tray）+ 浏览器 Mock 预览外壳
│  ├─ main.tsx             # React 入口（StrictMode + ErrorBoundary）
│  ├─ pages/Dashboard.tsx  # 主窗口（账号池/导航/KPI），~1178 行；日志/设置视图拆至 pages/dashboard/{LogsView,SettingsView,types}
│  ├─ pages/TrayPopover.tsx# 托盘弹层
│  ├─ context/QwitchContext.tsx  # 全局状态中枢，~1998 行（+ settingsUtils.ts、logStateUtils.ts、useToast.ts、useAppUpdate.ts）
│  ├─ domain/              # selectors.ts（统计口径）、toolRegistry.ts（工具品牌/可见工具）
│  ├─ components/          # AccountCard/ImportAccountModal/ColdSwitchModal/UpdateCard/... + ui/ 基座
│  ├─ data/mockStore.ts    # 浏览器预览 Mock 数据
│  └─ types/qwitch.ts      # 前端领域类型（与后端 camelCase 对齐）
├─ src-tauri/              # 后端
│  └─ src/
│     ├─ lib.rs            # Tauri Builder：setup/托盘/窗口事件/命令注册
│     ├─ commands/mod.rs   # AppState + 窗口/托盘辅助 + 子模块 glob 再导出；IPC 命令按域拆至 accounts/profile/settings/switch/logs/oauth.rs
│     ├─ adapter/          # ToolAdapter trait + trae/workbuddy/qoder/minimax + external(JSON-RPC) + common
│     ├─ scheduler/mod.rs  # AntiRiskScheduler：单队列串行+Jitter+幂等+工具级熔断
│     ├─ platform/mod.rs   # PlatformHost：路径/进程/钥匙串/自启/数据迁移，~1767 行
│     ├─ storage/mod.rs    # SQLite + 加密凭据 + 日志 + PII 迁移，~2406 行
│     ├─ storage/crypto.rs # AES-256-GCM、uid_hash、脱敏、日志清洗
│     ├─ domain/mod.rs     # 领域模型 + AppSettingsDto + CredentialEnvelope
│     └─ update.rs         # GitHub Release 更新检查/下载/校验
├─ docs/                   # 架构/契约/适配器规范/品牌/审计/路线图
└─ .github/workflows/      # ci.yml（双平台门禁）+ release.yml（三产物发布）
```

> `dist/`、`src-tauri/target/`、`src-tauri/gen/` 为构建产物（已 gitignore）。

---

## 4. 架构分层与数据流

四层，前后端以 **Tauri IPC（commands + events）** 连接：

1. **UI 交互层**（React/TS/Tailwind）：`Dashboard` 主窗口 + `TrayPopover` 托盘弹层；状态全部经 `QwitchContext` 中枢，统计口径集中在 `domain/selectors.ts`。
2. **核心调度/防风控层**（Rust）：`AntiRiskScheduler` 串行队列 + Jitter + 幂等 + 工具级熔断；`switch_desktop_account` 冷切换协调器（停进程→备份→写态→重启→存活确认→回滚）。
3. **适配器层**：`ToolAdapter` trait（内置 Rust 实现）+ `external`（`manifest.json` + stdin/stdout 单行 JSON-RPC 2.0，**默认不加载，显式信任后启用**，保留 ID 禁止冒名）。
4. **存储/平台基础设施**：`QwitchDatabase`（SQLite WAL + AES-256-GCM 凭据 + 脱敏日志 + PII 迁移）+ `PlatformHost`（钥匙串主密钥、进程探测/退出、路径解析、自启）。

**关键数据流**：前端 `invoke(command)` → 命令读 `AppState`（db/master_key/host/adapters/scheduler）→ 调度器串行执行 → 落库 + `app.emit("qwitch://...")` 事件 → 前端 `listen` 收到后 120ms 去抖 `syncFromBackend()` 全量刷新（`list_adapters/list_accounts/get_action_logs/get_action_log_stats/get_app_settings`）。

`AppState`（`commands/mod.rs`）：`db: Arc<QwitchDatabase>`、`master_key: [u8;32]`、`host: Arc<dyn PlatformHost>`、`adapters: Arc<HashMap<String, Arc<dyn ToolAdapter>>>`、`scheduler`、`oauth_flow_seq: AtomicU64`、`switching_in_progress: AtomicBool`（切号互斥 + 托盘不因失焦隐藏）。

---

## 5. 前端核心模块职责

- **`context/QwitchContext.tsx`（单一全局 Provider）**：持有 `adapters/accounts/logs/logStats/batchProgress/switchDialog/toast/appSettings/theme/clientStatuses/cstDayKey`；封装全部业务动作：`refreshAccountQuota`、`refreshAllQuotas`（→`refresh_quota_batch`）、`claimDailyBatch`（→`claim_daily_batch`）、`requestColdSwitch/confirmColdSwitch`（→`switch_desktop_account`）、`importAccount`、`previewLocalAccount`、`renameAccount`、`toggleAccountDisabled`、`recoverAccount`、`deleteAccount`、日志 archive/unarchive/delete/export、`quitApp`。
  - **Mock 分支**：非 Tauri 运行时（`__TAURI_INTERNALS__` 不存在）时，用 `data/mockStore.ts` 数据 + `sleep` 模拟异步，保证 `npm run dev` 浏览器可预览。
  - 设置保存有“链式串行 + 脏标记（jitter/retention draft 不被后台同步覆盖）+ 失败回滚”保护（`saveAppSettings`），只合并显式变更字段（`buildSettingsSavePayload`）。
  - 事件订阅在单一 `useEffect` 内 `listen` 并在卸载时 `unlisten`，注意重挂竞态已用 `cancelled` 处理。
- **`domain/selectors.ts`**：唯一统计口径来源——`getEffectiveClaimStatus`（与后端 `normalize_daily_claim_status` 同口径，CST 日界）、`selectClaimCandidates`、`selectScopedStats`（健康度/可领/待核实/筛选计数，互斥闭合）。**改统计必须改此处**，不要在组件内重算。
- **`domain/toolRegistry.ts`**：`TOOL_BRAND_META`（品牌色/图标/tagline）、`BUILTIN_TOOL_ORDER`、`getVisibleTools(adapters)`（合并后端 `list_adapters`，支持外部插件动态展示）。
- **`pages/Dashboard.tsx`**：三态导航 `nav: "all" | "logs" | "settings" | ToolId`；账号池（分组/扁平、筛选/搜索）、额度/签到操作、日志区（筛选/全选/归档/恢复/导出/分页）、设置区（外观主题、常驻/自启、自动化策略、抖动、日志保留、外部插件信任、客户端路径、软件更新）。
- **`components/`**：`AccountCard`（`React.memo`，卡片=额度进度+签到按钮+切号/改名/停用/删除）、`ImportAccountModal`（本机导入/在线授权/Token 三轨）、`ColdSwitchModal`（切号阶段：closing→writing→launching→done）、`UpdateCard`、`BatchClaimBanner`、`StatusBadge`、`ToolBrandIcon`；`ui/`：`Modal`（焦点陷阱）、`ConfirmDialog`、`ToastViewport`、`ErrorBoundary`。

---

## 6. 后端核心模块职责

- **`lib.rs`**：启动编排——开库→取凭据样本→选主密钥（`KeyExpectation`：`key_id` 指纹 + 样本密文双验证）→`verify_or_init_key_fingerprint`→PII 迁移→日志保留清理→自启→建适配器→`AppState`→托盘/菜单/窗口事件→注册 33 个命令。关闭主窗口按 `close_to_tray` 收进托盘；托盘 `Focused(false)` 在非切号中隐藏。
- **`commands/`**：`mod.rs`（`AppState`、窗口/托盘辅助、`pub use` 再导出）+ 6 个子模块：`accounts.rs`（账户增删改查/查额/领取）、`profile.rs`（导入/预览/Token）、`settings.rs`、`switch.rs`（切号）、`logs.rs`、`oauth.rs`（OAuth + 注入脚本）。含账户增删改查、导入/预览/Token、查额（单/批）、领取批量、切号、日志治理、更新。`resolve_client_executable` 统一 `custom_client_paths` 覆盖。第三阶段已按域拆分（原单文件职责过重的结构债已消除）。
- **`adapter/mod.rs`**：`ToolAdapter` trait（`import_from_local/parse_manual_token/inspect_quota/claim_daily(precheck)/refresh_credential/apply_session/backup_session/restore_session/daily_claim_device_key`）；`create_all_adapters`（内置 4 个 + 外部信任过滤 + `RESERVED_ADAPTER_IDS` 防冒名）；`backup_files/restore_files` 会话回滚（含 WAL/SHM）。
- **`adapter/{trae,workbuddy,qoder,minimax}.rs`**：各自本地凭据解析/解密、额度与签到接口、`apply_session` 写回。要点：Trae `iCubeAuthInfo://icube.cloudide`（CN 为 `"tc"` 加密串）+ 设备级每日一次（错误码 9095）；WorkBuddy `workbuddy-desktop.info`（`sym-v1` AES-GCM，需 Electron 运行时解密）+ 端点白名单（`copilot.tencent.com`/`www.workbuddy.cn`）；Qoder `auth.v1.dat`（Chromium v10 + DPAPI/Keychain）+ `campaigns` 活动领取；MiniMax 设备码 OAuth + `credit/signin` 接口 + `refresh_credential`。
- **`scheduler/mod.rs`**：`AntiRiskScheduler.mutex` 单队列。`run_daily_claim_batch`：逐账号→状态/能力/熔断/设备日限校验→`inspect_quota` 幂等前置（Fail-Closed：仅 `Available` 才领）→`claim_daily`→落库/日志/设备标记→账号间 `rand` Jitter（下限由设置保障）→`qwitch://batch-progress` 阶段事件→汇总 `done`。`run_quota_refresh_batch` 同理。`handle_adapter_failure` 将 401→`expired`、403/429→`risk_paused` 并熔断该工具。
- **`storage/mod.rs` + `crypto.rs`**：schema `accounts/quota_snapshots/action_logs/app_settings`（WAL/synchronous=NORMAL/busy_timeout=5000，`has_local_snapshot/has_api_token/archived_at` 列）；`list_accounts` 用单条 LEFT JOIN 取最新快照（已消除 N+1）；AES-256-GCM 随机 nonce 加密凭据、`mask_uid/email/phone`、`sanitize_log_message`、`compute_uid_hash`（SHA-256）、`verify_or_init_key_fingerprint`、`migrate_legacy_pii`。
- **`platform/mod.rs`**：`PlatformHost` 抽象 + `NativePlatformHost`（数据目录 `~/Library/Application Support/qwitch` / `%APPDATA%\qwitch`，兼容迁移旧 `quiver`）；主密钥托管（优先系统钥匙串，`key_id`/样本校验，冲突不误删）；进程探测/优雅退出/拉起的 macOS/Windows 分支；macOS LaunchAgent plist 自启。
- **`update.rs`**：GitHub Release 检查（API/重定向双路径）、按 OS/arch 选产物、SHA256 校验、Windows 调安装包、mac URL 只做指引；URL 仅允许 GitHub HTTPS。

---

## 7. 主要业务流程

1. **导入账号**（`import_local_account` / `add_account_by_token` / `start_webview_oauth`）：适配器解析→生成 `CredentialEnvelope`→`upsert_account`/`bind_account_credential`（加密落库，`label` 不覆盖自定义、`disabled/risk_paused` 不被复活）→后台 `schedule_import_quota_check`（复用串行锁）→emit 事件。
2. **查额**：单账号 `refresh_account_quota`（进 `queue_lock` 排队）；批量 `refresh_quota_batch`（≤200、去重、`try_lock_owned` 重入拒绝、spawn 后台跑 `run_quota_refresh_batch`）。
3. **每日领取**：`claim_daily_batch`（≤200、去重、Trae CN 活跃账号排前）→`run_daily_claim_batch` 串行 + 幂等先查后领 + 抖动 + 工具熔断 + 设备日限。
4. **冷切换**：`switch_desktop_account`→校验适配器/可执行文件→`SwitchFlagGuard` 互斥→运行则 `terminate_client_gracefully`→`backup_session`→`apply_session`→`launch_client`→15s 存活轮询（连续两次探测）→`set_active_on_client`→成功/失败均写日志并 `restore_session` 回滚。
5. **软件更新**：`check_app_update`（启动静默 + 设置页手动）→`download_and_install_update`（校验后 Windows 拉起安装）。

---

## 8. 关键数据模型与接口契约

**领域枚举**（前后端对齐，Rust snake_case ↔ TS camelCase，序列化 `camelCase`）：
- `AuthMethod`: `webview_oauth` / `local_import` / `token_paste`
- `AccountStatus`: `active` / `expired` / `risk_paused` / `disabled`
- `ClaimStatus`: `unknown` / `available` / `claimed` / `unsupported`
- `UnifiedErrorCode`: `AUTH_EXPIRED` / `RISK_TRIGGERED` / `RATE_LIMITED` / `DEVICE_DAILY_LIMIT_REACHED` / `NETWORK_ERROR` / `CLIENT_RUNNING` / `CLIENT_NOT_FOUND` / `UNSUPPORTED_CAPABILITY` / `INVALID_CREDENTIAL` / `INTERNAL_ERROR`

**`CredentialEnvelope`**（加密落盘核心）：`tool_id` + `uid` + `display_name/email` + `payload: CredentialPayload`（`Token{access,refresh,expires_at,extra}` / `Cookies` / `LocalSnapshot{files: Base64}`）。`capability_flags()` 由 payload 推导 `has_local_snapshot`/`has_api_token`（决定卡片“就绪/待授权”）。

**SQLite Schema**（`storage/mod.rs`）：`accounts`（`uid_hash` 唯一防重绑、`encrypted_credential` BLOB、能力标记列）、`quota_snapshots`（`buckets_json`，单账号保留清理）、`action_logs`（脱敏、`archived_at`）、`app_settings`（KV JSON）。

**Tauri Commands（已注册 28 个）**：`show_main_window`、`quit_app`、`get/save_app_settings`、`list_adapters`、`list_external_adapters`、`detect_client_status`、`list_accounts`、`import_local_account`、`preview_local_account`、`add_account_by_token`、`start_webview_oauth`、`refresh_account_quota`、`refresh_quota_batch`、`switch_desktop_account`、`delete_account`、`update_account_status`、`rename_account`、`claim_daily_batch`、`get_action_logs`、`get_action_log_stats`、`archive/unarchive/delete_action_logs`、`export_action_logs`、`check_app_update`、`download_and_install_update`、`open_update_url`。

**Tauri Events**（后端→前端）：`qwitch://batch-progress`（`action: "claim"|"refresh"`、`running`、`total`、`completed`、`current_label`、`phase: checking|claiming|jitter_wait|done`、`jitter_seconds`、`summary_text`）、`qwitch://account-updated`、`qwitch://logs-updated`、`qwitch://settings-updated`、`qwitch://switch-progress`、`qwitch://oauth-error`。

> 注意：`get_account_summary(accountId)` 单行增量同步**尚未实现**；前端目前每个事件都全量 `syncFromBackend()`（120ms 去抖）。

---

## 9. 构建与运行

```bash
npm install
npm run dev            # 仅前端（浏览器 Mock 预览，无 Tauri）
npm run tauri dev      # 桌面应用开发（Vite:1420 + Rust）
npm run build          # tsc + vite build（产出 dist/）
npm test               # Vitest（29 例）
cd src-tauri
cargo test --all-targets            # 82 例
cargo fmt --check && cargo clippy --all-targets -- -D warnings
npx tauri build        # 打包安装包
```

前置：Node ≥ 22、Rust stable（含 clippy/rustfmt）、macOS/Windows 桌面环境（依赖系统 WebView）。`vite.config.ts` 固定端口 1420、忽略 `src-tauri/**` 监听。版本号在 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 三处需同步。

---

## 10. 测试与 CI 配置

- **前端**：`selectors.test.ts`（15）、`settingsUtils.test.ts`（6）、`logStateUtils.test.ts`（3）、`toolRegistry.test.ts`（3）、`mockStore.test.ts`（2）= 29 例，`vitest run`。
- **后端**：模块内 `#[cfg(test)]` 覆盖 storage（增删改/密钥指纹/PII 迁移/快照保留/跨日口径）、crypto（加解密/脱敏）、platform（主密钥决策矩阵/LaunchAgent）、adapter（外部信任/会话回滚/Trae 设备头/WorkBuddy 端点白名单）、commands（OAuth 白名单/UID 校验/切号互斥）、update（选产物/校验/URL 白名单）= 82 例（含 scheduler 5 个行为测试）。
- **CI（`ci.yml`）**：push/PR 到 `main`，`macos-latest + windows-latest` 矩阵 → `npm ci` → `npm audit --omit=dev` → `npm run build` → `npm test` → `cargo fmt --check` → `cargo clippy -D warnings` → `cargo test --all-targets`。
- **Release（`release.yml`）**：`v*` tag 触发（先 ubuntu verify）→ 构建 macOS arm64 / macOS x64 / Windows x64 三产物 → 压 SHA256SUMS → `gh release` 发布（预发布带 `-` 则 prerelease）。
- **文档漂移提示**：测试数量以实测为准（当前 Rust 82 / Vitest 29）；二次开发时以代码为准。

---

## 11. 代码约定与风格（改动时必须保持）

- **命名/序列化**：Rust 结构体统一 `#[serde(rename_all = "camelCase")]` 与前端 `types/qwitch.ts` 对齐；枚举用 `snake_case`/大写错误码。新增字段前端类型与 Rust DTO **必须同步**。
- **单一统计口径**：任何额度/签到/健康度计算只放在 `domain/selectors.ts`（前端）与 `storage::normalize_daily_claim_status`（后端），保持 CST（UTC+8）日界一致，绝不各算各的。
- **防风控铁律**：所有平台请求必须经 `AntiRiskScheduler.queue_lock`（串行），批量用 `try_lock_owned` 重入即拒（返回错误字符串，不是排队）；领取严格“先查后领 + Fail-Closed（仅 `Available` 领）”；401→expired、403/429→risk_paused + 工具级熔断。
- **事件驱动同步**：任何改账号/日志/设置的后端命令结束后必须 `emit qwitch://account-updated` / `qwitch://logs-updated` / `qwitch://settings-updated`，否则主窗口与托盘会不一致。
- **安全**：凭据只经 `storage::crypto` AES-256-GCM 加密落盘，日志必须 `sanitize_log_message`；外部适配器默认不信任、ID 不得冒用 `RESERVED_ADAPTER_IDS`；OAuth 回调校验 nonce + 精确路径 + 官方域名白名单；URL 仅 HTTPS/白名单。
- **前端**：`AccountCard` 用 `React.memo`；需要焦点管理的模态用 `ui/Modal`；Toast 分级 `success/error/warning/info`；托盘与主窗口都需挂载 `ToastViewport`；Tauri/Mock 双分支统一在 `isTauriRuntime` 判断（Mock 用 `mockStore` + sleep）。
- **提交规范**：`feat:` / `fix:` / `chore:`；不擅自 commit/push。UI 文案为简体中文。

---

## 12. 现有技术债 / 待办（按当前源码核验）

**已落地（不必重复做）**：CSP 严格策略、SQLite WAL/busy_timeout、`list_accounts` 单 SQL 消 N+1、`refresh_quota_batch` 入调度器、`delete/update/rename` 广播+审计、主密钥 `key_id`+样本双验证、外部适配器信任门禁、会话备份回滚、切号存活轮询与原子提交、日志归档/保留、`custom_client_paths` 探针穿透。

**仍存在的债（优先级从高到低）**：
1. ~~`commands/mod.rs` 职责过重~~ 已在第三阶段拆分；`storage/mod.rs`（约 2406 行）仍偏大。
2. **`QwitchContext.tsx` 约 1998 行单体 Context**——每次事件全量 `syncFromBackend`；建议拆 `Account/Log/Settings/Feedback` store + 增量 `get_account_summary(accountId)` + 150ms 去抖合并。
3. **`uid_hash` 仍为无盐 `SHA-256(tool_id:uid)`**——建议升级 `HMAC-SHA256(master_key, …)` 并平滑迁移。
4. **AES-GCM 未绑定 AAD**（未绑定 account/tool 上下文）——建议引入 AAD，防跨账号密文替换。
5. **异步上下文仍有同步 `std::process::Command`**（如 `trae.rs` 的 `sysctl`/`sw_vers`，platform 探测分支）——建议 `tokio::process` 或 `spawn_blocking`，避免阻塞 Tokio。
6. **组件级集成测试缺失**：前端只有纯函数测试（无组件/DOM），Tauri bundle 未纳入 CI 构建校验。
7. **文档漂移**：README 测试数量、`docs/*_AUDIT.md` 为历史基线（含大量“已修复”标注），改代码时以源码为准。
8. 其他路线图项：外部插件内容哈希固定、OAuth 凭据走受限 IPC 桥（脱离 URL）、旧版数据迁移用 SQLite backup API、归档日志自动清底、代码签名/公证与自动更新。

---

## 13. 二次开发快速检查清单

- [ ] 新增工具适配器：实现 `ToolAdapter`（含 `manifest`、`import_from_local/parse_manual_token/inspect_quota/claim_daily/apply_session`，必要时 `backup/restore/refresh_credential/daily_claim_device_key`）→ 在 `create_all_adapters` 注册 → 前端 `toolRegistry.ts` 补 `TOOL_BRAND_META` 与 `BUILTIN_TOOL_ORDER` → 图标放 `public/icons/`。
- [ ] 新增 IPC 命令：Rust `#[tauri::command]` + 在 `lib.rs` `generate_handler!` 注册 + `types/qwitch.ts` 加类型 + `QwitchContext` 封装 + 相关事件 emit。
- [ ] 新增设置项：`AppSettingsDto` + `DEFAULT_APP_SETTINGS` + `normalizeAppSettings/sanitizeAppSettingsPatch` 同步 + 设置页 UI。
- [ ] 改统计口径：只动 `domain/selectors.ts`（与后端 `normalize_daily_claim_status` 对齐），补 `selectors.test.ts`。
- [ ] 提交前：`npm test`、`npm run build`、`cargo test`、`cargo fmt --check`、`cargo clippy -D warnings` 全绿。
