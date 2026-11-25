# Qwitch 模型目录探测链路修复报告（v0.6.1 基线）

> **问题来源**：外部深度排查报告《部分 Agent 订阅工具获取的模型不完整》。
> **核实结论**：报告列出的每一条结论均**属实**，本轮逐条复现并修复。
> **改动范围**：模型探测（`src-tauri/src/storage/model_discovery.rs`）、模型缓存（`src-tauri/src/storage/pools.rs`）、探测命令与前端契约（`src-tauri/src/commands/pools.rs`、`src/context/usePoolPrototypeStore.tsx`）、wire-id 解析（`src-tauri/src/domain/pools.rs`）、展示名合并（`src/data/mockPoolData.ts`、`src/pages/dashboard/AgentBindingsView.tsx`、`src/pages/Dashboard.tsx`）。
> **不改动**：号池调度策略、Agent 绑定协议、统计口径唯一出口（`src/domain/selectors.ts`）、品牌色系与整体信息架构。

---

## 一、报告结论的逐条核实

| # | 报告结论 | 核实 | 证据 |
|---|---|---|---|
| 1 | `is_noise_variant_id` 的 `ends_with("-max")` 误杀 `qwen4-max` / `qwen3.8-max` | ✅ 属实 | 静态目录（`domain/pools.rs`）明确收录这些模型，实时探测却把它们滤掉 → 实时结果比兜底表更差 |
| 2 | 探测超时仅 4s | ✅ 属实 | `DISCOVERY_TIMEOUT_SECS` 原为 4，冷链路中转站常超时 |
| 3 | 探测唯一触发点是前端手动「刷新真实模型」按钮 | ✅ 属实 | 全仓仅 `usePoolPrototypeStore.tsx` 一处调用 |
| 4 | 回退静态表被写入 `account_discovered_models` 并弹绿色成功 Toast | ✅ 属实 | `save_cached_account_models` 无来源校验；前端无条件 `notify(..., "success")` |
| 5 | 缓存读取无 TTL、不区分 `source` | ✅ 属实 | `load_cached_models_map` 直接反序列化，无时间/来源过滤 |
| 6 | WorkBuddy 探测只认 `session.json`，适配器写的是 `workbuddy-desktop.info` | ✅ 属实 | `adapter/workbuddy.rs` 写入键名与探测读取键名不一致 |
| 7 | Trae 写 `icube_auth_entries.json` / `trae_device.json`，版本写死 `2.0.15` | ✅ 属实 | `adapter/trae.rs` 的 `DEFAULT_TRAE_APP_VERSION = 3.4.22` |
| 8 | Qoder 机器码写 `auth.machine-id`，探测读 `machine_id` | ✅ 属实 | `adapter/qoder.rs` 键名不一致 |
| 9 | MiniMax token 在 `decrypted_token.txt` / `auth.json`；网关头含 `X-Mavis-*` | ✅ 属实 | `adapter/minimax.rs` 与 `gateway/mod.rs` 头构造 |

---

## 二、逐项修复（原因 → 改动 → 预期效果 → 验收要点）

### P0-1｜噪声过滤把正常主力模型当成噪声丢掉

**原因**　`is_noise_variant_id` 命中 `lower.ends_with("-max")`，把 `qwen4-max`、`qwen3.8-max`、`gpt-4-max` 等正常模型全部过滤。静态兜底目录里这些模型明确存在，于是**实时探测结果天然少于静态目录**——这正是「部分工具模型不完整」最直接的成因。

**改动**
- 删除 `-max` 噪声规则；保留 `auto` / `default` / `doubao-for-auto` / `*_advisor` / `subagent` / `sub_agent` / `search_agent` / `minimax-image` / `minimax-hailuo*` 等确属自动路由或非对话模型的条目。
- 显式补上裸 `auto`（此前只过滤 `-auto` 后缀，导致 Qoder 的 `auto` 条目漏进模型列表）。

**预期效果**　实时结果不再自损；`qwen4-max` / `qwen3.8-max` 等模型在 Qoder、Trae 的探测结果中保留。

**验收要点**
- Qoder CN 探测结果包含 `qwen4-max`（wire id `qmodel_latest`）与 `qwen3.8-max`（`qmodel_38max`）。
- 静态目录中存在的 `-max` 模型，在实时探测目录里同样存在。

---

### P0-2｜「探测失败」被固化成「探测成功」

**原因**　探测链路有三处把失败掩盖成成功：

1. `probe_models_for_account` 失败时回退到静态目录，但返回值不带任何来源标记；
2. `save_cached_account_models` 无条件接受这份兜底列表并落库；
3. 前端无论结果如何都弹「已完成全部上游账号真实模型目录探测」的绿色 Toast。

三者叠加的后果是：一次失败探测会把旧模型目录写进缓存并长期锁死，用户看到的却是「探测成功」。

**改动**
- 新增 `ProbeOutcome { models, source, error }`，`source ∈ {live, cache, catalog}`，并提供 `is_live()`。
- `try_probe_live_models` 由 `Option<Vec<_>>` 改为 `Result<Vec<_>, String>`，失败原因如实向上传递。
- `save_cached_account_models` **拒绝写入非 `live` 来源**（返回 `StorageError::Validation`），静态兜底与本机二级目录一律不入库。
- 缓存读取增加两道过滤：`source == "live"` 且未超过 `MODEL_CACHE_TTL_HOURS = 12`；历史误写入的静态条目自动失效。
- `refresh_upstream_models_cmd` 返回真实计数（探测数 / 实时成功 / 回退 / 失败）与逐账号明细（含失败原因），不再只回一个工作区快照。
- 前端按真实结果分档提示：全部实时成功 = 绿色成功；存在回退 = 黄色警告并说明回退来源；全部失败 = 红色错误并附具体原因。

**预期效果**　「探测成功」这三个字只在真的探测到上游目录时出现；模型目录不再被一次失败永久锁死；12 小时后自动重新对齐上游。

**验收要点**
- 断网后点击「刷新真实模型」→ 提示为警告/错误，文案含「回退」与失败原因，**不再出现**「已完成全部上游账号真实模型目录探测」。
- 断网刷新后，`account_discovered_models` 表不新增记录（回退结果不入库）。
- 恢复网络后再次刷新 → 提示「实时探测成功 N 个」，模型列表与上游一致。

---

### P0-3｜探测只在用户手动点击时发生

**原因**　探测链路没有任何自动触发点。用户导入账号、完成 OAuth 授权后看到的仍是旧目录，必须自己找到「刷新真实模型」按钮才知道目录已变。

**改动**
- 新增命令 `ensure_upstream_models_cmd`：**只探测缓存已过期或缺失的账号**（TTL 由存储层判定），已新鲜的直接跳过；返回探测/跳过/无凭据计数。
- 触发点三处：
  1. 应用启动（store 初始化）；
  2. `qwitch://account-updated` 事件（账号导入、OAuth 授权完成的统一出口）；
  3. 首次进入 Agent 调度页（该页是模型目录的主要消费场景）。
- 自动探测全程静默：失败只写控制台，不弹 Toast、不打断用户操作。

**预期效果**　用户不需要记住「刷新」按钮在哪；上游目录变更后进入应用即自动对齐；重复触发不会造成请求风暴（TTL 内直接跳过）。

**验收要点**
- 新导入一个账号 → 无需点击任何按钮，Agent 调度页的模型列表已包含该账号的真实模型。
- 应用启动时抓包/看日志可见 `ensure_upstream_models_cmd` 被调用一次；TTL 内再次进入不重复探测。
- 探测失败时不出现任何红色弹窗。

---

### P0-4｜各工具的探测参数与真实适配器不一致

**原因**　探测侧对各工具的凭据键名、版本号、鉴权头几乎全部是写死的估计值，与适配器实际写入的快照格式脱节，导致「本机导入的账号一律探测失败」。

**改动**（逐工具对齐适配器实测键名）

| 工具 | 原实现 | 修复后 |
|---|---|---|
| WorkBuddy | 只认 `session.json`；token 从 `auth.accessToken` 取（磁盘上是密文对象） | 读 `workbuddy-desktop.info`（回退 `session.json`）；token 取 `decrypted_token.txt`；补 `X-Enterprise-Id` / `X-Tenant-Id`；UA 升至 `WorkBuddy/5.9.3 WorkBuddy/5.9.3 CLI/2.165.0` |
| WorkBuddy 解析 | 命中 `cli` agent 后**只输出该 agent 的模型**，丢弃 `data.models` 其余条目 | 合并 `data.models` **全量**，`cli` agent 声明的模型排前但不丢弃其余 |
| Qoder CN | 只遍历写死的 `app` / `chat` 两个场景桶 | 遍历 `data` 下**所有**数组场景桶（chat / code / agent / quest / cli …）；场景按对话优先排序；机器码读 `auth.machine-id` |
| Trae CN | 认 `machine_id`；`ide_version` 写死 `2.0.15` | 读 `icube_auth_entries.json` 的明文 `token`；`ide_version` 与 `x-app-version` 统一 `3.4.22`；从 `trae_device.json` 补 `x-device-id` / `x-device-brand` / `x-device-type` / `x-os-version` |
| MiniMax | 要求 `provider.name` 为空或 `minimax`；只认 `data.providers[].config.models`；token 缺失时发匿名请求 | 接受任意 provider 名；同时支持 `provider.config.models` 与扁平 `data.models`；token 缺失时**不再发匿名请求**（匿名只会拿到缩减列表，反而制造「模型不完整」假象）；补 `X-Mavis-Agent-Id: main`、`X-Mavis-Timezone-Offset: 28800`；`auth.json` records 兜底取 token |
| 自定义 API Key | 固定拼 `/v1/models`；同时发 Bearer 与 `x-api-key` | `build_models_endpoint` 按 base URL 是否已含 `v1/v3/v4/v1beta` 决定是否补版本段；鉴权头按协议二选一（Anthropic 风格用 `x-api-key` + `anthropic-version`，否则 Bearer）；支持 `has_more` + `last_id` 分页（最多 10 页）；解析兼容 `data` / `models` / `items` / `list` / 根数组与 `id` / `model` / `name` 字段 |

**预期效果**　本机导入的账号也能真实探测；各工具拿到的模型集合不少于静态兜底目录。

**验收要点**
- WorkBuddy 本机导入账号 → 探测成功且模型数 ≥ 静态目录条数。
- Qoder CN 探测结果同时含对话与编码场景模型。
- Trae CN 探测请求头 `x-app-version: 3.4.22`，返回非空模型列表。
- 自定义网关 base 为 `https://x.com/v1` 时请求 `https://x.com/v1/models`（不出现 `/v1/v1/models`）。

---

### P1-1｜wire-id 解析只认静态映射表

**原因**　上游真实模型 ID 大小写敏感（如 `DeepSeek-V4-Pro`、`Kimi-K4-Preview`），而网关内部用归一化后的标准 ID 调度。静态映射表只覆盖已知模型，静态目录之外的实时模型一旦被归一化，还原出的 wire ID 就与上游不一致 —— 请求会被上游拒绝或静默降级。

**改动**
- domain 层新增进程内 `LIVE_WIRE_IDS` 注册表：实时探测成功即登记「标准 ID → 上游真实 ID」，只登记 `source == "live"` 的条目（静态兜底里的 wire ID 是猜测值，不得覆盖上游真实值）。
- `resolve_vendor_wire_model_id` 改为「静态表优先 → 未命中再查实时注册表 → 都没有才回落原值」，**不改动网关热路径签名**（该函数位于请求热路径，3 处调用点，回归风险高）。

**预期效果**　新上线 / 冷门的实时模型能正确还原成上游真实 ID；同时静态映射表的既有行为完全不变。

**验收要点**
- 静态表收录的模型 wire ID 与修复前逐一致（`qoder_cn/deepseek-v4-pro → dmodel`、`trae_cn/deepseek-v4-pro → DeepSeek-V4-Pro`）。
- 实时探测到的、静态表未收录的模型，wire ID 等于上游返回的原始 ID。

---

### P1-2｜展示名与成员模型列表仍以静态目录为准

**原因**　两处仍在用静态目录：
- Agent 调度页的模型展示名只查 `UPSTREAM_MODEL_OPTIONS`（静态兜底表），上游改名后仍显示旧名；
- 按候选账号新建号池成员时 `supportedModels` 写死 `[]`，该账号**明明已被探测到可用模型**，进入专属号池后却不贡献任何模型 —— Agent 侧看到的目录比实际更少。

**改动**
- 新增 `buildLiveModelLabels(candidates)`：汇总 `工具/模型ID → 展示名`，实时来源优先；`computeAgentExposedModels` 新增可选入参，展示名优先取实时值，无实时数据时回落静态目录。
- 候选账号派生的号池成员继承候选账号的真实模型列表（`modelOptions` 优先，回退 `defaultModels`），并按工具前缀作用域化。

**预期效果**　展示名与上游一致；新建号池不再丢失账号已探测到的模型。

**验收要点**
- Agent 调度页模型名与上游返回的 `name` 一致。
- 把一个已探测的账号加入号池后，该账号的模型出现在号池的模型策略中（此前为空）。

---

## 三、契约变更（前后端必须同步）

| 命令 | 变更前 | 变更后 |
|---|---|---|
| `refresh_upstream_models_cmd` | 返回 `PoolWorkspaceState` | 返回 `RefreshUpstreamModelsResult { workspace, probedCount, liveCount, fallbackCount, failedCount, items[] }` |
| `ensure_upstream_models_cmd` | —（新增） | 返回 `EnsureUpstreamModelsResult { probedCount, skippedCount, noCredentialCount, liveCount, fallbackCount, failedCount }` |

前端 `usePoolPrototypeStore.tsx` 已同步更新类型与调用方；`AgentBindingsView` 的「刷新真实模型」按钮走新的诚实提示。

---

## 四、回归用例

**Rust（新增 9 例，全量 231 例通过）**
- `noise_filter_keeps_legitimate_max_models`：`-max` 保留 / `auto`、`default`、`search_agent` 过滤
- `parses_workbuddy_qoder_minimax_and_openai_model_payloads`：WorkBuddy 全量合并、Qoder 多场景桶、MiniMax 扁平结构、OpenAI 多形态
- `openai_parser_accepts_multiple_payload_shapes`、`models_endpoint_respects_versioned_base_urls`
- `qoder_snapshot_reads_real_machine_id_key`、`workbuddy_snapshot_reads_real_auth_filename_and_enterprise_id`、`trae_snapshot_reads_auth_entries_token`
- `live_wire_registry_restores_probed_wire_id_for_unknown_models`、`live_wire_registry_ignores_catalog_entries_and_keeps_static_priority`
- `model_cache_accepts_only_live_and_expires_after_ttl`：非 live 拒写 + 13 小时后过期

**前端（新增 7 例，全量 169 例 / 26 files 通过）**
- `usePoolPrototypeStore.modelDiscovery.test.tsx`：全部成功 → 绿色；存在回退 → 黄色并说明来源；全部失败 → 红色；启动时自动触发一次 TTL 感知探测
- `mockPoolData.models.test.ts`：实时展示名优先、候选目录合并、静态回落

---

## 五、门禁

```
cargo fmt --check                                  ✅
cargo clippy --all-targets -- -D warnings          ✅
cargo test --locked --all-targets                  ✅ 231 passed
npx tsc --noEmit                                   ✅
npm run lint --max-warnings=0                      ✅ 0 warnings
npm test                                            ✅ 169 passed / 26 files
npm run build                                       ✅
```

---

## 六、本轮**未**处理（需真实抓包，列为 P2）

- `lobsterai` / `doubaowork` / `codearts` 的实时模型目录探测：需要真实客户端抓包确认协议后再实现；本轮仅保证它们的**静态兜底目录不再被噪声过滤误伤**。
- 上游模型下线后的缓存即时失效：目前依赖 12 小时 TTL 兜底，未做推送式失效。
