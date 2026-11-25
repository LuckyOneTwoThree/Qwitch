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

---

## 八、第二轮补充（同日）：从「手工维护」升级为「自动读取 + 一键套用」

第一版把码道模型目录改成纯手工填写，实测反馈是「学习成本高、体验差」。本轮在不撒谎的前提下把交互成本降到最低。

### 8.1 码道：改为从本机客户端读取真实清单（无需任何操作）

码道没有可调用的模型接口，但 IDE 每次启动都会把完整配置 POST 给内核，日志里形如
`[init_post] config: {"multimodalModels":[...]}`。`storage/model_discovery.rs` 新增
`probe_codearts_client_models()`：按修改时间从新到旧扫描 `app-logs/logs/kernel/*.txt`，
取最后一次配置行，解析 `multimodalModels`（并兼容 `provider.models`），
来源标记为 **`client`**（UI 显示「本机客户端」）。

- 它是本机明文、只读、不含凭据的模型名列表；
- 允许落库（与 `live` 同级），受 12 小时 TTL 约束（客户端升级后自动刷新）；
- 代码：`codearts_models_from_kernel_log()` 为纯函数并有单测。

### 8.2 无探测能力的工具：内置「推荐目录」一键套用

新增 `recommended_model_options_for_tool()`（`domain/pools.rs`）与
`apply_account_preset_models_cmd`：来源标记 `preset`，UI 显示「内置预设」，
不进 TTL、不会被当作实时探测。码道预设取真实账号核验过的 5 个模型，
有道龙虾 / 豆包工作沿用既有目录但**不再冒充探测结果**。

入口：模型列表空态的首个按钮「套用推荐目录」，以及手工维护弹窗里每个账号的同一按钮。

### 8.3 「码道点了选不中」的根因与修复

根因（静默失败）：码道账号为 `client_readonly`（本机只读凭据）时——

1. 前端 `materializeDedicatedPoolForAgent` 明确跳过只读账号，它永远不会成为号池成员；
2. 后端 `set_pool_tool_models` 同样以 `code_arts_mode != ClientReadonly` 过滤；
3. 而勾选态由「号池成员的 supportedModels ∩ 已启用策略」推导。

于是每次点击：乐观更新让复选框短暂打勾 → 命令返回的快照里没有这条记录 → 立即跳回未勾选。
表现为「怎么点都选不中」。

修复：
- 新增 `domain/toolScope.ts::isToolScopeReadonly()`（带单测）；
- 该工具账号全部只读时，模型勾选与「设为默认」直接禁用，并在列表上方给出说明卡片，
  内嵌「签发独立 OAuth」按钮（沿用既有命令），不再让用户对着无反应的复选框猜。

### 8.4 关于「有道龙虾 / 豆包工作也显示未探测」

它们和之前的码道一样：`try_probe_live_models` 没有实现它们的协议，
只能回退静态表，而静态表在上一轮之后被如实标成「未探测」。
本轮给它们提供了一键套用入口；真正的自动探测仍需真实客户端抓包。

### 8.5 门禁与回归

```
cargo fmt --check / clippy -D warnings   ✅
cargo test                                ✅ 238 passed（+2：码道日志解析、预设目录）
npx tsc --noEmit / npm run lint           ✅
npm test                                  ✅ 185 passed / 30 files（+7）
npm run build                             ✅
```

---

## 九、同一码道账号出现两条：OAuth 导入没有稳定身份

### 9.1 现象与证据

用户先「内置授权导入」再「本机导入」同一个码道账号，账号列表出现两条。操作日志（脱敏后）直接暴露了两种 uid：

```
14:31:36 通过内置授权导入  code****81
14:31:56 从本机导入        c02b****f7
```

- 本机导入：`uid = session.userId` → `c02b6539a92f4027bc7b95945aba38f7`（与客户端 userInfo.id 一致）；
- 内置授权：STS 响应里没有 `principal_id` / `user_id` / `userId`，于是退化到
  `hash_fallback_uid("codearts", access_key)` → `codearts_<sha256前12位>`（掩码后 `code****81`）。

账号去重键是 `(tool_id, uid_hash)`，两者不同 → 必然新建一条。

更糟的是：STS 的 access key 是**一次性**的，同一个账号再授权一次又会得到新的兜底 uid，
所以即使不做本机导入，重复授权也会持续堆号。

### 9.2 修复

1. `exchange_authorization_code` 换完 token 后，若 uid 是兜底值，调用
   `verify_identity`（`/v5/caller-identity`）取稳定身份：
   优先 `user.id` → `principal_id` → `account_id`，并补 `display_name` / `domain_id`。
2. `is_fallback_uid` 增加 `codearts_` 前缀，避免兜底值被当成真实身份参与合并。
3. 合并边界：本机快照账号用 OAuth 重新授权时，`merge_from` 会保留快照形态而丢掉
   incoming 的 `codearts_mode=isolated_oauth`；现在 `upsert_account` 会把
   `isolated_oauth.json` 标记补进快照，账号才能从「只读」升级为可调度。

### 9.3 回归

- `authorization_import_uses_stable_identity_instead_of_ephemeral_access_key`
  （wiremock 模拟 STS + caller-identity，断言 uid 落在 IAM user id）
- `sts_response_without_identity_falls_back_to_ephemeral_uid`（记录历史行为，说明为何必须补 identity）
- `apply_identity_prefers_iam_user_id_and_keeps_existing_name`
- `codearts_oauth_reauth_marks_existing_local_snapshot_as_isolated`
  （同一账号不新增第二条 + 快照补回标记 + 模式升级为 isolated_oauth）

`cargo test` 全量 **242 通过**。

---

## 十、自动探测长期失效的真凶：ACL 漏登记（比模型问题严重得多）

### 10.1 现象

「本机导入码道后拿不到订阅模型」——界面直接是空目录。而在本机直接跑探测函数一切正常：

```
source=client count=5 error=None
deepseek-v4.1-flash-1017 | glm-5.3-flash | deepseek-v4.1-flash | kimi-k3.1-pro | kimi-k3
```

### 10.2 根因

`ensure_upstream_models_cmd`（导入后/启动时的自动探测）在 `lib.rs` 注册了，
但**没有**登记进 `build.rs::APP_COMMANDS`，也没有 `allow-ensure-upstream-models-cmd`。
本项目在 build.rs 里声明了 `AppManifest::commands`，Tauri 会对**所有**自定义命令强制 ACL 校验，
于是这个调用被直接拒绝；前端又把它 catch 成 `console.warn`，表现就是「静默不工作」。

影响面比码道大得多：这是**全工具自动探测**的统一入口。

### 10.3 修复后的实测（启动 12 秒后自动落库）

| 工具 | 来源 | 模型数 |
| --- | --- | --- |
| codearts | `client`（本机客户端） | 5 |
| lobsterai | `live`（实时接口） | 18 |
| workbuddy ×3 | `live` | 56 / 56 / 56 |

之前 `account_discovered_models` 长期是 0 行，根因就在这里。

### 10.4 顺带完成

1. **有道龙虾实时探测**：`GET {server}/api/models/available`（桌面端模型选择器同一接口，
   Bearer 鉴权），解析 `{code:0, data:[{modelId, displayName, accessible}]}`，
   跳过 `accessible == false` 与噪声变体；带单测。
2. **豆包工作**：客户端没有可用的模型接口（`public_config.json` 只有 text_picker），
   仍按预设处理。
3. **回退顺序**：本机 provider → **内置推荐目录（preset）** → 静态兜底（catalog）。
   这样没有探测能力的工具默认显示「内置预设」而不是刺眼的「未探测」。
4. **ACL 守护测试** `tests/command_acl.rs`：解析三份清单做集合比对，
   漏登记任意一处直接测试失败。这类坑已经踩过两次，必须钉死。

门禁：`cargo test` 243 + ACL 2，前端 185 / 30 files。

---

## 十一、码道为什么「勾选模型没反应」：它根本不在网关的上游列表里

前几轮一直把原因归到「只读凭据 / 需要 OAuth」，这个结论**不完整**。真实情况：

网关的 `resolve_upstream_auth`（`gateway/mod.rs:1310`）只认一种凭据形态——Bearer 令牌：
它调 `extract_bearer_and_base_url(cred)`，拿不到令牌就直接报「账号缺少有效 API Token」。
而码道的凭据里 `access_token` 恒为空（`adapter/codearts.rs` 三处构造都是 `String::new()`），
真实鉴权材料是华为云 AK/SK + STS，走的是 **SDK-HMAC-SHA256 签名**。

网关的上游地址映射也只有四个工具：

```rust
"minimax_code_cn" => "...", "workbuddy" => "...", "qoder_cn" => "...", "trae_cn" => "...",
_ => "https://api.openai.com",
```

没有 codearts 分支，也没有 AK/SK 签名实现。也就是说：即使账号被放进号池、即使完成独立 OAuth、
即使模型被勾选，请求也只会在 `resolve_upstream_auth` 阶段失败。

### 本轮修正

1. 新增 `domain/toolScope.ts::isToolGatewayRoutable()`：把「网关能调度哪些工具」显式化，
   码道返回 false，并附上原因（AK/SK 无 bearer 令牌）。
2. 模型区横幅改为如实说明：「{工具} 暂不支持网关调度，这里展示的模型目录仅用于和客户端对照，
   勾选不会生效」，删掉上一轮「去签独立 OAuth 就能用」的错误承诺。
3. 勾选 / 设为默认在该工具下直接禁用（`scopeNotSelectable = gatewayUnsupported || scopeReadonly`）。
4. README 同步修正。

### 想让码道真正可调度需要做什么（未实施）

内核二进制里能看到协议线索：`@ai-sdk/openai-compatible` + `@ai-sdk/inferhub-provider`，
请求 `POST {inferhubBaseUrl}chat/completions`（base 为
`https://snap-access.cn-north-4.myhuaweicloud.com/api/v2/`），鉴权头含
`x-auth-token` 与 `hws_access_key/hws_secret_key/hws_security_token`，另有 `maas_type` 头。

缺口有两个：① 现有凭据只存 AK/SK/STS，没存该接口需要的 `accessToken`；
② 需要在网关里新增一条 CodeArts 上游分支（含流式解析与错误映射）。
这是一个功能开发，不是修 bug，需要单独排期与真机验证。

门禁：前端 187 / 31 files（新增 gateway 可调度性测试）；Rust 246 + ACL 2。
