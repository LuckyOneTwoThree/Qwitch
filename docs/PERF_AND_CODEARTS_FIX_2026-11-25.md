# Agent 页卡顿与码道模型目录修复报告（v0.6.1 基线）

> **问题来源**：用户反馈「Agent 接入里选择模型很卡、有明显延迟」与「码道显示的模型不对：没有 DeepSeek-V4-Pro 却显示，反而 V4.1-Flash 不显示」。
> **结论**：两个问题根因不同，均已修复。卡顿来自「同步命令在主线程跑全量快照 + 一次勾选触发两轮快照」；码道来自「没有实时探测能力 + 静态表是手写猜测」。

---

## 一、实测数据（本机真实数据库，排查用临时程序，已删除）

单次 `build_workspace_state`（修复前）：

| 阶段 | 耗时 |
| --- | --- |
| `list_pools`（内部构建一遍候选账号） | 14 ms |
| `list_upstream_candidates`（再构建一遍） | 14 ms |
| `inspect_all_agent_bindings` | **77 ms**（其中 6 次 `pgrep -f` 约 66 ms） |
| `list_routing_traces(200)` | 1 ms |
| JSON 序列化（204 KB） | 6.5 ms |
| **合计** | **113 ms** |

一次「勾选模型」实际触发：**6 次候选账号构建 + 12 次 pgrep + 2 × 204 KB 快照**。

| 修复后 | 耗时 |
| --- | --- |
| 候选账号只构建一次（`list_pools_with_candidates` 复用） | 14 ms |
| `inspect_all_agent_bindings`（1 次进程快照） | 13 ms |
| traces + 序列化 | 8 ms |
| **合计** | **36 ms（-68%）** |

单进程探测对比：7 次 `pgrep -f` ≈ 75.7 ms → 1 次 `pgrep -fl .` ≈ 12.1 ms。

---

## 二、P0 — 卡顿

| # | 根因 | 位置 | 修复 |
| --- | --- | --- | --- |
| 1 | 每个 Agent 各跑一次 `pgrep`，一次快照 7 个子进程 | `agent_wiring/mod.rs::is_agent_app_running` | 改为一次 `process_snapshot()`（`pgrep -fl .`，失败回退 `ps`），内存内逐 Agent 匹配；新增纯函数 `agent_running_in_snapshot` 与双平台单测 |
| 2 | 快照构建重复构建候选账号（含逐账号解密） | `commands/pools.rs::build_workspace_state` | 新增 `list_pools_with_candidates`，候选只构建一次 |
| 3 | 命令是同步 `fn`，Tauri 在主线程执行 → 直接冻结 UI | `get_pool_workspace_state`、`set_agent_tool_models_cmd`、`update_agent_tool_schedule_cmd` | 改为 `async fn`，落到运行时线程池 |
| 4 | 每次勾选都发写命令，连点线性叠加 | `usePoolPrototypeStore.tsx::setAgentToolModels` | 250 ms 合并窗口，同一 `(agent, tool)` 只下发最终列表一次；卸载前强制 flush |
| 5 | 命令返回快照后又由事件触发一次全量刷新（纯重复） | `usePoolPrototypeStore.tsx::scheduleRefresh` | 命令回填快照后 600 ms 内跳过事件刷新 |
| 6 | 6 秒轮询拉 204 KB 全量快照 | `usePoolPrototypeStore.tsx::refreshAgentRunning` | 新增轻量命令 `detect_agent_running_cmd`（只回 6 个 bool）；状态无变化时返回原引用，避免整页重渲染 |
| 7 | 跳过事件刷新的实现若按时间窗粗暴屏蔽，会吞掉外部变更 | `eventRefreshGuard.ts`（新增） | 改为「计数 + 时间上限」：只吞命令自身 emit 的那一次，外部事件照常刷新 |

**收益**：单次勾选从「主线程阻塞约 300 ms」变为「后台约 70 ms，窗口不冻」，连点不再线性叠加；停留页面时的 6 秒卡顿消失。

---

## 三、P1 — 码道模型目录

**根因链**

1. `storage/model_discovery.rs::try_probe_live_models` 只支持 workbuddy / qoder_cn / trae_cn / minimax_code_cn / custom_api_key，`codearts` 走 `other => Err("暂不支持实时模型目录探测")`；
2. 探测失败 → `fallback_models` → 静态兜底表；码道的静态表（Rust 与前端各一份）是手写的 4 个模型；
3. `account_discovered_models` 表实测 0 行 —— 整个应用从未成功落库过任何实时目录，所有工具展示的都是兜底表；
4. UI 不区分来源，用户无法判断「这是探测到的」还是「这是猜的」。

**该账号的真实模型**（取自 CodeArts 内核启动下发的配置 `multimodalModels`）：

```
deepseek-v4.1-flash-1017, glm-5.3-flash, deepseek-v4.1-flash, kimi-k3.1-pro, kimi-k3
```

与静态表（deepseek-v4-pro / glm-5.3 / glm-5.2 / openPangu-Coder-Pro）几乎零交集 —— 与用户观察完全一致。

**为什么不做自动探测**：本地内核 `127.0.0.1:<port>/cag/model` 需要 IDE 私有 `auth_token`（实测 401），端口由 IDE 启动参数给出；云端 inferhub 的模型路径没有出现在客户端任何明文里，4 个候选路径均 404。因此本轮不做猜测式实现。

**修复**

| # | 改动 |
| --- | --- |
| 1 | 删除码道静态表中的 4 个编造模型（Rust `default_model_options_for_tool` 与前端 `UPSTREAM_MODEL_OPTIONS`），改为空目录 |
| 2 | 新增「手工维护模型目录」：`set_account_manual_models_cmd` → `account_discovered_models`，`source = "manual"`，不受 12 h TTL 约束；实时探测成功时覆盖它（实时优先）。前端入口在「模型配置」区右上角，弹窗按账号填写（每行一个模型 ID） |
| 3 | 来源徽章如实标注：`实时接口探测 / 部分实时 / 手工维护 / 未探测（静态兜底）`；单个模型行也带 `手工` / `未探测` 标签 |
| 4 | 目录为空时给出引导式空态（说明该工具无探测接口 + 一键打开手工维护），不再只显示「无匹配模型」 |

**影响面**：该码道账号为 `client_readonly`（无本机快照、未入池），本次改动只影响展示与可勾选列表；一旦后续签发独立 OAuth 让它参与调度，手工目录会直接作为可路由模型，避免拿错模型被上游 400。

---

## 四、契约与工程变更

| 项 | 变更 |
| --- | --- |
| 新增命令 | `detect_agent_running_cmd`（`Vec<AgentRunningState>`）、`set_account_manual_models_cmd`（`PoolWorkspaceState`） |
| 三处同步 | `lib.rs` 注册、`build.rs::APP_COMMANDS`、`capabilities/default.json`（权限 toml 由 tauri-build 生成，目录已 gitignore） |
| 命令转 async | `get_pool_workspace_state`、`set_agent_tool_models_cmd`、`update_agent_tool_schedule_cmd` |
| 存储新增 | `list_pools_with_candidates`、`save_manual_account_models`、`clear_account_models` |

---

## 五、回归用例

**Rust（+2，全量 236 通过）**
- `process_snapshot_matches_only_running_agents_on_macos` / `..._on_windows`：单次快照后逐 Agent 判定仍准确，空快照一律判未运行
- `manual_models_persist_without_ttl_and_live_overrides_them`：手工目录去空行去空格、不随 TTL 过期、实时覆盖、空列表清空

**前端（+9，全量 178 / 29 files 通过）**
- `usePoolPrototypeStore.modelWrite.test.tsx`：250 ms 内连勾 3 次只发 1 次写命令且携带最终列表；运行状态轮询不再拉全量快照；手工目录按账号落库并去空行
- `ManualModelModal.test.tsx`：回填、按行解析保存、清空提交空数组、未探测来源标记、空账号空态
- `eventRefreshGuard.test.ts`：只吞自己那一次事件、超窗自动作废、连续两次写命令各吞一次

---

## 六、门禁

```
cargo fmt --check                          ✅
cargo clippy --all-targets -- -D warnings  ✅
cargo test --locked --all-targets          ✅ 236 passed
npx tsc --noEmit                           ✅
npm run lint --max-warnings=0              ✅ 0 warnings
npm test                                   ✅ 178 passed / 29 files
npm run build                              ✅ 388 KB（gzip 103 KB）
```

---

## 七、未处理（后续）

- 码道 / 有道龙虾 / 豆包工作的**自动**模型探测仍缺协议依据；在有真实抓包前，手工维护是唯一不撒谎的方案。
- `set_pool_tool_models` 仍会把该工具**所有成员**重写为同一模型集（原 P2-10）；本轮未动，属语义设计取舍。
- 其余低频变更命令（挂载 / 解绑 / 删池）仍是同步命令，单次冻结约 100 ms；若后续继续收敛，可按本报告方式统一转 async。
