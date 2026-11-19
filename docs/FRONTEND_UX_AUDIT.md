# Qwitch 前端体验与交互审计报告（最终版）

> **⚠️ 历史基线文档（2026-11-18 状态标注）**：本报告为修复前的问题基线（当时前端 12 个源文件约 4,337 行，现已扩展并新增测试）。P0-1~P0-3、F-1~F-4、P1/P2 所列问题已随 commit `7d45cd2`、`43de677`、`e09e391`、`3f8285c`、`1f0b10b` 等批次修复（托盘 Toast/进度、导入防重置、统计口径 selectors、Modal 焦点陷阱稳定化、设置草稿保护、保存失败不误报、核实并领取闭环、ErrorBoundary、托盘骨架、日志分页与确认范式）。**仍待处理**：Context 四拆与增量同步、日志虚拟列表、组件级集成测试、主题真正跟随系统（后端默认值待改 system）。请勿将本文问题表当作当前现状。

> 本文档基于对 Qwitch 前端 12 个源文件（约 4,337 行）、Rust 侧事件/窗口契约的逐行源码核验，以及 Vite dev server 双主题运行时 DOM/渲染实测汇总形成，作为前端架构演进与体验优化的权威基线。

---

## 一、审计范围、方法与结论速览

* **代码审计范围**：[src/](../src) 全部 12 个源文件（含 [App.tsx](../src/App.tsx)、[QwitchContext.tsx](../src/context/QwitchContext.tsx)、[Dashboard.tsx](../src/pages/Dashboard.tsx)、[TrayPopover.tsx](../src/pages/TrayPopover.tsx)、6 个组件、Mock 数据与类型定义）及 Rust 侧 IPC/窗口/适配器联动逻辑。
* **基线验证**：`tsc --noEmit` 通过，生产构建通过（JS 268.59 kB / gzip 73.90 kB，CSS 52.45 kB / gzip 8.49 kB）。
* **整体评价**：视觉骨架简洁克制，卡片层级与额度进度条表达清晰，符合 [架构文档](../docs/ARCHITECTURE.md) 第 6 章定位；但存在**四类结构性问题**（反馈断链、统计/状态口径不一致、响应式断点错位、可访问性及动态扩展能力缺口）。

| 类别 | 核心问题 | 影响面 |
|---|---|---|
| **反馈断链** | 托盘窗口无全局反馈通道且切号失焦即隐藏；导入弹窗被后台同步静默重置；批量查额吞错无进度 | 托盘操作结果不可见；用户输入丢失；异常被掩盖 |
| **口径与语义冲突** | 同屏统计口径打架；头部作用域错位；Qoder 签到后额度条反置为 `0%` 红条；就绪态与过期态矛盾 | 用户对数字失去信任，核心「额度水位」被反向误导 |
| **响应式与功能缺口** | 统计卡在 860–934px 破版；缺少重命名/停用开关；外部适配器 UI 不可见 | 最小窗口下排版断裂；后端已有能力前端不可达 |
| **可访问性与工程债** | 浅色 78 处/深色 56 处对比度低于 WCAG AA；模态/进度/焦点/播报全缺；Context 与 IPC 放大 | 键盘与读屏不可用；批量操作触发高频冗余全量同步 |

---

## 二、源码核验与技术细节修正

经逐行核对前后端源码，原《前端体验审计报告》中的 **P0-1 ~ P0-3、P1-1 ~ P1-13、P2-1 ~ P2-10** 核心问题全部属实，但有 **2 处技术细节** 在核验后作如下精确修正：

1. **修正 P0-1 中关于「后端吞掉批处理失败且不发事件」的描述**：
   * 核验 [scheduler/mod.rs](../src-tauri/src/scheduler/mod.rs#L52-L265) 可知，`run_daily_claim_batch` 在循环内部已捕获单账号 `AdapterError`、累加 `failed_count`、写入 `action_logs` 并在结束时发送 `phase: "done"` 的 `qwitch://batch-progress` 事件（[commands/mod.rs](../src-tauri/src/commands/mod.rs#L489-L500) 的 `let _ =` 仅包裹函数自身的 `Result`，而函数内仅可能提前返回空列表 `Ok(())`）。
   * **托盘之所以完全失联，真正根因在于前端**：
     1. 无可领账号时 [QwitchContext.tsx](../src/context/QwitchContext.tsx#L477-L482) 仅调用 `showToast`，而 [App.tsx](../src/App.tsx#L16-L26) 在托盘分支提前 `return`，未挂载 Toast；
     2. 正常触发批次时，[TrayPopover.tsx](../src/pages/TrayPopover.tsx) 未挂载 `<BatchClaimBanner />`，也未对 `batchProgress.summaryText` 触发提示；
     3. 切号时 [lib.rs](../src-tauri/src/lib.rs#L124-L126) 规定托盘 `Focused(false)` 立即隐藏，目标客户端重启抢走焦点导致托盘直接消失。
2. **修正 P1-13 中关于「导出也仅限 200 条」的描述**：
   * 前端 [QwitchContext.tsx](../src/context/QwitchContext.tsx#L258) 确实硬编码 `limit: 200, offset: 0` 且无分页加载更多；
   * 但在 Tauri 真实运行时，导出调用后端 `export_action_logs`（[commands/mod.rs](../src-tauri/src/commands/mod.rs#L579)），上限为 `10,000` 条（仅浏览器 Mock 模式受限于前端内存）；
   * 此外还需注意：虽然后端 [commands/mod.rs](../src-tauri/src/commands/mod.rs#L506-L520) 已支持 `offset` 参数，但前端日志列表缺少分页机制且无截断提示。

---

## 三、源码深度考究新增补漏清单（F-1 ~ F-4）

在原报告之外，源码核验发现以下 **4 项高影响前端/展示体验缺陷**：

### F-1（P0 级业务展示缺陷）｜Qoder CN 签到后额度进度条反向变为 `0%` 红色告警
* **源码依据**：[qoder.rs](../src-tauri/src/adapter/qoder.rs#L708-L716) 将当期活动中已领福利 `claimed_credits` 写入 `used`，将待领福利 `claimable_credits` 写入 `remaining`；而 [AccountCard.tsx](../src/components/AccountCard.tsx#L145-L186) 与 [TrayPopover.tsx](../src/pages/TrayPopover.tsx#L56-L90) 统一按 `remaining / total` 渲染额度进度条，且 `<= 15%` 标为 `bg-rose-500` 红色告警。
* **后果**：用户尚未签到时显示 `100 / 100 Credits (100%)`；**一旦成功签到**，`claimable_credits` 降为 `0`，卡片进度条瞬间变成 **`0%` 红色危急状态**，严重误导用户以为「签到完反而清空了余额」。

### F-2（P1）｜「刷新全部额度」静默吞掉失败且无进度提示
* **源码依据**：[QwitchContext.tsx](../src/context/QwitchContext.tsx#L426-L441) 的 `refreshAllQuotas` 在 `for` 循环中依次 `invoke("refresh_account_quota")`，`catch {}` 直接吞掉所有异常；完成后无条件提示「已刷新所有已完成在线授权账号的额度」。
* **后果**：部分账号出现 `401`（凭据过期）或网络错误时用户误以为全部刷新成功；账号较多时按钮只转圈，无法感知当前刷新进度。

### F-3（P1）｜缺少账号重命名与手动停用/启用（`disabled`）交互入口
* **源码依据**：导入时留空别名会自动使用官方昵称或 UID，之后前端没有任何入口修改 `label`；后端已支持 `AccountStatus::Disabled` 及 `update_account_status` 命令（[commands/mod.rs](../src-tauri/src/commands/mod.rs#L462-L471)），但前端除 `recoverAccount` 外从未使用，卡片缺少「停用/恢复启用」开关。
* **额外关联发现**：[Dashboard.tsx](../src/pages/Dashboard.tsx#L642) 当 `claimableCount === 0` 时直接显示「已全签」，即便所有账号均未授权、未查额或工具不支持签到也会误报「已全签」。

### F-4（P2）｜外部扩展适配器在前端 UI 中静态硬编码导致不可见/不可导入
* **源码依据**：后端 [external.rs](../src-tauri/src/adapter/external.rs#L299-L334) 已完整支持外部插件发现，但前端 [Dashboard.tsx](../src/pages/Dashboard.tsx#L48-L52)（`TOOL_TABS`）、[TrayPopover.tsx](../src/pages/TrayPopover.tsx#L7-L11)（`TOOL_GROUPS`）、[ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx#L7-L11)（`TOOL_OPTIONS`）全部静态写死三款内置工具，新增外部适配器后无法在导入弹窗选择、也无法在侧边栏和托盘展示。

---

## 四、定稿问题全表

### 4.1 P0 — 阻断级（共 4 项）

| 编号 | 问题 | 核心源码定位 |
|---|---|---|
| **P0-1** | 托盘窗口无 Toast 与批次进度反馈，且切号触发 `Focused(false)` 自动隐藏导致反馈不可见 | [App.tsx](../src/App.tsx#L16-L26)、[TrayPopover.tsx](../src/pages/TrayPopover.tsx)、[lib.rs](../src-tauri/src/lib.rs#L124-L126) |
| **P0-2** | 导入弹窗 `useEffect` 依赖 `adapters`，后台 `syncFromBackend` 触发清空用户输入的别名与选项 | [ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx#L32-L66)、[QwitchContext.tsx](../src/context/QwitchContext.tsx#L280) |
| **P0-3** | 统计卡 `md:grid-cols-4` 在 860–934px 窗口宽度下因侧栏挤压导致卡片破版截断 | [Dashboard.tsx](../src/pages/Dashboard.tsx#L571)、[tauri.conf.json](../src-tauri/tauri.conf.json#L19) |
| **F-1** | Qoder CN 签到后把待领额度当可用余额渲染，进度条反向变为 `0%` 红色告警 | [qoder.rs](../src-tauri/src/adapter/qoder.rs#L708-L716)、[AccountCard.tsx](../src/components/AccountCard.tsx#L145-L186) |

### 4.2 P1 — 体验与数据可信度级（共 16 项）

| 编号 | 问题 | 核心源码定位 |
|---|---|---|
| **P1-1** | 同屏统计口径打架（统计卡用全局 `accounts`，筛选胶囊与副标题用 `scopeAccounts`） | [Dashboard.tsx](../src/pages/Dashboard.tsx#L275-L330) |
| **P1-2** | 头部动作（「刷新全部额度」「手动一键领取」）作用域与工具/日志/设置页不匹配 | [Dashboard.tsx](../src/pages/Dashboard.tsx#L517-L564) |
| **P1-3** | 无首屏加载状态，Tauri 异步拉取完成前闪现「尚未导入任何 AI 工具账号」空状态 | [QwitchContext.tsx](../src/context/QwitchContext.tsx#L118-L120)、[Dashboard.tsx](../src/pages/Dashboard.tsx#L769) |
| **P1-4** | 统计口径不闭合（`activeCount` 与 `attentionCount` 重叠、`disabled` 遗漏、无待领时误报「已全签」） | [Dashboard.tsx](../src/pages/Dashboard.tsx#L289-L310) |
| **P1-5** | 额度百分比仅显示 `63%` 未标明「剩余」或「已用」，存在语义歧义 | [AccountCard.tsx](../src/components/AccountCard.tsx#L172-L175) |
| **P1-6** | 卡片底部就绪态仅看 `hasApiToken`，过期/风控账号仍显示绿色「查额与签到就绪」 | [AccountCard.tsx](../src/components/AccountCard.tsx#L214-L225) |
| **P1-7** | 破坏性操作两套范式（删账号 3s 再点一次无确认弹窗；删日志用确认条） | [AccountCard.tsx](../src/components/AccountCard.tsx#L42-L48)、[Dashboard.tsx](../src/pages/Dashboard.tsx#L1094-L1135) |
| **P1-8** | Toast 无语义分级（成功/失败同样式）、无 `role="status"`、且 `setTimeout` 未清理存在竞态 | [QwitchContext.tsx](../src/context/QwitchContext.tsx#L147-L152)、[App.tsx](../src/App.tsx#L84-L88) |
| **P1-9** | 对比度不达标（浅色 78 处 / 深色 56 处低于 WCAG AA 4.5:1），`text-zinc-400` 与 `10px/11px` 泛滥 | 全前端 JSX 样式 |
| **P1-10** | 模态/进度条无障碍全缺（无 `role="dialog"`、`aria-modal`、Esc 关闭、焦点锁定、`role="progressbar"`） | [ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx)、[ColdSwitchModal.tsx](../src/components/ColdSwitchModal.tsx) |
| **P1-11** | 10 个纯图标按钮缺 `aria-label`；品牌图标 `alt` 与文本重复导致读屏播报两次工具名 | [ToolBrandIcon.tsx](../src/components/ToolBrandIcon.tsx#L111) |
| **P1-12** | 单卡「领取」按钮在批量队列运行期间未禁用，重复点击触发第二批覆盖 `batchProgress` | [AccountCard.tsx](../src/components/AccountCard.tsx#L268-L286) |
| **P1-13** | 前端日志列表硬编码 `limit: 200, offset: 0`，超过 200 条无法翻页/加载更多且无截断提示 | [QwitchContext.tsx](../src/context/QwitchContext.tsx#L258) |
| **F-2** | 「刷新全部额度」静默吞掉单账号异常且缺少实时刷新进度 | [QwitchContext.tsx](../src/context/QwitchContext.tsx#L426-L441) |
| **F-3** | 缺少账号重命名（修改 `label`）与手动停用/恢复（`disabled`）交互入口 | [AccountCard.tsx](../src/components/AccountCard.tsx)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L462) |

### 4.3 P2 — 工程与一致性债务（共 11 项）

| 编号 | 问题 | 核心源码定位 |
|---|---|---|
| **P2-1** | 工具元数据在 4 个文件重复硬编码且文案不一致 | `Dashboard.tsx:48`、`TrayPopover.tsx:7`、`ImportAccountModal.tsx:7`、`ToolBrandIcon.tsx:13` |
| **P2-2** | 无基础 UI 组件库，[Dashboard.tsx](../src/pages/Dashboard.tsx) 1677 行内联大量重复变体，圆角 5 档混用 | [Dashboard.tsx](../src/pages/Dashboard.tsx) |
| **P2-3** | 无 `@theme` 设计令牌，[index.css](../src/index.css) 仅 8 行 | [index.css](../src/index.css) |
| **P2-4** | 主题不跟随系统 `prefers-color-scheme`，初始固定 `"light"` 导致深色模式首帧闪白 | [QwitchContext.tsx](../src/context/QwitchContext.tsx#L134) |
| **P2-5** | 死代码与断路能力（`token_paste` 后端与 Context 已实现但模态无入口；`ImportAccountModal` 的 `targetAccount` 无调用方） | [types/qwitch.ts](../src/types/qwitch.ts#L4)、[ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx#L22) |
| **P2-6** | `QwitchContext` Provider `value` 未 `useMemo`，每次 Toast/进度刷新都引发全树重渲染 | [QwitchContext.tsx](../src/context/QwitchContext.tsx#L1048-L1088) |
| **P2-7** | IPC 与重渲染放大（每次 `account-updated` 拉取 5 次接口 + `adapters` 换引用重跑 3 次进程探测；挂载时日志双拉；`listen().then()` 卸载竞态） | [QwitchContext.tsx](../src/context/QwitchContext.tsx#L203-L372)、[Dashboard.tsx](../src/pages/Dashboard.tsx#L144-L146) |
| **P2-8** | 浏览器托盘预览高度 `480px` 与真实 Tauri 托盘 `420px` 不一致，掩盖真实高度裁切 | [App.tsx](../src/App.tsx#L78)、[tauri.conf.json](../src-tauri/tauri.conf.json#L29) |
| **P2-9** | 主窗口关闭即隐藏到托盘，无显式「退出 Qwitch」入口与关闭提示 | [lib.rs](../src-tauri/src/lib.rs#L120-L122) |
| **P2-10** | 全项目 0 处 `focus-visible` 焦点环，输入框 `outline-none` 键盘焦点极弱 | 全前端样式 |
| **F-4** | 外部扩展适配器在前端侧边栏、托盘与导入弹窗中不可见 | [Dashboard.tsx](../src/pages/Dashboard.tsx)、[TrayPopover.tsx](../src/pages/TrayPopover.tsx)、[ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx) |

---

## 五、前端专项重构与设计规范

### 5.1 目标目录结构

```text
src/
├─ app/
│  ├─ AppShell.tsx          # 双入口分发（Dashboard / Tray）、Toast 宿主、ErrorBoundary
│  └─ routes.ts             # 导航视图与作用域定义（all | tool:<id> | logs | settings）
├─ store/                   # 按更新频次拆分的 4 个独立 Context（均做 useMemo 隔离）
│  ├─ AccountStore.tsx      # accounts / adapters / clientStatuses / initialized
│  ├─ LogStore.tsx          # logs / logScope / logStats / hasMoreLogs
│  ├─ SettingsStore.tsx     # theme / jitter / retention / customClientPaths
│  └─ FeedbackStore.tsx     # toasts（分级队列） / batchProgress / refreshProgress / switchDialog
├─ domain/
│  ├─ selectors.ts          # scopeAccounts / scopedStats / quotaHealth —— 口径唯一出口
│  └─ toolRegistry.ts       # 工具元数据唯一注册表（支持合并后端动态 adapters）
├─ components/ui/           # Button / Card / Badge / Progress / Modal / ConfirmDialog / Toast / Skeleton
└─ features/                # accounts/ | logs/ | settings/ | tray/
```

### 5.2 关键设计规范
1. **单一口径 `selectors.ts`**：统计卡、筛选胶囊、头部副标题、托盘摘要全部消费同一组选择器函数；主视图与单工具视图严格使用各自作用域指标，健康态按「正常 / 需关注（过期或风控）/ 待完善（缺凭据或快照）/ 已停用」互斥闭合划分。
2. **`@theme` 设计令牌（[index.css](../src/index.css)）**：最小字号收口至 `11px`（淘汰 `10px`），次要文字至少达到 `zinc-500`（浅色白底 >= 4.6:1）与 `zinc-400`（深色底 >= 4.5:1）；语义色固定映射：`emerald`=活跃/可用、`amber`=过期/需注意、`rose`=风控/危险、`sky`=每日签到/额度、`indigo`=切号动作。
3. **无障碍基座**：所有弹窗统一复用 `<Modal>`（支持 `role="dialog"`、`aria-modal="true"`、`aria-labelledby`、Esc 关闭、打开聚焦、焦点陷阱与关闭后归还焦点）；进度条补 `role="progressbar"` 与 `aria-valuenow`；纯图标按钮补 `aria-label`；装饰性品牌图标设 `alt=""` 避免重复播报。

---

## 六、前端专项优化计划与任务安排（FE-Phase 1 ~ FE-Phase 4）

### FE-Phase 1｜止血修复（高优先阻塞项，约 2–3 天）

| 任务 ID | 映射问题 | 涉及文件 | 具体改动内容 | 验收标准 |
|---|---|---|---|---|
| **FE-1.1** | P0-1, P1-8 | `App.tsx`, `TrayPopover.tsx`, `QwitchContext.tsx`, `lib.rs` | 将 Toast 移出 `isTauriTrayWindow` 提前 return 分支；托盘顶部/底部补充精简批次进度条与分级 Toast（`success/error/warning/info`），错误态保留手动关闭；清除 `showToast` 定时器竞态；切号触发失焦隐藏前支持通知或保留状态 | 托盘点击领取/切号/导入失败均可见红色错误提示；批次执行完托盘可见结果 |
| **FE-1.2** | P0-2 | `ImportAccountModal.tsx` | 将原单 `useEffect` 拆分为「弹窗打开时初始化表单」与「`adapters` 变化时仅校正不可用方式」，移除背景刷新时的 `setLabel("")` 重置 | 弹窗打开并输入别名期间，后端推送 `account-updated` 不会清空输入或切走 Tab |
| **FE-1.3** | P0-3 | `Dashboard.tsx` | 统计卡网格由 `md:grid-cols-4` 调整为容器查询或 `grid-cols-2 xl:grid-cols-4`，保证单卡最小可用宽 >= 165px | 窗口拉窄至 `minWidth: 860px` 时四张统计卡文字与数值零截断、零换行错位 |
| **FE-1.4** | F-1 | `qoder.rs` / `AccountCard.tsx` | 修复 Qoder 额度快照语义（区分「活动周期福利进度」与「账户总余额」，或单独标记为「今日可领/已领」而非剩余余额进度），避免签到后出现 `0%` 红色告警 | Qoder 账号成功签到后卡片展示正向「今日已领 +100 Credits」绿色/中性态 |
| **FE-1.5** | P1-9, P1-11 | `Dashboard.tsx`, `AccountCard.tsx`, `ToolBrandIcon.tsx` 等 | 批量将浅色下 `text-zinc-400` 提升至 `text-zinc-500`、废弃 `10px` 最小字号改为 `11px/12px`；为 10 个纯图标按钮补充 `aria-label`；品牌图与文字并列时设 `decorative`（`alt=""`） | 浅色/深色对比度不达标节点接近清零；读屏器不再读出「按钮」或重复工具名 |

### FE-Phase 2｜口径统一与核心交互闭环（约 1 周）

| 任务 ID | 映射问题 | 涉及文件 | 具体改动内容 | 验收标准 |
|---|---|---|---|---|
| **FE-2.1** | P1-1, P1-4 | 新增 `src/domain/selectors.ts`, `Dashboard.tsx` | 抽取统一切片统计选择器：在特定工具页展示该工具作用域统计；关闭健康度重叠与遗漏；区分「已全签 / 暂无待领 / 含未授权」文案 | 任意视图下统计卡与下方筛选胶囊、副标题数字 100% 闭合对账 |
| **FE-2.2** | P1-2 | `Dashboard.tsx`, `QwitchContext.tsx` | 在 `logs`/`settings` 页隐藏「刷新全部额度」「手动一键领取」；在工具专页仅刷新/领取当前工具账号并明确按钮文案（如「刷新 Trae CN 额度」） | 单工具页点击头部按钮仅触发该工具下账号请求 |
| **FE-2.3** | P1-3, F-2 | `QwitchContext.tsx`, `Dashboard.tsx`, `BatchClaimBanner.tsx` | 增加 `isInitializing` 加载态渲染骨架屏替代首屏空状态闪现；配合后端批量查额接口或汇总成功/失败数展示真实反馈与进度 | 冷启动不再闪现「尚未导入」；批量查额有失败时弹出汇总警告 |
| **FE-2.4** | P1-5, P1-6, P1-12 | `AccountCard.tsx`, `TrayPopover.tsx` | 百分比胶囊明确标注「剩余 63%」；就绪态增加 `account.status === "active"` 前置判定；批量队列运行中禁用单卡「领取」按钮 | 过期账号不再显示绿色就绪；批量进行中单卡不可重复发起领取 |
| **FE-2.5** | P1-7, F-3 | `AccountCard.tsx`, 新增 `ConfirmDialog.tsx`, `commands/mod.rs` | 账号删除统一升级为模态二次确认（移除 3s 定时器按钮）；卡片增加「重命名备注」与「停用/启用（`disabled`）」操作菜单 | 删除账号/清空日志使用同一确认范式；支持改备注和停用账号 |

### FE-Phase 3｜设计系统、组件抽象与功能完善（约 1.5–2 周）

| 任务 ID | 映射问题 | 涉及文件 | 具体改动内容 | 验收标准 |
|---|---|---|---|---|
| **FE-3.1** | P2-2, P2-3, P2-10 | `src/index.css`, `src/components/ui/*` | 在 `index.css` 建立 `@theme` 令牌；抽取 `Button / Card / Badge / Progress / Modal / ConfirmDialog / Toast`；全面补齐 `focus-visible` 环 | 全项目圆角统一为 3 档；键盘 Tab 焦点全程清晰可见 |
| **FE-3.2** | P1-10 | `Modal.tsx`, `ImportAccountModal.tsx`, `ColdSwitchModal.tsx` | 基于 `Modal` 基座改造全部弹窗：支持 Esc 关闭、焦点捕获与释放、背景滚动锁定、`role="dialog"` 与 `progressbar` 语义 | 键盘仅用 Tab / Shift+Tab / Esc 可完整操作所有弹窗 |
| **FE-3.3** | P2-1, P2-5, F-4 | `src/domain/toolRegistry.ts`, `Dashboard.tsx`, `TrayPopover.tsx`, `ImportAccountModal.tsx` | 收敛 4 处工具注册表；结合后端 `adapters` 动态生成侧栏、托盘与导入选项；在导入弹窗开放「手动 Token / JSON 粘贴」入口或完善高级折叠区 | 接入外部适配器或启用国际版后无需修改 UI 硬编码即可导入与管理 |
| **FE-3.4** | P1-13, P2-4, P2-8, P2-9 | `index.html`, `LogStore`, `Dashboard.tsx`, `App.tsx` | 首帧内联脚本读取主题并适配 `prefers-color-scheme`；日志列表支持「加载更多」分页与总条数提示；托盘预览框统一为 `340×420`；侧栏底部增设「退出 Qwitch」按钮 | 深色系统首帧无白闪；超过 200 条日志可翻页；主窗口可直接退出应用 |

### FE-Phase 4｜状态分层拆分与 IPC/渲染性能优化（约 1–1.5 周）

| 任务 ID | 映射问题 | 涉及文件 | 具体改动内容 | 验收标准 |
|---|---|---|---|---|
| **FE-4.1** | P2-6 | `src/store/*`, `AccountCard.tsx` | 将 `QwitchContext` 拆分为 `AccountStore` / `LogStore` / `SettingsStore` / `FeedbackStore` 并 `useMemo` 导出；`AccountCard` 包裹 `React.memo` | Toast 或进度条跳秒不再触发账号卡片重渲染 |
| **FE-4.2** | P2-7 | `AccountStore.tsx`, `Dashboard.tsx` | 为 `account-updated` 绑定增量更新与 150ms 去抖合并窗口；`refreshClientStatuses` 仅依赖适配器 ID 签名而非数组引用；消除挂载期日志重复拉取；修正 `listen().then()` 异步卸载竞态 | 批量领取 10 个账号触发的 IPC 次数从 ~80 次降至 <= 12 次 |
| **FE-4.3** | P3（前端测试） | `package.json`, `src/**/*.test.ts(x)` | 引入 Vitest + Testing Library，覆盖 `selectors.ts`、`ImportAccountModal` 防重置、托盘反馈、删除确认流程与键盘无障碍测试 | `npm test` 通过核心交互与选择器单元测试 |
