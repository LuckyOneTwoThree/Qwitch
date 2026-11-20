# Qwitch 项目理解报告

> **报告性质**：全项目深度认知基线（只读分析，未修改任何业务代码）
> **分析日期**：2026-11-20
> **代码基线**：`main` @ `9b4e31c`，版本 `0.3.0`
> **分析方式**：源码逐层阅读 + 四项自动化门禁实测 + 数据库查询计划验证 + 构建产物核验
> **可信度约定**：本文所有结论均标注 `文件:行` 证据；标 **[实测]** 者为本次亲自验证，标 **[推断]** 者需进一步确认

---

## 0. 摘要与关键结论

### 0.1 一句话定位

Qwitch 是一个**本地优先（Local-First）的桌面端 AI 工具多账号会话与额度管理器**：把 Trae CN / WorkBuddy / Qoder CN / MiniMax Code CN 四个 AI 编码客户端的「多账号额度水位 + 每日签到领取 + 一键冷切换登录态」收进一个托盘应用，全部数据留在本机，无云端、无遥测。

### 0.2 工程成熟度判断

**这是一个成熟度显著高于同类个人项目的代码库**，但存在两个结构性短板。

**做得好的地方（有实测证据）**：

| 维度 | 证据 |
| --- | --- |
| 加密设计 | AES-256-GCM 随机 nonce + 每账号 AAD 绑定 `tool_id:account_id`、HMAC-SHA256 加盐 UID 指纹、密文样本+指纹双验证选钥、Fail-Closed 决策矩阵 |
| 防风控纪律 | 单队列串行 + Jitter + 幂等先查后领（仅 `Available` 才领）+ 工具级熔断 + 设备日限 + 跨设备冷却，全部有行为测试 |
| 切号事务性 | 停进程 → 备份 → 写态 → 重启 → 45s 存活双探测 → 失败回滚，含 WAL/SHM sidecar 保护 |
| 错误码体系 | 统一 `UnifiedErrorCode`，401→Expired、403/429→RiskPaused + 熔断，前后端口径一致 |
| 日志脱敏 | 单一写入口 + 统一 sanitizer（邮箱/手机/JWT/Bearer/KV），导出文件 0600 |
| 质量门禁 | `cargo fmt` / `clippy -D warnings` / 86 Rust 测试 / 67 前端测试 / `npm audit` 全绿 **[实测]** |
| 发布流水线 | 标签与三处版本号强一致校验、防覆盖已有 Tag、架构 slice 校验、SHA256SUMS |

**两个结构性短板**：

1. **「渲染进程 → 原生能力」缺少最小权限边界**（安全面）
   34 个 IPC 命令全量暴露、无命令级 ACL、`custom_client_paths` 仅校验「绝对路径且存在」，与 `launch_tool_profile` 组合可达成**任意可执行文件执行**；外部插件「信任」决策完全由前端入参驱动。一次前端 XSS 即可升级为「任意代码执行 + 全量凭据解密」。

2. **状态层的「假优化」与主线程阻塞**（性能面）
   四个 store 已拆分，但被合并进**单个** 55 字段 Context，且 store 每次渲染返回新对象字面量 → `useMemo` 依赖恒变 → `React.memo` 完全失效。同时 19 个同步 `#[tauri::command]` 在**主线程**做 SQLite 与进程探测 I/O。

### 0.3 关键数字 **[实测]**

| 指标 | 数值 | 备注 |
| --- | --- | --- |
| 总代码量 | ~31,500 行 | `src/` 10,831 + `src-tauri/src/` 20,702 |
| 内置适配器 | 4 个 | trae_cn / workbuddy / qoder_cn / minimax_code_cn |
| IPC 命令 | 34 个 | `lib.rs` `generate_handler!` |
| 后端事件 | 6 个 | 前后端事件名 100% 对齐 |
| Rust 测试 | 86 例全通过 | 与文档声称一致 |
| 前端测试 | **67 例 / 16 文件** | README 写 54、审计文档写 66，**均已漂移** |
| 生产路径 panic 点 | **3 处**（均不可达） | 极低，优于绝大多数 Rust 项目 |
| `npm run build` | 通过 | 1604 模块，347 KB JS / 97 KB gzip |
| `cargo fmt` / `clippy -D warnings` | 零告警 | — |
| `npm audit`（含 dev） | 0 漏洞 | — |

---

## 1. 项目目标与业务场景

### 1.1 解决的三个真实痛点（`docs/ARCHITECTURE.md:1.2`）

1. **状态切换成本高**：AI 桌面客户端同时只维护单账号会话，多账号需反复登出重登或手工备份目录。
2. **额度分散黑盒化**：各平台免费额度、每日签到积分、重置周期割裂，无统一水位监控。
3. **脚本工具门槛高且不安全**：命令行脚本无 UI、常明文存凭据、易因高频轮询触发风控。

### 1.2 明确的非目标（Non-Goals，`docs/ARCHITECTURE.md:1.3`）

- 不做云端同步服务、不上传凭据
- 不做无人值守高频挂机（自动刷新/自动签到**默认关闭**，自动签到需二次确认）
- 不默认篡改设备指纹
- 不内置闭源核心依赖

> **产品哲学**：**Human-in-the-Loop**。所有敏感动作由用户主动触发。这是全代码库最一致的设计约束——`AppSettingsDto` 的 `auto_refresh_interval_minutes` 默认 `0`、`auto_claim_daily` 默认 `false`，并有专门测试固化（`domain/mod.rs:586-597`）。

### 1.3 用户画像与核心场景

| 场景 | 触发入口 | 频率 |
| --- | --- | --- |
| 托盘速览额度、快速切号 | 菜单栏图标 → `TrayPopover` | 高频（每日多次） |
| 批量领取每日福利 | Dashboard「一键领取」 | 每日 1 次 |
| 账号池管理（导入/改名/停用/删除） | Dashboard 主窗口 | 低频 |
| 冷切换客户端登录态 | 卡片「切换」→ 确认弹窗 | 中频 |
| 排查失败原因 | 日志视图（归档/筛选/导出） | 按需 |

---

## 2. 技术栈

| 层 | 技术选型 | 版本 |
| --- | --- | --- |
| 桌面框架 | Tauri v2（`tray-icon`、`macos-private-api`、`single-instance`） | tauri 2.12.6 |
| 前端 | React 18 + TypeScript（strict）+ Tailwind CSS 4 + Vite 6 | — |
| 前端测试 | Vitest 5 + jsdom 26 + @testing-library/react | — |
| 后端 | Rust 2021（MSRV 1.88）+ tokio | — |
| 存储 | rusqlite（bundled SQLite，WAL） | 0.32 |
| 加密 | aes-gcm 0.10 + sha2 + hmac + rand | — |
| HTTP | reqwest（rustls-tls，**不用 OpenSSL**） | 0.12 |
| 密钥托管 | keyring v3（apple-native / windows-native） | 3.6.3 |
| 其他 | chrono、uuid、base64、zip 2（纯 Rust 解压）、rfd | — |

### 2.1 技术选型的三个值得注意的决策

1. **`reqwest` 锁 0.12 而非 0.13**：`docs/PROJECT_AUDIT_REPORT.md` 明确记录，为沿用 `ring` 后端、避免 Windows CI 引入 `aws-lc-rs` 的 C 构建依赖。**代价**：`Cargo.lock` 中同时存在 `reqwest 0.12`（直接）与 `0.13`（Tauri 传递），编译体积翻倍。
2. **`rusqlite` 用 `bundled`**：SQLite 随二进制分发，无系统依赖，但增大包体。
3. **`zip` 用纯 Rust 而非系统 `ditto`/`unzip`**：这是安全决策——避免命令注入与 zip-slip，代价是需要自行实现解压上限（已实现：4096 条目 / 单文件 16 MiB / 总量 160 MiB，`commands/profile.rs:68-182`）。

---

## 3. 目录结构与代码规模

```
Qwitch/
├─ src/                          # 前端 10,831 行
│  ├─ App.tsx                    # 双窗口分流（main / tray）+ Mock 外壳
│  ├─ pages/
│  │  ├─ Dashboard.tsx (1154)    # 主窗口：导航 / 账号池 / KPI
│  │  ├─ TrayPopover.tsx (573)   # 托盘弹层
│  │  └─ dashboard/              # LogsView(595) / SettingsView(648) / types
│  ├─ context/                   # 状态中枢
│  │  ├─ QwitchContext.tsx (344) # 仅 Provider 装配
│  │  ├─ useAccountStore.ts (1353)  ← 最大文件，核心编排
│  │  ├─ useLogStore.ts (359) / useSettingsStore.ts (224) / useFeedbackStore.ts (74)
│  │  └─ logStateUtils / settingsUtils / useToast / useAppUpdate
│  ├─ domain/                    # selectors.ts（唯一统计口径）/ toolRegistry.ts
│  ├─ components/                # AccountCard(674) / ImportAccountModal(606) / ...
│  └─ data/mockStore.ts (364)    # 浏览器预览 Mock
├─ src-tauri/src/                # 后端 20,702 行
│  ├─ lib.rs (388)               # 启动编排 / 托盘 / 窗口事件 / 34 命令注册
│  ├─ commands/                  # mod / accounts(702) / oauth(1087) / profile(494) / settings / switch / logs
│  ├─ adapter/                   # trae(2326) workbuddy(2014) minimax(1837) qoder(1250) external(830) common
│  ├─ scheduler/mod.rs (1413)    # 防风控串行调度器
│  ├─ platform/mod.rs (1766)     # PlatformHost：钥匙串/进程/路径/自启
│  ├─ storage/                   # mod(1223) crypto(609) logs settings tests(1197)
│  ├─ domain/mod.rs (598)        # 领域模型 + CredentialEnvelope
│  └─ update.rs (707)            # GitHub Release 更新
├─ docs/                         # 11 份文档，2,034 行
├─ scripts/                      # 3 个运维脚本（335 行）
└─ .github/workflows/            # ci.yml + release.yml
```

### 3.1 已完成的架构拆分（值得肯定）

审计报告记录的「上帝文件」治理已实质落地 **[实测]**：

| 文件 | 历史规模 | 当前 | 说明 |
| --- | --- | --- | --- |
| `commands/mod.rs` | 3,261 行 | 388 行 | 拆为 6 个域子模块 |
| `QwitchContext.tsx` | 2,045 行 | 344 行 | 拆为四 store |
| `Dashboard.tsx` | 2,341 行 | 1,154 行 | 日志/设置视图外移 |
| `storage/mod.rs` | 2,294 行 | 1,223 行 | 拆出 logs/settings/tests |

**但**：拆分只做了「文件级」拆分，未做「订阅级」拆分——见 §9.2。

---

## 4. 核心模块

### 4.1 四层架构

```
┌──────────────── UI 交互层（React 18 / TS strict / Tailwind 4）────────────────┐
│ Dashboard 主窗口 · TrayPopover 托盘 · Context 状态中枢 · selectors 统一口径   │
└───────────────────────────────▲─────────────────────────────────────────────┘
                                │ Tauri IPC：34 commands + 6 events
┌───────────────────────────────▼─────────────────────────────────────────────┐
│ 核心调度与防风控层（Rust）                                                    │
│  AntiRiskScheduler：单队列串行 + Jitter + 幂等 + 工具级熔断 + 设备日限         │
│  switch_desktop_account：冷切换协调器（停→备→写→启→验→回滚）                  │
└───────────────────────────────▲─────────────────────────────────────────────┘
                                │ ToolAdapter trait（async）
┌───────────────────────────────▼─────────────────────────────────────────────┐
│ 适配器层                                                                     │
│  内置：trae_cn / workbuddy / qoder_cn / minimax_code_cn                       │
│  外部：manifest.json + stdin/stdout JSON-RPC 2.0（默认不加载，需显式信任）    │
└───────────────────────────────▲─────────────────────────────────────────────┘
                                │
┌───────────────────────────────▼─────────────────────────────────────────────┐
│ 存储与平台基础设施层                                                          │
│  QwitchDatabase：SQLite WAL + AES-256-GCM + 日志归档 + PII 迁移               │
│  PlatformHost：钥匙串 / 进程探测 / 路径解析 / 开机自启                        │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 4.2 后端核心模块职责

| 模块 | 职责 | 关键设计 |
| --- | --- | --- |
| `lib.rs` | 启动编排：开库 → 取密文样本 → 选主密钥（指纹+样本双验证）→ 三个迁移 → 建适配器 → `AppState` → 托盘 | 启动失败用 `rfd` 原生弹窗给出恢复指引（`lib.rs:355-380`） |
| `commands/` | 34 个 IPC 命令，按域拆 6 子模块 | `resolve_client_executable` 统一 `custom_client_paths` 覆盖 |
| `scheduler/` | 批量领取/查额引擎 | `SchedulerEventSink` trait 注入事件端，使行为可测（5 个行为测试） |
| `adapter/` | 4 内置 + 外部插件 | `ToolAdapter` trait 12 个方法（6 必须 + 6 默认） |
| `platform/` | `PlatformHost` 抽象 | macOS/Windows 分支：pgrep/pkill vs tasklist/taskkill、LaunchAgent vs 注册表 |
| `storage/` | SQLite + 加密 + 迁移 | 单连接 `Mutex<Connection>`、WAL、`QWA1` 版本化密文 |
| `update.rs` | GitHub Release 检查/下载/校验/安装 | 逐跳重定向白名单（≤5）、SHA256 强制、200 MiB 上限 |

### 4.3 适配器能力矩阵

| 适配器 | 凭据解密 | 查额 | 签到 | 切号 | 刷新令牌 | 每日设备限 |
| --- | --- | :---: | :---: | :---: | :---: | :---: |
| `trae_cn` | 自研 `tc`：Base64 → magic → seed → SHA512 → AES-128-CBC（Rust 内闭环） | ✅ | ✅ | ✅ | — | ✅（码 9095） |
| `workbuddy` | `sym-v1` AES-256-GCM，**借本机 Electron 运行时解密**（`ELECTRON_RUN_AS_NODE`） | ✅ | ✅ | ✅ | — | ❌ |
| `qoder_cn` | Chromium `v10`：Windows 走 DPAPI；**macOS 仅剥离前缀**（占位实现） | ✅ | ✅ | ✅ | — | ❌ |
| `minimax_code_cn` | 明文 `auth.json` + 自建 OAuth 2.0 设备码 + PKCE | ✅ | ✅ | ✅ | ✅ | ❌ |

> **注意**：`DeviceDailyLimitReached` 只有 Trae 会产出，调度器的设备级短路（`scheduler/mod.rs:288-300`）对其余三者**实际不生效**。

### 4.4 前端核心模块职责

| 模块 | 职责 | 备注 |
| --- | --- | --- |
| `QwitchContext.tsx` | 仅 Provider 装配（344 行） | 合并四 store → 单 Context |
| `useAccountStore` | 账号/切号/查额/领签/事件编排 | **1,353 行，全仓库最大且零测试** |
| `useLogStore` | 日志/游标分页/归档 | 有完整 `seq + scope` 竞态守卫 |
| `useSettingsStore` | 设置/主题/抖动/保留 | 保存链式串行 + 脏标记保护 |
| `useFeedbackStore` | toast/更新/批量进度/切号弹窗 | — |
| `domain/selectors.ts` | **唯一统计口径** | 与后端 `normalize_daily_claim_status` 对齐 CST 日界 |
| `domain/toolRegistry.ts` | 品牌元数据 + 可见工具合并 | 支持外部插件动态展示 |

---

## 5. 关键业务流程

### 5.1 导入账号（三条路径）

```
用户选择工具 + 方式
  ├─ local_import  → import_local_account → adapter.import_from_local(host)
  │                    → CredentialEnvelope → upsert_account（AES 加密落库）
  ├─ token_paste   → add_account_by_token → adapter.parse_manual_token
  └─ webview_oauth → start_webview_oauth → 独立 WebView 窗口
                       → 导航白名单校验 + nonce + flow_id
                       → 回调 qwitch.local/oauth-callback#nonce=..&payload=..
                       → 进程内拦截（不发网络请求）→ 解析 fragment
  → schedule_import_quota_check（复用串行锁，异步）
  → emit qwitch://account-updated
```

**关键设计**：凭据只经 URL **fragment** 传递（不发送到任何服务器），且显式拒绝从 query 读取（`commands/oauth.rs:680-695`）。这是正确做法。

### 5.2 每日批量领取（防风控核心）

```
claim_daily_batch(ids)
  → 上限 200 + 去重 + Trae 活跃账号排前
  → scheduler.queue_lock().try_lock_owned()   ← 重入即拒绝（非排队）
  → tokio::spawn → run_daily_claim_batch
       for each account（严格串行）:
         1. 读凭据 + 状态/能力/熔断/设备日限校验
         2. refresh_credential（按需轮换令牌）
         3. 跨设备冷却（切换 device 身份时额外抖动）
         4. inspect_quota ← 幂等前置（Fail-Closed）
         5. 按 ClaimStatus 分流：
              Claimed/Unsupported/Unknown → 跳过并记日志
              Available → claim_daily(precheck)
         6. 失败映射：401→Expired；403/429→RiskPaused + 工具熔断
         7. 账号间 Jitter（默认 1500–3500ms，后端强制下限 500ms）
  → emit batch-progress（每阶段）+ account-updated（每账号）+ logs-updated
```

**这是全项目质量最高的模块**，且有 5 个行为测试覆盖：幂等/Fail-Closed、工具级熔断隔离、设备日限 + 跨设备冷却、凭据轮换 + 限流熔断、队列互斥。

### 5.3 冷切换（事务性最强）

```
switch_desktop_account(account_id, force_kill)
  → SwitchFlagGuard（AtomicBool CAS）  ← 并发互斥
  → scheduler.queue_lock().try_lock_owned()  ← 与批量任务互斥
  → 校验适配器能力 + 解析可执行文件（custom_client_paths 优先）
  → 检测客户端运行态 → 运行中则 terminate_client_gracefully(5000ms)
  → backup_session()  ← 备份失败则重拉原客户端并中止
  → apply_session()   ← 失败则 restore_session + 重拉 + 中止
  → launch_client()
  → 45s 存活轮询（700ms × 64 次，连续两次探测存活才确认）
  → 迟到进程二次确认（超时后才出现的进程视为成功，不回滚）
  → set_active_on_client()（失败重试一次）
  → emit switch-progress + account-updated + logs-updated
```

**亮点**：`restore_files` 对 SQLite sidecar 有专门保护——主库恢复失败时**保留 `-wal`**，避免破坏数据库（`adapter/mod.rs:159-200`，有测试）。

### 5.4 软件更新

```
check_app_update → GitHub API /releases/latest（失败降级 302 跳转）
  → 按 OS/arch 选资产 → 返回版本/说明/大小/checksumsUrl
download_and_install_update（仅 Windows）
  → 校验资产名 .exe → 校验仓库前缀 → 下载（200 MiB 上限）
  → verify_checksum（SHA256SUMS.txt 强制，失败中止）
  → 写临时文件 → spawn 安装程序 → 900ms 后退出
macOS → 只提供 open_update_url 指引（不做应用内自替换）
```

---

## 6. 数据流与接口关系

### 6.1 单向数据流

```
前端 invoke(command)
   ↓
命令读 AppState { db, master_key, host, adapters, scheduler, ... }
   ↓
（批量操作）scheduler 串行执行 → 适配器 → 外部 API / 本地文件
   ↓
落库（SQLite，凭据 AES-GCM 加密）
   ↓
app.emit("qwitch://...")
   ↓
前端 listen → 去抖（100–120ms）→ 增量/全量刷新 → setState → 重渲染
```

### 6.2 事件契约（前后端 100% 对齐 **[实测]**）

| 事件 | 载荷 | 前端消费 |
| --- | --- | --- |
| `qwitch://batch-progress` | `action/total/completed/phase/jitterSeconds/summaryText` | `BatchClaimBanner`、`TrayPopover` |
| `qwitch://account-updated` | `account_id` | 定向刷新单卡（取不到 id 回退全量） |
| `qwitch://logs-updated` | `()` | 刷新日志 + 统计 |
| `qwitch://settings-updated` | `()` | 跨窗口设置同步 |
| `qwitch://switch-progress` | `account_id/phase` | `ColdSwitchModal` 三阶段 |
| `qwitch://oauth-error` | 错误消息 | Toast |

### 6.3 核心数据模型

**`CredentialEnvelope`**（加密落盘核心）：
```
tool_id + uid + display_name/email
  + payload: Token{access, refresh, expires_at, extra}
           | Cookies{cookies, user_agent}
           | LocalSnapshot{files: HashMap<String, Base64>}
```
`capability_flags()` 由 payload 推导 `has_local_snapshot` / `has_api_token`，决定卡片「就绪 / 待授权」。

**SQLite Schema**：

| 表 | 关键约束 | 索引 |
| --- | --- | --- |
| `accounts` | `UNIQUE(tool_id, uid_hash)`、`encrypted_credential BLOB NOT NULL` | `idx_accounts_tool` |
| `quota_snapshots` | `FK account_id ON DELETE CASCADE` | `idx_quota_account(account_id, fetched_at DESC)` |
| `action_logs` | 无外键、`AUTOINCREMENT` | `idx_logs_created`、`idx_logs_archived_id` |
| `app_settings` | KV + `updated_at` | — |

**PRAGMA**：`foreign_keys=ON`、`journal_mode=WAL`、`synchronous=NORMAL`、`busy_timeout=5000`（`storage/mod.rs:98-101`）

---

## 7. 配置依赖与构建运行

### 7.1 构建与运行

```bash
npm install                # Node ≥ 22
npm run dev                # 仅前端（浏览器 Mock 预览，无 Tauri）
npm run tauri dev          # 桌面应用开发（Vite:1420 + Rust）
npm run build              # tsc + vite build
npm test                   # Vitest
cd src-tauri && cargo test --locked
cargo fmt --check && cargo clippy --all-targets -- -D warnings
npm run tauri build        # 打包
```

**版本号三处必须同步**：`package.json` / `src-tauri/Cargo.toml` / `src-tauri/tauri.conf.json`（当前均 `0.3.0` **[实测]**），`release.yml` 有强校验。

### 7.2 关键配置

| 配置 | 值 | 影响 |
| --- | --- | --- |
| CSP | `script-src 'self'`、`object-src 'none'`、`base-uri 'self'`、`frame-ancestors 'none'`、`form-action 'self'` | 强；仅 `style-src 'unsafe-inline'` 一处放松 |
| capabilities | 仅 `core:default`，**无 `permissions/` 目录** | ⚠️ 无命令级 ACL（见 §9.1 P0-2） |
| 数据目录 | macOS `~/Library/Application Support/qwitch`<br>Windows `%APPDATA%\qwitch` | 避开 macOS TCC |
| 托盘窗口 | 384×500、transparent、alwaysOnTop、skipTaskbar | 自定义定位算法（多显示器 + 缩放） |
| 默认设置 | 抖动 1500–3500ms、日志归档 14 天/200 条、自动刷新 **关**、自动签到 **关** | Human-in-the-Loop |

### 7.3 CI/CD

| 流水线 | 触发 | 内容 |
| --- | --- | --- |
| `ci.yml` | push/PR → main | macOS + Windows 矩阵：`npm ci` → `npm audit --omit=dev` → build → test → `bash -n` 脚本 → `cargo fmt` → `clippy -D warnings` → `cargo test` → `tauri build --no-bundle` |
| `release.yml` | `v*` tag | verify（版本一致性 + Tag 防覆盖）→ build（macOS arm64 / macOS x64 / Windows x64）→ 架构 slice 校验 → SHA256SUMS → `gh release` |

**发布流水线质量高于平均水平**：有版本-标签强一致性门禁、防覆盖已有 Tag、`lipo -archs` 架构校验、macOS 签名显式 opt-in（`vars.MACOS_SIGNING_ENABLED`）。

---

## 8. 质量基线实测

| 门禁 | 结果 | 备注 |
| --- | --- | --- |
| `npm run build`（tsc + vite） | ✅ 通过 | 1604 模块，347 KB JS / 97 KB gzip |
| `npm test`（Vitest） | ⚠️ **32 文件 / 133 例** | **含重复目录 `qwitch/` 的 16 文件 / 66 例** |
| `npm test`（排除 `qwitch/`） | ✅ **16 文件 / 67 例** | 真实值 |
| `cargo test --locked` | ✅ **86 例全通过** | 0.37s |
| `cargo fmt --check` | ✅ 零告警 | — |
| `cargo clippy --all-targets -D warnings` | ✅ 零告警 | — |
| `npm audit`（含 dev） | ✅ 0 漏洞 | — |
| `bash -n scripts/*.sh` | ✅ 通过 | — |
| 生产路径 panic 点 | ✅ 仅 3 处，均不可达 | `minimax.rs:693`、`crypto.rs:143`、`storage/mod.rs:1080` |

### 8.1 测试分布 **[实测]**

**Rust（86 例）**：`adapter` 38、`storage` 24、`commands` 8、`scheduler` 5、`update` 5、`domain` 3、`platform` 3

**前端（67 例）**：`selectors` 15、`SettingsView` 7、`settingsUtils` 6、`AccountCard` 5、`Dashboard` 5、`useAppUpdate` 4、`useToast` 4、`logStateUtils` 3、`toolRegistry` 3、`ToastViewport` 3、`ColdSwitchModal` 2、`ImportAccountModal` 2、`ErrorBoundary` 2、`mockStore` 2、`TrayPopover` 2、`LogsView` 2

**无测试的关键文件**：`useAccountStore.ts`（1,353 行）、`QwitchContext.tsx`、`useLogStore.ts`、`useSettingsStore.ts`、`lib.rs`、`commands/mod.rs`、`commands/logs.rs`、`commands/settings.rs`、`storage/logs.rs`、`storage/settings.rs`

---

## 9. 潜在问题、技术债与风险点

> 分级：**P0** = 安全红线 / 数据不可逆 / 核心功能阻断；**P1** = 高危或显著体验损伤；**P2** = 中危 / 技术债

### 9.1 P0 — 需优先处理

#### P0-1｜渲染进程 → 任意可执行文件执行（完整 RCE 链）**[实测]**

**证据链**：
1. `commands/settings.rs:109-124` — `custom_client_paths` 仅校验 `p.is_absolute() && p.exists()`，**不校验 basename、不校验可执行性、不校验目录**
2. `commands/profile.rs:371-387` — `launch_tool_profile` 取该路径并 `host.launch_client(&exe, &args)`
3. `platform/mod.rs:1077-1081` — `Command::new(executable).spawn()`
4. 无命令级 ACL（`capabilities/default.json` 仅 `core:default`，无 `src-tauri/permissions/`）

**攻击路径**：`save_app_settings({customClientPaths:{trae_cn:"/bin/sh"}})` → `launch_tool_profile({tool_id:"trae_cn", profile_dir:"/tmp/x"})` → 任意二进制以当前用户权限执行。

**影响**：任何能在 `main`/`tray` webview 执行脚本的路径（前端 XSS、供应链污染、被信任插件的 UI 注入）都可跃迁为「用户态代码执行 + 全量凭据解密」。

**修复方向**：`custom_client_paths` 只接受 `pick_file` 签发的一次性 token；Rust 侧 `canonicalize` + basename/目录白名单双校验；补 `permissions/*.toml` 最小权限 ACL。

---

#### P0-2｜外部插件信任由前端自我批准 + 插件无文件系统隔离 **[实测]**

**证据**：
1. `commands/settings.rs:135-202` — `trusted_adapters` / `trusted_adapter_hashes` **完全来自前端入参**；Rust 只把请求哈希与「磁盘现值」比对后固定。即「信任 = 前端说信任 + 磁盘现值」，**无独立于渲染进程的用户确认**
2. `adapter/external.rs:460-487` — `is_valid_entry_command`：**绝对路径无条件放行**（`472-474`）；相对路径不存在时**也放行**（fail-open，`480-484`）
3. `adapter/external.rs:21-38` — `env_clear()` 后仍继承 `HOME`/`USERPROFILE`/`APPDATA`/`LOCALAPPDATA`
4. 插件内容哈希只覆盖目录内文件，**不覆盖实际被执行的二进制**

**影响**：`data_dir/adapters/<id>/` 一旦被写入，渲染进程可一步完成「信任 + 哈希固定 + 下次启动注册」，插件随后继承 `HOME` 直接读 `Trae CN/.../storage.json` 与 `qwitch.db`，离线解密全部凭据。

**修复方向**：信任改为 Rust 侧 `begin_trust_request` → 原生对话框确认 → `commit_trust_request`；`is_valid_entry_command` 改 fail-closed；插件上平台沙箱（macOS `sandbox-exec` / Windows AppContainer）。

---

#### P0-3｜主密钥唯一副本 + `key_id` 非原子写 → 不可恢复的凭据全损

**证据**：
1. `platform/mod.rs:716-737` — 钥匙串迁移成功后**删除明文 `master.key`** → 密钥唯一副本在系统钥匙串
2. `storage/mod.rs:871-877` — `key_id` 用 `std::fs::write` **非原子写**，chmod 在其后
3. `storage/mod.rs:852-862` — `key_id` 为空/截断时走「指纹冲突，拒绝启动」分支，**密钥其实正确**

**影响**：重装系统 / 钥匙串被清 / 断电导致 `key_id` 截断 → 应用拒绝启动，且**无导出/备份入口**，凭据永久不可恢复。

**修复方向**：`key_id` 改 tmp + `fsync` + `rename`；空/非法内容视为「缺失」而非「冲突」；首次托管成功后强制生成加密导出备份或在 UI 明示风险。

---

#### P0-4｜旧格式密文的 AAD 绑定可被完全绕过

**证据**：
1. `storage/crypto.rs:101-111` — 解密时若无 `QWA1` 前缀则**忽略 AAD**，走兼容路径
2. `storage/mod.rs:637-644` — `migrate_credential_aad` 对解不开的行 `Err(_) => continue`，**永久停留旧格式**

**影响**：任何仍是旧格式的凭据行，跨账号密文移植依然可解密 —— AAD 防串号保护对历史数据等于不存在。

**修复方向**：迁移失败行写入 `credential_format='legacy'` 并在启动时告警/强制升级，而非静默跳过。

---

#### P0-5｜三个安全迁移失败仅 `eprintln!`

**证据**：`lib.rs:79-87` — `migrate_uid_hash` / `migrate_credential_aad` / `migrate_legacy_pii` 失败仅打印「不影响启动」，应用照常启动。

**影响**：安全升级（HMAC 换算法、AAD 重封装、PII 脱敏）静默未生效，用户无任何提示。

---

### 9.2 P1 — 高危 / 显著体验损伤

#### P1-1｜Context 单点广播导致全树重渲染（`React.memo` 假优化）**[实测]**

**根因（三处叠加）**：
1. `QwitchContext.tsx:104` — 全应用仅**一个** Context，value 含 **55 个字段**
2. `QwitchContext.tsx:221-335` — `useMemo` 依赖 **54 项**，任一变化整体重建；且**漏了 `setLogScope`**（对象里有 `:229`）
3. `useAccountStore.ts:1327` / `useLogStore.ts:210` / `useFeedbackStore.ts:60` — **每次渲染返回新的对象字面量** → 作为 `useMemo` 依赖恒变 → memo 永远失效

**放大机制**：单账号领取触发 3–4 次 `batchProgress` setState（`useAccountStore.ts:494/520/651/734`）+ 1 次 `account-updated` → 200 账号批量 = ~200 事件 + ~600 进度事件 → **每次事件都重建 Context** → Dashboard + 200 张 AccountCard + TrayPopover 全量重渲染。

**`React.memo` 完全失效**：`AccountCard.tsx:41` 有 memo，但 `:48-62` 立刻 `useQwitch()` 解构 12 个字段 → Context 更新绕过 props 直穿 memo。

**修复方向**：actions 与 state 拆成两个 Context（actions 用 `useMemo(() => ({...}), [])` 稳定）；或引入 `useSyncExternalStore` 做订阅粒度；`AccountCard` 改为 props 驱动 + 只在必要字段变化时更新。

---

#### P1-2｜Mock 数据静态打进生产包，探测失败即静默用假数据 **[实测]**

**证据**：
1. `useAccountStore.ts:4` / `useLogStore.ts:3` — **静态 import** mockStore
2. `useAccountStore.ts:76-84` — 初始值直接取 `INITIAL_ACCOUNTS` / `MOCK_ADAPTERS`
3. `QwitchContext.tsx:106-107` — 判定仅一行 `"__TAURI_INTERNALS__" in window`
4. **产物已核验**：`dist/assets/index-CD8xytNB.js` 中可检出 `acc-minimax-1`、`Trae CN · 主力工作号`、`9390****7536`

**影响**：探测失效（如 Tauri 注入时机变化）时，用户对着假账号点「领取」，走 `sleep(450)` + `bonus=100` 假成功路径，产生假 Toast 与假日志。

**修复方向**：mock 改动态 `import()` + 构建期常量 DCE；或仅在 `import.meta.env.DEV` 分支引入。

---

#### P1-3｜批量任务互斥存在竞态窗口

**证据**：
1. `useAccountStore.ts:476-478` — 前端锁在 `invoke` 返回即释放
2. `commands/accounts.rs:612-621` — 后端 `tokio::spawn` 后立即 `Ok(())`
3. 首个 `batch-progress` 尚未抵达时 `batchProgress?.running` 仍为 `null` → 二次点击穿透双守卫 → 被后端 `try_lock` 拒绝 → 用户看到红色 Toast

**修复方向**：后端获取锁后**立即**发初始 `batch-progress`；前端配合 ref 乐观锁。

---

#### P1-4｜同步命令在主线程做阻塞 I/O **[实测]**

**证据**：
1. `tauri-macros-2.7.5/src/lib.rs:48-56` — 明确「sync commands run on the main thread」
2. 19 个同步命令中，以下均在主线程做 I/O：
   - `list_accounts` / `get_account_summary`（`commands/accounts.rs:14,27`）— SQLite 查询
   - `get_action_log_stats`（`commands/logs.rs:27` → `storage/logs.rs:104-124`）— **无 WHERE 的 `SUM(CASE...)` 全表扫描**（`get_action_logs` 本身有 keyset 游标，但仍是主线程同步执行）
   - `export_action_logs`（`commands/logs.rs:76`）— 一次拉 10,000 条 + 序列化 + 写盘
   - `detect_client_status`（`commands/settings.rs:220`）— `is_client_running` → `pgrep`/`tasklist`，**无超时**
3. `lib.rs:75` — keyring 访问在 `setup` 主线程（可能弹钥匙串授权框）

**影响**：大账号量或钥匙串弹窗时 UI 卡死。

**修复方向**：改 `#[tauri::command(async)]` 或内部 `spawn_blocking`；`pgrep`/`tasklist` 加超时；keyring 移出主线程。

---

#### P1-5｜数据库索引与 `ORDER BY` 失配，每次查询触发 TEMP B-TREE **[实测]**

**证据**：
1. `storage/mod.rs:434,462,482,1022` — `ORDER BY julianday(q2.fetched_at) DESC, ...`
2. `storage/mod.rs:152` — 索引为 `idx_quota_account(account_id, fetched_at DESC)`
3. **查询计划实测**：`USE TEMP B-TREE FOR ORDER BY`

**影响**：`list_accounts` / `get_account_summary` 用相关子查询，**每行一次排序**。`prioritize_active_trae_account_on_client` 对每个 id 调一次 `get_account_summary`（`commands/accounts.rs:568-579`），最多 200 次串行查询 + 200 次排序。

**修复方向**：`fetched_at` 改 `INTEGER epoch_ms` 并建 `(account_id, fetched_ms DESC)`；或改用 `ROW_NUMBER()` 窗口函数。

---

#### P1-6｜WAL checkpoint 仅在迁移路径调用，运行期无界增长 **[实测]**

**证据**：全项目仅一处 `PRAGMA wal_checkpoint(TRUNCATE)`（`storage/mod.rs:741`），在 `migrate_legacy_pii` 内且被 `let _ =` 吞。无周期性 checkpoint、无 `journal_size_limit`、退出时不 checkpoint。

**实测佐证**：本机数据目录 `qwitch.db` 96 KB，而 `qwitch.db-wal` **2.0 MB**（20 倍）。

**修复方向**：增加周期性 `wal_checkpoint(PASSIVE)` + `journal_size_limit`；退出时 `TRUNCATE`。

---

#### P1-7｜更新可被降级安装（downgrade）**[实测]**

**证据**：
1. `update.rs:281` — `update_available: latest > current` **只用于展示**
2. `update.rs:441-505` — `download_and_install_update` 接收前端传入的 `asset_url`/`checksums_url`，**不校验目标版本与当前版本关系**
3. `update.rs:89-110` — `validate_official_repo_url` 只要求仓库前缀，**任意历史 tag 均通过**
4. `verify_checksum` 从同一仓库取 `SHA256SUMS.txt` → **旧版哈希同样通过**

**影响**：可把用户降级到含已知漏洞的旧版本（Windows 自动安装路径）。

**修复方向**：安装前重新拉取 latest release，校验资产逐项相等 + `latest > current` + `min_supported_version`。

---

#### P1-8｜PKCE `verifier` 出现在 URL query

**证据**：`adapter/qoder.rs:457-465` — `parse_with_params(..., &[("nonce",nonce),("verifier",verifier),...])`。verifier 应仅在 POST body / header。

**影响**：verifier 进入服务端访问日志、代理、CDN 日志，削弱 PKCE 保护。

---

#### P1-9｜OAuth nonce 在第三方页面上下文可读，无「已消费」语义

**证据**：
1. `commands/oauth.rs:181` — 凭据 URL 由注入脚本在 `trae.cn` 页面内拼接，nonce 以字面量存在（`:632` 替换）
2. `flow_seq` 只做「最新流」判定，**不做「已消费」标记**

**影响**：同页任意脚本（第三方 SDK、被入侵子资源）可读 nonce 并伪造回调，向凭据库注入任意 token → 账号投毒 + 后续 `apply_session` 写入客户端。

**修复方向**：凭据回传改 `invoke`/`postMessage` 专用 bridge；nonce 加一次性消费标记；回调 payload 加 Rust 侧 HMAC。

---

#### P1-10｜`DeviceDailyLimitReached` 仅 Trae 产出，设备级防护对其余三者失效

**证据**：`adapter/trae.rs:1620` 是唯一产出点；`scheduler/mod.rs:288-300` 的设备短路依赖该码。

**影响**：WorkBuddy / Qoder / MiniMax 的多账号同设备领取无设备级拦截。

---

#### P1-11｜macOS Qoder 解密为占位实现，能力静默降级

**证据**：`adapter/qoder.rs:131-139` — 仅剥离 `v10` 前缀后直接 `serde_json::from_slice`，**无 Keychain 调用**（全仓 `grep -i keychain` 在 `adapter/` 下 0 命中）。与 `docs/ADAPTER_SPEC.md:16` 声明的「v10 + AES-256-GCM」不符。

**影响**：macOS 本地导入始终无 `has_api_token` → 用户看到「未完成在线授权」，错误信息完全不指向「平台解密未实现」。

---

#### P1-12｜WorkBuddy 解密子进程从「用户可控路径 / 目录内任意文件」拉起二进制

**证据**：`adapter/workbuddy.rs:200-207` — `Contents/MacOS` 下找不到目标就**取目录里第一个普通文件**执行；路径来自 `settings.custom_client_paths`，**无签名/TeamID 校验**。

**修复方向**：删除「取第一个文件」兜底；自定义路径必须校验 codesign TeamID / Authenticode。

---

### 9.3 P2 — 中危 / 技术债

| # | 问题 | 证据 |
| --- | --- | --- |
| P2-1 | **重复目录 `qwitch/` 未跟踪、未忽略，污染测试** | `git status` 显示 `?? qwitch/`；`npm test` 跑成 32 文件/133 例（真实 16/67） |
| P2-2 | **无 vitest 配置**，默认 glob 扫描全 cwd | `vite.config.ts` 无 `test` 段 → 任何同名副本都会被双跑 |
| P2-3 | **文档漂移** | `ADAPTER_SPEC.md` **完全未提 MiniMax**（`grep -c minimax` = 0）；`ARCHITECTURE.md` 写 `prepare_switch`（实际 `apply_session`）；README 称 54 前端测试（实际 67）、82 Rust（实际 86）；`PROJECT_COGNITION_SUMMARY.md:110` 写「15s 存活轮询」（实际 45s） |
| P2-4 | **`storage/mod.rs` schema 演进靠 `let _ = ALTER TABLE`** | `:156-170` 五条吞错，真实错误（磁盘满/锁）与「列已存在」不可区分 → 可能半迁移 |
| P2-5 | **锁中毒即永久瘫痪** | `Mutex<Connection>` + 统一 `LockPoisoned`，无 `PoisonError::into_inner` 恢复 |
| P2-6 | **`get_action_log_stats` 全表扫描** | `logs.rs:106-112` `SUM(CASE...)`，无可用索引，每次日志刷新都跑 |
| P2-7 | **迁移 TOCTOU** | `migrate_credential_aad` 读快照与写事务**分两次加锁**（`:621-652`），可能用旧 blob 覆盖并发写入的新凭据 |
| P2-8 | **`zeroize` 只覆盖中间 buffer** | `crypto.rs:20-28,66,115`；`CredentialEnvelope` 明文字段、`hex_key` String 未清零 |
| P2-9 | **`mask_uid` 对 7 字符 UID 暴露 6 字符** | `crypto.rs:150-165`，测试还把该值固化（`crypto.rs:528`） |
| P2-10 | **`write_private_file_atomic` 的 Windows 分支无 ACL 收紧** | `adapter/mod.rs:100-104` 仅 `#[cfg(unix)]` |
| P2-11 | **`export_action_logs` 存在 chmod TOCTOU** | `logs.rs:110-115` 先 `fs::write` 后 `set_permissions(0600)` |
| P2-12 | **三个适配器 `merge_post_claim_quota` 字节级重复** | `trae.rs:1017-1042` / `qoder.rs:175-200` / `minimax.rs:825-850` 完全一致 |
| P2-13 | **`host` 注入不一致，绕开 `ScopedPlatformHost`** | `workbuddy.rs:1455,1465` / `qoder.rs:162` 自造 `NativePlatformHost` |
| P2-14 | **无 `[dev-dependencies]`，21 处 HTTP 调用点无法在测试中验证** | `Cargo.toml` 无 `wiremock`/`mockito` |
| P2-15 | **`update.rs` 建 3 个独立 `reqwest::Client`** | `:55,63,73`，而 `adapter/common.rs:10` 已有共享客户端 |
| P2-16 | **`syncFromBackend` 无请求序号守卫** | `useAccountStore.ts:196-226`（对比 `useLogStore.ts:63` 有完整守卫）→ 慢响应覆盖新状态（「删了又回来」） |
| P2-17 | **日志全选 O(n²)** | `Dashboard.tsx:283-285` `visibleLogIds.every(id => selectedLogIds.includes(id))` |
| P2-18 | **无 ESLint** | `package.json` 无 lint 脚本、无配置文件 → `exhaustive-deps` 类问题无护栏（正是 P1-1 漏 dep 的成因） |
| P2-19 | **`trae_probe.sh` 用可预测 PID 命名临时文件 + `umask` 在 `mkdir` 之后** | `:30` vs `:60-61`；token 含 `"` 可逃逸 curl 配置值 |
| P2-20 | **CI 无 `cargo audit`；actions 未按 SHA 固定** | `ci.yml`/`release.yml` 用 `@v4` 与 `dtolnay/rust-toolchain@stable`（可变分支） |
| P2-21 | **`master.key` 明文降级路径无 OS 级保护** | `platform/mod.rs:663-673`，仅 0600 |
| P2-22 | **`inspect_profile_dir` 回显绝对路径** | `profile.rs:302` → 文件系统存在性 oracle |
| P2-23 | **MiniMax OAuth 轮询 `expires_in` 未 clamp + `loop` 无迭代上限** | `minimax.rs:460-465`；`commands/oauth.rs:902` |
| P2-24 | **5xx 映射为 `NetworkError` 不触发熔断** | `common.rs:21-40` + `scheduler/mod.rs:154` → 平台 500 会逐个账号继续打 |
| P2-25 | **无 `Retry-After` 解析、无退避** | 全仓 `grep -i retry-after` 0 命中 |

---

## 10. 性能瓶颈汇总

| # | 瓶颈 | 位置 | 量级 | 优先级 |
| --- | --- | --- | --- | --- |
| 1 | Context 单点广播 → 全树重渲染 | `QwitchContext.tsx:221-335` + store 返回新对象 | 200 账号批量 ≈ 800 次全量重渲染 | **P1** |
| 2 | 同步命令主线程阻塞 I/O | 19 个 sync command | 10,000 条导出 / 无超时 `pgrep` | **P1** |
| 3 | `julianday()` 破坏索引 → TEMP B-TREE | `storage/mod.rs:434,462,482,1022` | 每行一次排序 | **P1** |
| 4 | WAL 无 checkpoint → 无界增长 | 仅 `:741` 一处 | 实测 96 KB 库 / 2 MB WAL | **P1** |
| 5 | `get_action_log_stats` 全表 `SUM` | `logs.rs:106-112` | 每次日志刷新全扫 | P2 |
| 6 | `prioritize_active_trae_account` N+1 | `commands/accounts.rs:568-579` | ≤200 次串行查询 | P2 |
| 7 | 日志列表无虚拟化 + 全选 O(n²) | `LogsView.tsx:419-570`、`Dashboard.tsx:283-285` | 2000 行 / 4×10⁶ 次比较 | P2 |
| 8 | `save_quota_snapshot` 每次插入后跑 `NOT IN` 清理 | `storage/mod.rs:1016-1026` | 每写一次全扫 | P2 |
| 9 | 单连接 `Mutex<Connection>` 全局串行 | `storage/mod.rs:32` | 所有 DB 操作串行 | P2 |
| 10 | 6 个独立 `reqwest::Client` 连接池 | `update.rs:55,63,73` + `common.rs:10` | 连接复用率低 | P2 |

---

## 11. 待确认问题

以下问题**需要项目所有者确认**，会实质影响优化优先级排序：

| # | 问题 | 为什么重要 |
| --- | --- | --- |
| Q1 | `qwitch/` 目录是什么？IDE 误操作产物 / 有意快照 / 临时备份？可以删除吗？ | 影响 P2-1/P2-2 处理方式；若误提交进 CI 会双跑测试 |
| Q2 | 是否会真正发布 macOS 签名版本？Apple Developer 证书是否可获取？ | 决定 P0-3（钥匙串反复授权 + 升级拒启）的修复路径：签名是根治，代码兜底是缓解 |
| Q3 | 是否存在真实用户在使用外部插件（`data_dir/adapters/` 当前为空）？ | 决定 P0-2 是「立即修」还是「随 v0.4 修」；无用户则风险窗口小 |
| Q4 | 四个工具的账号规模上限是多少？（当前批量上限 200） | 决定 P1-1/P1-4/P1-5 的性能修复紧迫度；若人均 <10 账号，可降级为 P2 |
| Q5 | Qoder macOS 本地导入是「计划实现」还是「已知不支持」？ | 决定 P1-11 是补实现还是改 UI 文案诚实降级 |
| Q6 | 是否有意支持 Linux？`keyring` 无 Linux backend → 必然降级明文密钥 | 若不做 Linux，应在 `README` 明确声明并移除半残分支 |
| Q7 | 上游平台（Trae/Qoder 等）的 ToS 对「多账号切换」的容忍度？ | 影响产品边界与「不篡改设备指纹」承诺的执行力度 |
| Q8 | `x-app-version: 3.4.22` 这类硬编码版本号的更新机制是什么？ | 客户端升级后若服务端校验版本区间，会整体 403 并被误判为「风控」 |

---

## 12. 风险点汇总（按「影响 × 可能性」排序）

| 风险 | 影响 | 可能性 | 等级 |
| --- | --- | --- | --- |
| 前端 XSS → RCE → 凭据全量泄露（P0-1 + P0-2 叠加） | 极高 | 低（需先有 XSS 或供应链污染） | **极高** |
| 钥匙串丢失 / `key_id` 损坏 → 凭据永久不可恢复（P0-3） | 极高 | 中（重装系统、备份恢复、断电） | **高** |
| 旧格式密文跨账号移植（P0-4） | 高 | 低（需本地攻击者） | 中 |
| 安全迁移静默失败（P0-5） | 高 | 中 | **高** |
| 上游接口变更 → 全工具静默失效（P1-10 硬编码） | 高 | **高**（服务端随时可能改） | **高** |
| 更新降级攻击（P1-7） | 高 | 低 | 中 |
| 大账号量 UI 卡顿（P1-1/P1-4） | 中 | 高（取决于 Q4） | 中 |
| Mock 数据泄漏到生产（P1-2） | 中（信任损伤） | 低 | 中 |
| 文档漂移导致二次开发误判（P2-3） | 中 | **已发生** | 中 |
| 测试双跑掩盖真实覆盖率（P2-1） | 低 | 已发生 | 低 |

---

## 13. 后续优化优先级建议

### 阶段 A：安全止血（建议本迭代内完成，1–2 天）

> 目标：把「一次前端污染 = 全量凭据泄露」的链路切断。

| 序 | 任务 | 对应 | 工作量 |
| :-: | --- | --- | :-: |
| A1 | `custom_client_paths` 改为只接受 `pick_file` 签发的一次性 token；`launch_client` 前复核 basename/目录白名单 | P0-1 | 中 |
| A2 | 补 `src-tauri/permissions/*.toml` + 收敛 `capabilities/default.json` 到最小集 | P0-1 | 中 |
| A3 | 插件信任改 Rust 侧原生确认（`begin/commit_trust_request`）；`is_valid_entry_command` 改 fail-closed | P0-2 | 中 |
| A4 | `key_id` 改原子写；空/非法视为缺失而非冲突 | P0-3 | 小 |
| A5 | 三个安全迁移失败改为**阻断启动 + 明确指引**（或只读模式） | P0-5 | 小 |
| A6 | `rm -rf qwitch/` + `.gitignore` 加 `qwitch/` + 补 vitest `include` 配置 | P2-1/2-2 | **极小** |

### 阶段 B：数据安全与契约收敛（1 周）

| 序 | 任务 | 对应 |
| :-: | --- | --- |
| B1 | 旧格式密文标记隔离 + 启动告警，消除 AAD 绕过 | P0-4 |
| B2 | 首次托管密钥后强制加密导出备份 / UI 明示不可恢复风险 | P0-3 |
| B3 | 更新加 anti-rollback（重拉 latest + 版本单调性 + `min_supported_version`） | P1-7 |
| B4 | PKCE verifier 出 query；OAuth 凭据改 `invoke` 桥 + nonce 一次性消费 | P1-8/9 |
| B5 | `map_business_code` 补齐平台私有码；5xx 加连续计数熔断；429 解析 `Retry-After` | P2-24/25 |
| B6 | WorkBuddy 运行时删「取第一个文件」兜底 + 签名校验 | P1-12 |

### 阶段 C：性能与状态层重构（1–2 周）

| 序 | 任务 | 对应 |
| :-: | --- | --- |
| C1 | Context 拆 actions/state 双 Context；store 返回值 `useMemo` 稳定化；`AccountCard` 改 props 驱动 | P1-1 |
| C2 | 19 个同步命令改 `async` / `spawn_blocking`；`pgrep`/`tasklist` 加超时；keyring 移出主线程 | P1-4 |
| C3 | `fetched_at` 改 `INTEGER epoch_ms` + 重建索引，消除 TEMP B-TREE | P1-5 |
| C4 | 周期性 `wal_checkpoint(PASSIVE)` + `journal_size_limit` + 退出 checkpoint | P1-6 |
| C5 | mock 改动态 import + DCE | P1-2 |
| C6 | `syncFromBackend` 加 `seq` 守卫；日志全选改 `Set`；日志列表虚拟化 | P2-16/17 |

### 阶段 D：工程护栏与文档收敛（持续）

| 序 | 任务 | 对应 |
| :-: | --- | --- |
| D1 | **补 `useAccountStore.test.ts`**（1,353 行零测试，含 4 个早退分支、互斥、事件注册/取消）+ `Modal.test.tsx` 焦点陷阱 | 测试缺口 |
| D2 | 引入 ESLint（`react-hooks/exhaustive-deps: error`）——立即暴露 P1-1 的漏 dep 并防复发 | P2-18 |
| D3 | `Cargo.toml` 加 `[dev-dependencies] wiremock`，为 21 处 HTTP 调用点建 7 态矩阵 | P2-14 |
| D4 | 文档一次性收敛：`ADAPTER_SPEC` 补 MiniMax、修正 `prepare_switch`→`apply_session`、统一测试数（86/67）、15s→45s | P2-3 |
| D5 | CI 加 `cargo audit` + `shellcheck`；actions 按 SHA 固定；`npm audit` 去 `--omit=dev` | P2-20 |
| D6 | 提取 `merge_post_claim_quota` 等 3 处字节级重复到 `common.rs`；统一 host 注入 | P2-12/13 |
| D7 | `storage/mod.rs` schema 演进改 `PRAGMA table_info` 判断，错误上抛 | P2-4 |
| D8 | 锁中毒恢复（`PoisonError::into_inner`） | P2-5 |

### 13.1 优先级判断依据

1. **安全 > 数据 > 性能 > 体验 > 工程卫生**：P0-1/P0-2 是唯一能把「本地应用」变成「远程可利用」的路径，且修复成本低（均为输入校验收紧），投入产出比最高。
2. **A6 是「5 分钟买回测试真实性」**：当前 133 例中有 66 例是重复目录的副本，会掩盖真实覆盖率，且让 CI 时间翻倍。
3. **C1 与 C3 是性能修复的杠杆点**：C1 解决 200 账号批量时的重渲染风暴；C3 解决每次列表查询的排序开销。二者都不需要改业务逻辑。
4. **D4 应尽早做**：文档已出现「ADAPTER_SPEC 完全不提第 4 个适配器」级别的漂移，会直接误导后续迭代（例如按文档实现 Qoder macOS 解密会得到错误结论）。

---

## 14. 附：本次分析的方法与局限

**方法**：
- 全量源码分层阅读（前端 10.8k 行 / 后端 20.7k 行）
- 四项自动化门禁实测：`npm run build`、`npm test`、`cargo test`、`cargo fmt` + `cargo clippy` + `npm audit`
- 数据库查询计划验证：`EXPLAIN QUERY PLAN` 确认 `julianday` 索引失配
- 构建产物核验：在 `dist/assets/*.js` 中检索 mock 字符串确认泄漏
- 交叉验证：`tauri-macros` 源码确认 sync command 主线程语义；`git status` 确认重复目录未跟踪

**局限**：
- **未在 Windows 实机运行**：`cargo check --target x86_64-pc-windows-msvc` 在 `ring` 的 C 编译阶段因缺 MSVC 头失败，故 Windows-only 分支（Qoder DPAPI、`creation_flags`、NSIS 安装）**无编译证据**
- **未做动态测试**：未对真实上游 API 发请求（避免消耗真实账号额度），适配器行为结论基于源码与既有测试
- **未审计依赖 CVE**：`cargo audit` 未安装，Cargo 侧 400+ crate 的已知漏洞未核查；npm 侧 `npm audit` 为 0
- **外部插件路径未实测**：`data_dir/adapters/` 为空，插件沙箱结论基于源码
- **Linux 未验证**：`keyring` 无 Linux backend 的判断基于 `Cargo.toml` features，未实机验证降级行为

---

*报告结束。本报告为只读分析产物，未修改任何业务代码。*
