# Qwitch 综合优化计划与任务排期总表

> **完成度状态（2026-11-18）**：Sprint 1（止血与安全红线）与 Sprint 2（口径收口/切号可靠性/调度闭环）已随 commit `7d45cd2`、`43de677` 及后续修复批次落地；Sprint 3 大部分落地（设计令牌/无障碍/toolRegistry/退出按钮/外部适配器信任门禁/CSP 基础加固），`uid_hash` HMAC 升级仍为待评估项；Sprint 4 部分落地（WAL/单 SQL/快照清理/CI 测试门禁/Vitest 基线），Context 四拆与 `get_account_summary` 增量同步仍待排期。详见各批次 commit 说明。

> 本路线图拉通 [前端体验与交互审计报告](../docs/FRONTEND_UX_AUDIT.md) 与 [代码安全与架构审查报告](../docs/BACKEND_ARCH_REVIEW.md)，将前端与后端的协同改造编排为 **4 个可独立交付的 Sprint**，优先消除数据丢失与安全红线风险，再推进体验闭环、设计系统与底层架构演进。

---

## 一、全局优化优先级策略

1. **红线先行（Sprint 1）**：先修会**静默丢失用户输入/跨窗口数据不一致/导致安全凭据外泄/引发风控误发请求**的 P0/P1 问题。
2. **契约协同（Sprint 2）**：解决需要前后端配合的可靠性与口径问题（防风控批量查额 `refresh_quota_batch`、切号原子性、`custom_client_paths` 穿透、统计口径收口与操作完整闭环）。
3. **体验与扩展打底（Sprint 3）**：落地设计系统令牌、无障碍规范、动态适配外部插件与账号管理功能补齐，同时收紧 WebView CSP。
4. **性能与工程长治久安（Sprint 4）**：按更新频次拆分前端 Context、消除 IPC 与 SQL N+1 放大、完善数据库治理与自动化回归测试基线。

---

## 二、分阶段协同 Sprint 排期表

### Sprint 1｜止血修复与安全红线加固（P0 / 后端 P1 核心，约 3–5 天）

> **阶段目标**：消除所有阻断级反馈断链、用户数据丢失与高危安全隐患。

| 序号 | 责任域 | 对应任务 ID | 核心改动摘要 | 跨端接口/协同说明 |
|---|---|---|---|---|
| 1 | 前端 | **FE-1.1** | 托盘窗口恢复 Toast 与精简批次进度条；Toast 分级（`success/error/warning/info`）且清除定时器竞态；切号触发失焦隐藏时保留系统通知或延迟隐藏 | 可配合后端切号/批处理事件展示结果 |
| 2 | 前端 | **FE-1.2** | 修复 `ImportAccountModal` 因 `adapters` 引用变化清空表单输入 | 独立前端修复，零接口变更 |
| 3 | 前端 | **FE-1.3** | 修复 860–934px 窗口宽度下统计卡 `md:grid-cols-4` 破版截断 | 独立前端修复 |
| 4 | 前后端协同 | **FE-1.4** | 修复 Qoder CN 签到后进度条反向变为 `0%` 红条误报（区分「今日活动可领/已领」与账户余额） | `qoder.rs` 返回专用 key 或前端 `AccountCard`/`TrayPopover` 按 `claim_status` 区分展示 |
| 5 | 前端 | **FE-1.5** | `zinc-400` 提亮至 `zinc-500`、消除 `10px` 极小字号、纯图标按钮补 `aria-label`、品牌图 `alt=""` | 独立前端修复 |
| 6 | 后端 | **BE-1.1** | WorkBuddy `endpoint` 强制 HTTPS 与官方域名白名单校验，阻断凭据外泄 | 拒绝非法端点并返回 `INVALID_CREDENTIAL` |
| 7 | 后端 | **BE-1.2** | 移除 Keychain 有效时的明文 `master.key` 镜像写入并安全清理历史明文密钥文件 | 独立后端存储安全升级 |
| 8 | 后端 | **BE-1.3** | Trae OAuth 回调校验一次性随机 `flow_nonce` 与精确路径，Token 改由受限桥接传递不再放 URL Query | 独立后端 OAuth 安全加固 |
| 9 | 后端 | **BE-1.4** | 签到状态与领取响应严格 Fail-Closed：仅明确 `Available` 才可领，禁用缺失字段默认成功/100 积分 | 后端 `ClaimStatus::Unknown` 需前端识别（显示「待核实」而非误报已全签） |
| 10 | 后端 | **BE-1.5** | `delete_account`、`update_account_status` 补充 `qwitch://account-updated` 广播与审计日志 | 确保主窗口与托盘账号列表实时同步 |

---

### Sprint 2｜口径收口、切号可靠性与调度闭环（约 1 周）

> **阶段目标**：前后端拉通防风控调度、自定义客户端路径与切号原子性，让统计数字与核心操作 100% 可信。

| 序号 | 责任域 | 对应任务 ID | 核心改动摘要 | 跨端接口/协同说明 |
|---|---|---|---|---|
| 1 | 后端 | **BE-2.1** | `custom_client_paths` 贯穿至 `WorkBuddyAdapter`（`sym-v1` 解密）及进程检测/退出逻辑 | 前端设置页保存路径后即刻对查额、签到、冷切换全生效 |
| 2 | 后端 | **BE-2.2** | 切号引入可执行文件前置检查、会话备份回滚、`state.vscdb` 严格错误检查、`launch_client` 成功后才提交活跃态 | 失败时向前端返回明确错误码，UI 不再假成功 |
| 3 | 后端 | **BE-2.3** | OAuth 轮询引入 `flow_id` + 取消令牌彻底解决重开串扰，并复用 HTTP Client | 独立后端重构 |
| 4 | 前后端协同 | **BE-2.4** + **FE-2.3** | 后端新增防风控 `refresh_quota_batch(accountIds)` 命令接入单队列串行+Jitter；前端增加首屏骨架屏并展示批量查额实时进度与部分失败汇总 | 新增 IPC Command `refresh_quota_batch`，复用 `qwitch://batch-progress`（扩展 `action: "claim" \| "refresh"`） |
| 5 | 前端 | **FE-2.1** | 新增 `src/domain/selectors.ts` 统一全局与单工具统计口径；修复健康度重叠及无待领误报「已全签」 | 独立前端重构 |
| 6 | 前端 | **FE-2.2** | 收敛头部操作按钮作用域（日志/设置页隐藏、工具页仅操作当前工具） | 独立前端重构 |
| 7 | 前端 | **FE-2.4** | 额度百分比标注「剩余 XX%」；卡片就绪态绑定 `status === "active"`；队列运行中禁用单卡「领取」按钮 | 独立前端修复 |
| 8 | 前后端协同 | **FE-2.5** + **BE-1.5** | 统一 `ConfirmDialog` 删除确认；卡片新增「修改备注（`rename_account`）」与「停用/启用（`update_account_status`）」 | 后端新增 `rename_account(accountId, label)` 命令 |

---

### Sprint 3｜设计系统、无障碍基座与生态可扩展性（约 1.5–2 周）

> **阶段目标**：建立可复用的前端组件库与令牌体系，补齐无障碍标准，打通外部扩展适配器前端入口与 WebView CSP。

| 序号 | 责任域 | 对应任务 ID | 核心改动摘要 | 跨端接口/协同说明 |
|---|---|---|---|---|
| 1 | 前端 | **FE-3.1** | 在 `index.css` 建立 `@theme` 设计令牌，抽取 `Button / Card / Badge / Progress / Modal / ConfirmDialog / Toast`，全面补齐 `focus-visible` | 独立前端组件化 |
| 2 | 前端 | **FE-3.2** | 全部模态框与进度条补齐 `role="dialog"`、Esc 关闭、焦点锁定/归还、`role="progressbar"` | 独立前端无障碍改造 |
| 3 | 前后端协同 | **FE-3.3** | 建立唯一 `toolRegistry.ts` 合并后端 `list_adapters()`，支持外部插件动态展示；补齐 `token_paste` 入口 | 与后端 `external.rs` manifest 能力声明完全打通 |
| 4 | 前端 | **FE-3.4** | 主题初始化跟随 `prefers-color-scheme` 与首帧防白闪；日志列表支持「加载更多」分页；对齐托盘预览尺寸 `340×420`；侧栏新增「退出 Qwitch」按钮 | 需后端新增 `quit_app` 命令供主窗口调用 |
| 5 | 后端 | **BE-3.3** | 外部适配器增加 `timeout_ms` 上限（60s）、stdout/stderr 读取字节限额与显式用户信任开关（`trusted_adapters`） | 前端「设置 -> 插件管理」展示外部插件信任开关 |
| 6 | 后端 | **BE-3.4** | `tauri.conf.json` 配置严格 `csp` 策略；评估并实施 `uid_hash` 加盐 `HMAC-SHA256` 升级 | 需验证不影响内置 `QwitchLogo` SVG、本地图标与 Tauri IPC |

---

### Sprint 4｜状态分层、存储性能与全链路测试基线（约 1.5 周）

> **阶段目标**：消除前端重渲染与后端 N+1 查询瓶颈，完成代码模块化拆分与前后端自动化回归测试覆盖。

| 序号 | 责任域 | 对应任务 ID | 核心改动摘要 | 跨端接口/协同说明 |
|---|---|---|---|---|
| 1 | 前端 | **FE-4.1** | 将 `QwitchContext` 拆分为 `AccountStore` / `LogStore` / `SettingsStore` / `FeedbackStore`，`AccountCard` 包裹 `React.memo` | 独立前端架构重构 |
| 2 | 前端 | **FE-4.2** | 后端事件监听改为增量更新 + 150ms 去抖合并；修复 `refreshClientStatuses` 重复探测、挂载双拉与 `listen().then()` 卸载竞态 | 可选配合后端新增 `get_account_summary(accountId)` 做单行增量同步 |
| 3 | 后端 | **BE-3.1** | SQLite 开启 `WAL` + `busy_timeout`；`list_accounts` 单条 SQL 联合最新额度消除 N+1；`quota_snapshots` 单账号保留上限清理 | 接口返回值结构保持兼容 |
| 4 | 后端 | **BE-3.2** | `workbuddy.rs` / `qoder.rs` / `platform/mod.rs` 的子进程调用全面改为异步 `tokio::process::Command` 或 `spawn_blocking` | 独立后端异步化改造 |
| 5 | 后端 | **BE-4.1** | 拆分 `commands/mod.rs`（1225 行）为领域子模块；抽取 `adapter/common.rs` 消除三家适配器重复代码 | 独立后端重构 |
| 6 | 前后端协同 | **BE-4.2** + **FE-4.3** | 后端补齐 25+ 核心单元测试与 Mock HTTP 集成测试（`cargo test`）；前端引入 Vitest + Testing Library 并接入 `.github/workflows/ci.yml` | CI 双平台（macOS + Windows）同时运行前后端测试门禁 |

---

## 三、各阶段接口契约变更清单（前后端对齐速查）

| 变更阶段 | 后端新增/调整 Command 或 Event | 前端对应消费位置 |
|---|---|---|
| Sprint 1 | `qwitch://account-updated` 补充覆盖 `delete_account`、`update_account_status` | 托盘窗口与主窗口同步移除/更新账号卡片 |
| Sprint 1 | `QuotaSnapshot` 针对周期活动型额度清晰区分「可领福利」与「可用余额」 | `AccountCard.tsx`、`TrayPopover.tsx` 消除 `0%` 红色告警误导 |
| Sprint 2 | 新增 Command：`refresh_quota_batch(accountIds: string[])` | `Dashboard.tsx` 头部刷新按钮与 `BatchClaimBanner` |
| Sprint 2 | 新增 Command：`rename_account(accountId: string, label: string)` | `AccountCard.tsx` 重命名弹窗/行内编辑 |
| Sprint 3 | 新增 Command：`quit_app()` | `Dashboard.tsx` 侧栏底部退出按钮 |
| Sprint 3 | `AppSettingsDto` 新增外部适配器信任列表 `trustedAdapters: string[]` | `Dashboard.tsx` 设置页外部适配器启用开关 |
| Sprint 4 | 新增 Command：`get_account_summary(accountId: string)`（支持单条增量刷新） | `AccountStore` 收到 `qwitch://account-updated` 后定向刷新单卡 |
