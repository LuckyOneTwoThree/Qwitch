# Qwitch 项目状态深度审查报告

- **审查基线**：分支 `main` @ `b1eaa21`（工作区另有 3 个未提交改动：`Dashboard.tsx`、`AgentBindingsView.tsx`、`AgentBindingsView.test.tsx`）
- **版本**：`package.json` / `Cargo.toml` / `tauri.conf.json` 均为 `0.5.1`
- **审查范围**：在上一轮审计（`docs/PROJECT_AUDIT_REPORT.md`，2026-11-20）之后新增的「多模型分级号池 + 本地多协议网关 + M:N Agent 挂载」大版本，及其对既有代码的回归影响
- **规模**：Rust ≈ 41.4k 行、前端 ≈ 20.7k 行；新增旗舰模块 `gateway/*` + `agent_wiring` + `storage/pools` + `usePoolPrototypeStore` + `PoolsView` + `AgentBindingsView` + `mockPoolData` 合计 ≈ 14.1k 行
- **审查方式**：全量静态走查 + 4 路并行深审（网关 / 号池与挂载 / 前端 / 安全与依赖）+ 自动化门禁实跑

---

## 1. 自动化门禁实测结果

| 门禁 | 命令 | 结果 | 说明 |
| --- | --- | --- | --- |
| 前端类型 + 构建 | `npm run build` | ✅ 通过 | 1615 模块；产出 `dist`；**单一 JS chunk 531 KB（gzip 146 KB）> 500 KB 告警** |
| 前端 Lint | `npm run lint`（`--max-warnings=0`） | ✅ 通过 | 0 告警 |
| 前端测试 | `npm test`（Vitest） | ✅ 24 文件 / **132 例全过** | — |
| 前端依赖审计 | `npm audit` | ✅ **0 漏洞** | 生产 + dev 均 0 |
| Rust 格式 | `cargo fmt --check` | ❌ **失败** | 9 个文件存在格式漂移：`agent_wiring`、`commands/pools`、`domain/pools`、`gateway/{ir,mod,scheduler}`、`storage/{crypto,model_discovery,pools}` |
| Rust 静态检查 | `cargo clippy --all-targets -- -D warnings` | ❌ **失败（8 error）** | 见下表 |
| Rust 测试 | `cargo test --locked --all-targets` | ✅ **185 例全过** | — |
| Rust 依赖审计 | `cargo audit` | ⚠️ 本地未安装 | CI `audit` job 安装；本轮无法离线复跑 |

**Clippy 8 处错误（均为 `-D warnings` 升级的默认 lint，非风格偏好）：**

| # | lint | 位置 |
| --- | --- | --- |
| 1 | too_many_arguments (8/7) | `agent_wiring/mod.rs:265` |
| 2 | too_many_arguments (11/7) | `agent_wiring/mod.rs:978` |
| 3 | too_many_arguments (8/7) | `agent_wiring/mod.rs:1291` |
| 4 | redundant reference in `format!` | `gateway/mod.rs:2098` |
| 5 | needless lifetimes `'a` | `gateway/mod.rs:2205` |
| 6 | redundant guard | `storage/model_discovery.rs:247` |
| 7 | type_complexity | `storage/pools.rs:687` |
| 8 | too_many_arguments (9/7) | `storage/pools.rs:1287` |

> **结论**：`main` 当前**无法通过 CI 的 `fmt` 与 `clippy` 两道门禁**（`ci.yml` 与 `release.yml` 的 build job 都会执行），即本次大版本是在门禁变红的情况下合入的；发布流水线会被同样阻断。

---

## 2. 九维健康度总览

| 维度 | 评级 | 核心现状 |
| --- | :---: | --- |
| **功能完整性** | 🔴 差 | 网关把工具定义（tools）整体丢弃，Agent 工具调用全链路失效；本轮唯一旗舰「号池管理」UI `PoolsView` 未接线成死代码；伪流式 + 20s 超时截断长生成 |
| **安全性** | 🟠 偏弱 | 本地网关**无鉴权**、绑定 token 为可猜静态常量，任意本机进程可白嫖号池额度乃至本机 Codex OAuth；模型探测 SSRF/token 外发；上一轮安全修复均未回归 |
| **稳定性 / 数据完整性** | 🟠 偏弱 | 多语句写库无事务（重复账号入池即产生半成品脏池）；Agent 配置解析失败时被空文档覆盖、破坏用户原配置；改端口/删池静默吞掉 wiring 失败 |
| **架构设计** | 🟠 偏弱 | 手写 HTTP/1.1 服务器替代设计文档声称的 axum；TS 与 Rust 双套仿真算法；`PoolWorkspaceState` 三处重复定义；多个 1.6k–2.7k 行上帝文件 |
| **测试覆盖** | 🟡 中 | 185 Rust / 132 前端全过，但网关 2775 行仅 12 个测试；`PoolsView.test` 覆盖的是生产不可达的孤儿组件（虚假覆盖信心）；事务/回滚/配置解析失败/鉴权无测试；无覆盖率度量 |
| **依赖配置** | 🟢 良好 | `npm audit` 0 漏洞；`reqwest` 单版本 0.12.28（较上轮改善，无 0.12+0.13 并存）；`rust-version=1.88` 已声明；`tokio` features 已收窄。仅 `base64/rand/sha2/ring` 存在多版本 |
| **构建部署** | 🔴 差 | `fmt`/`clippy` 双红，CI 与 Release 均被阻断；单 chunk > 500 KB 未拆包 |
| **性能** | 🟡 中 | 网关全量缓冲上游响应（内存无上限、TTFT 劣化）；`session_affinity` 无上限增长；前端并发命令全量快照回写导致 UI 覆盖闪烁 |
| **文档一致性** | 🟠 偏弱 | README 写「134 Rust / 122 Vitest 测试、36 命令」，实测 **185 / 132 / 70 命令**；设计文档标 v0.6.x 而代码 v0.5.1 |

---

## 3. P0 — 阻塞核心功能 / 阻断发布

### P0-1 网关从不向上游透传工具定义（`tools` / `tool_choice`），Agent 工具调用全线失效
- **位置**：`src-tauri/src/gateway/ir.rs:411-499`（`to_upstream_openai_chat_body`）、`src-tauri/src/gateway/ir.rs:500-577`（`to_upstream_anthropic_body`）
- **证据**：
  ```rust
  // ir.rs:486-497  上游 body 只拼 model/messages/stream/max_tokens/temperature
  let mut body = json!({ "model": upstream_model, "messages": msgs, "stream": stream });
  ```
  `ir.tools` 在三个 parser（`ir.rs:93/190/301`）里被解析进 `IrRequest.tools`，但两个上游构造器**从不写入 `tools` / `tool_choice`**；`call_upstream_member`（`gateway/mod.rs:1273-1295`）也不补。
- **影响范围**：dsh / opencode / codex / claude 全部依赖 function calling 读写文件、执行命令。上游收不到工具 schema → 模型永不产出 `tool_calls` → 编码 Agent 经网关路由时工作流整体瘫痪（响应侧虽能解析 `tool_calls`，上游根本不会给）。
- **根因**：IR 请求构造遗漏 `tools` 透传（parse 有、serialize 无）。
- **修复建议**：`body["tools"] = ir.tools`（并透传 `tool_choice`）；Anthropic 侧转为 `[{name, description, input_schema}]`。补充「带 tools 的请求→上游 body 含 tools」单测。
- **验证要点**：构造含 `tools` 的 `/v1/chat/completions` 请求打到 wiremock，断言上游收到 `tools[0].function.name`；端到端用 dsh 触发一次文件读取工具调用。

### P0-2 `main` 无法通过 CI（fmt + clippy 双红），发布链路被阻断
- **位置**：见第 1 节表格与 9 个 fmt 文件 / 8 处 clippy
- **影响范围**：`ci.yml`（push/PR 门禁）与 `release.yml`（macos-arm64、windows 两个 `run_tests=true` 的 build 矩阵都执行 `cargo fmt --check` + `cargo clippy -D warnings`）全部失败 → 无法出包/发布。
- **根因**：大版本合入前未跑本地门禁，`too_many_arguments`/`type_complexity` 等默认 lint 未处理，`cargo fmt` 未执行。
- **修复建议**：`cargo fmt` 全量格式化；对确实需要多参数的函数引入 `#[allow(clippy::too_many_arguments)]` 或重构为参数结构体；`type_complexity` 抽 `type` 别名；`redundant_guard`/`needless_lifetimes`/`redundant reference` 直接按提示改。
- **验证要点**：`cd src-tauri && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings` 退出码 0。

---

## 4. P1 — 高危（功能缺陷 / 凭据与数据风险）

### P1-1 本地网关无鉴权、绑定 token 为可猜静态常量，任意本机进程可白嫖号池额度与 Codex OAuth
- **位置**：`gateway/mod.rs:1710-1748`（仅把 bearer 当“身份提示”）、`gateway/mod.rs:1676-1687`（仅 Host/Origin 防护）、`agent_wiring/mod.rs:15-32`（`qwitch-bind-<agent>` 静态常量）、`storage/pools.rs:1580`（`binding_token_hash` 恒写空串）
- **证据**：
  ```rust
  let auth_token = headers.get("authorization")
      .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
      .or_else(|| headers.get("x-api-key").map(String::as_str)).unwrap_or("");
  // 之后只用于 agent_id_from_token 推断 agent，全程无「校验失败→401」分支
  ```
  全站没有任何一处对期望令牌做相等校验并拒绝；`qwitch-bind-*` 是可公开猜到的固定值。
- **影响范围**：任意同用户本地进程（`curl -H 'Host: 127.0.0.1:18425'`）即可无凭据调用 `/v1/chat/completions`，耗尽所有池化账号额度并读取模型回复。更严重：请求未知模型且带任意 bearer 时会进入 `resolve_codex_own_auth`（`gateway/mod.rs:658-677`），读取本机 `~/.codex/auth.json` 的 **ChatGPT OAuth access_token** 代理调用——把最高价值凭据开放给任意本机进程。
- **根因**：token 被当作“路由提示”而非“访问控制”；未落地设计文档承诺的「绑定专属 Token 强校验」。
- **修复建议**：启动时按 agent 生成**每装机随机密钥**，写入 Agent 配置并只在 `AppState` 保存其 hash；服务端对 `/v1/*`（含 `/v1/models`、`/v1/messages/count_tokens`）先鉴权，缺失/不匹配一律 `401`；禁止把客户端任意 bearer 当上游 key 透传；清理或真正启用 `binding_token_hash` 列。
- **验证要点**：无 token / 错误 token / 正确 token 三种请求分别断言 401/401/200。

### P1-2 伪流式 + 全局 20s 客户端超时，长生成被截断/整段缓冲
- **位置**：`gateway/mod.rs:1326-1328`（`resp.text().await` 读完整 body）、`adapter/common.rs:14`（`shared_http_client` 固定 `timeout(20s)`）、`gateway/mod.rs:2174-2187`（用完整响应伪造 SSE）
- **证据**：所有上游调用走 20s 超时的共享客户端，且 `to_upstream_*` 一律 `stream=false`；拿到整段 `text` 后再由 `to_*_sse` 合成 SSE。
- **影响范围**：①任何一次生成耗时 > 20s（长代码补全常态）→ reqwest 超时 → 候选失败 → 最终 `502 All pool candidates failed`；②TTFT = 全量生成时间，Agent 端长时间无输出；③超大响应整串驻留内存无上限。dsh 配置里写的 `streamIdleTimeoutMs: 600000` 与 20s 客户端超时自相矛盾。
- **根因**：以缓冲式调用冒充流式；共享客户端超时过短且无逐请求覆盖。
- **修复建议**：流式请求改用 `resp.bytes_stream()` 逐块解析并透传；为网关单独建 `read_timeout` 较长（或总超时按模型/请求可配）的 client；仅非流式走 `resp.text()`。
- **验证要点**：wiremock 上游慢速分块返回，断言客户端首字节在 <1s 到达且总时长可超 20s 不被截断。

### P1-3 Agent 配置文件解析失败时以空文档整体覆盖，破坏用户原有配置
- **位置**：`agent_wiring/mod.rs:807-821`（`write_json_deployment_mode_3p`）、`agent_wiring/mod.rs:1062-1064`（OpenCode 读入）
- **证据**：
  ```rust
  let mut doc: Value = std::fs::read_to_string(path).ok()
      .and_then(|s| serde_json::from_str(&s).ok())
      .unwrap_or_else(|| json!({}));      // 解析失败 → 空对象
  obj.insert("deploymentMode".into(), ...);
  write_private_file_atomic(path, ...);    // 写回只剩 deploymentMode
  ```
  OpenCode 侧 `serde_json::from_str(&strip_jsonc_comments(&existing)).unwrap_or_else(|_| json!({}))`，而 `strip_jsonc_comments` 不处理尾逗号/单引号/未加引号 key。
- **影响范围**：用户的 `claude_desktop_config.json`（含注释/尾逗号）、`opencode.jsonc`（尾逗号）一旦解析失败，原有 `mcpServers` / 其它 provider / agents 配置被清空，Claude Desktop / OpenCode 直接坏掉。虽有 `stash_original_files` 加密备份，但用户不解绑就看不到原文。
- **根因**：用「解析失败即空文档」做兜底并覆盖写回。
- **修复建议**：解析失败必须 `return Err` 中止写入并提示；或用宽松 JSONC 解析器；更稳妥采用 `# qwitch:managed-start/end` 标记块只替换块内内容、保留用户其余配置与注释。
- **验证要点**：对含尾逗号/注释的 JSONC 执行挂载，断言不覆盖原文件、返回明确错误、`stash` 可还原。

### P1-4 IR 结束原因与用量失真（截断被伪装成正常结束、token 计量错误）
- **位置**：`gateway/ir.rs:648-652`（`stop_reason` 仅由有无 tool_calls 决定，丢弃 `finish_reason`）、`gateway/mod.rs:1425-1435`（usage 缺失时硬编码 `100/64`）、`gateway/mod.rs:1589-1592`（Anthropic 流式忽略 `message_delta.usage.output_tokens`）、`ir.rs:825`（`message_start` 把 output_tokens 写死 0）
- **影响范围**：Anthropic 客户端永远收到 `end_turn`，无法感知 `length`/`max_tokens` 截断，会把“半截输出”当完整结果继续；计费/计量错误；跨协议 `stop_reason` 回传不归一。
- **根因**：缺少 `length↔max_tokens`、`stop↔end_turn`、`tool_calls↔tool_use` 的双向映射；usage 缺省值伪造而非省略。
- **修复建议**：建立结束原因双向映射表；`message_delta` 解析 `usage.output_tokens`；无 usage 时返回 0 或省略，不伪造。

### P1-5 号池管理主界面 `PoolsView` 未接线，旗舰「多池编排」在真机不可达（死代码 + 虚假测试覆盖）
- **位置**：`src/pages/Dashboard.tsx:1477-1484`（`nav==="pools"` 渲染的是 `AgentBindingsView`）、`src/pages/dashboard/PoolsView.tsx`（1361 行无任何生产 import）、`CreatePoolModal / AddPoolMembersModal / BindAgentModal`（仅被 `PoolsView` 引用）
- **证据**：全仓无 `import PoolsView`、无 `setNav("pools")`（侧栏只有 `agents/routing/logs/settings`）；`AgentBindingsView` 声明了 `onNavigatePools?` 但从不解构使用，Dashboard 传入的回调是空转。
- **影响范围**：号池的「新建/删除池、T1/T2/T3 梯队、同池多账号差异化模型、策略与亲和护栏、挂载/卸载 Diff」在真实应用里**完全不可达**；`PoolsView.test.tsx` 全部通过只能证明这段孤儿代码自洽，制造**虚假覆盖信心**。真机上唯一可用路径是 `AgentBindingsView` 的「一键接入推荐模型」→ `ensure_agent_dedicated_pool` 自动建“每 Agent 专属池”，无法做多池 M:N 编排。
- **根因**：路由 `pools` 与 `agents` 合并到同一视图，旧 `PoolsView` 与三个 Modal 未删，`onNavigatePools` 半接线。
- **修复建议**：产品确认走向——若“纯 Agent 中心”→ 删除 `PoolsView` 及三 Modal、清理 `"pools"` 分支与空转 prop；若需要多池编排 → 把 `PoolsView` 接回 `nav==="pools"` 并让 `AgentBindingsView` 真正调用 `onNavigatePools`。二者择一，避免死代码与误导性测试。
- **验证要点**：真机点击各侧栏入口可达号池 CRUD；删除死代码后 `PoolsView.test` 应同步移除或改为测试真实入口。

---

## 5. P2 — 中危（功能异常 / 数据完整性 / 隔离 / 鲁棒性）

| 编号 | 问题 | 位置 | 影响 / 触发 | 修复 |
| --- | --- | --- | --- | --- |
| P2-1 | 多语句写库无事务 | `storage/pools.rs:588-665` 等 | `create_pool` 若 `input.members` 含同账号两次，第 2 条 `UNIQUE` 报错时池/策略/部分成员已落库不回滚 → 半成品脏池 | 用 `unchecked_transaction()` 包裹；入池前按 `account_id` 去重 |
| P2-2 | Agent 配置跨多文件写入无回滚 | `agent_wiring/mod.rs:1004-1260` | 多文件顺序写，任一失败前序已写的文件不还原、binding 表未更新 → 配置半新半旧 | 先写临时文件、全部成功后 rename 提交；失败按 stash 回滚 |
| P2-3 | 改端口/删池静默吞掉 wiring 失败 | `commands/pools.rs:145,241` | `let _ = apply_agent_wiring(...)`；磁盘写失败时池已删/端口已变但 Agent 配置仍旧值，函数仍返回成功 | 改 `?` 传播；失败回滚 binding 并提示 |
| P2-4 | 健康账号计数与真实调度口径不一致 | `domain/pools.rs:944-998` vs `gateway/scheduler.rs:87-89` | `compute_pool_serving_accounts` 未排除 `fallback_api_key_enabled=false` 的 custom_api_key 成员 → UI 显示可用数虚高 | 计数补相同过滤条件 |
| P2-5 | 模型→池路由忽略 Agent 挂载绑定 | `gateway/mod.rs:526-583` | `/v1/models` 按挂载过滤，但实际请求遍历所有启用池按 model 匹配 → Agent 可请求未挂载池的同名模型，M:N 隔离失效、可能用错账号 | 路由时校验目标池 ∈ 该 agent mounted 集合 |
| P2-6 | 请求体读取不完整/无上限 | `gateway/mod.rs:1655-1693` | 不支持 `Transfer-Encoding: chunked`（无 Content-Length 时 body 被忽略→400）；>16MB 静默截断；`read_exact` 无读超时（慢速占满 16MB） | 支持 chunked 或返回 411；超限返回 413；加读超时 |
| P2-7 | 无并发/头部长度上限、连接无 idle 超时 | `gateway/mod.rs:1640-1674,192-194` | 每 accept 即 spawn，`read_line` 无长度/条数上限 → 本地进程可耗尽 fd/内存 | 加信号量并发上限、header 长度上限、读/idle 超时 |
| P2-8 | 上游 `base_url` 直接取自凭据、无 scheme/host 校验（SSRF） | `gateway/mod.rs:766,1264-1271`；`storage/model_discovery.rs:12-17,435-448,525-622` | 被污染的 `base_url`/`endpoint` 可让网关携带 token 请求内网/`169.254.169.254`；`model_discovery` 仅 `starts_with("http")`、默认跟随重定向、`resp.json()` 无大小上限 | 强制 https + 域名白名单；`redirect(none)` 或逐跳校验；响应体加 MAX_BODY 上限 |
| P2-9 | RoundRobin 高并发“惊群” | `gateway/scheduler.rs:50-54`；`gateway/mod.rs:1751,1999-2006` | `today_requests` 并发读到同一快照 → N 个并发请求同一 offset 全打同一账号 → 429 风暴 | 进程内 `AtomicU64` 游标或 in-flight 计数预留 |
| P2-10 | `set_pool_tool_models` 覆盖该工具所有成员为同一模型集 | `storage/pools.rs:1256-1260` | 主/备账号差异化模型被一次“按工具选模型”抹平 | 改并集合并或仅更新显式勾选成员 |
| P2-11 | `list_pools` 内存补出的 model_policies 不落库 | `storage/pools.rs:485-486,953-963` | 无 policy 行的模型显示 enabled，点开关 `toggle_pool_model_enabled` 查无行返回 NotFound → “可见不可切” | 读路径不造影子 policy，或 toggle 改 upsert |
| P2-12 | 调度重排把未列出账号按字母序追加 | `storage/pools.rs:1357-1369` | `remaining_accs.sort()` 打乱主备顺序 | 按现有 `priority` 排序追加 |
| P2-13 | `cooldown_seconds` 无上限、u64→i64 溢出 | `storage/pools.rs:624,725`；`commands/pools.rs:419` | 超大值 `as i64` 变负→读路径被 `.max(10)` 静默改回 10；`*1000` 位极值回绕 | clamp 上限（如 86400）；`checked_mul` |
| P2-14 | `today_requests/today_failovers` 从不按日重置 | `storage/pools.rs:1500-1530` | 名为 "today" 的统计永久累计失真 | 加 `stat_date` 或应用层按天归零 |
| P2-15 | 删除唯一成员/账号留下空池 | `storage/pools.rs:1471-1498`；`storage/mod.rs:1210` | Agent 专属池变 0 成员仍 enabled，网关直接 503 | 删后若池空则自动禁用或提示 |
| P2-16 | 前端 Mock 数据缺生产门禁 | `usePoolPrototypeStore.tsx:26-45,525-546,177-239` | 账号域有 `mockStoreLoader`（生产恒空），号池 store 静态 import mock；若 `__TAURI_INTERNALS__` 探测失效则展示假池/假候选/假成功 | 仿 `mockStoreLoader`，非 DEV 返回空集合 |
| P2-17 | 乐观更新 + 全量快照回写、并发无排序保护 | `usePoolPrototypeStore.tsx:578-642` 及多 mutator | 快速连点/并行 invoke 响应乱序，旧快照覆盖新状态 → 网关启停显示与实际相反、trace 被抹 | 加单调 `opSeq` 只应用更新序号；或命令局部合并 |
| P2-18 | 路由演练 Tauri 下双跑（TS 本地算 + Rust 后端算） | `usePoolPrototypeStore.tsx:1590-1930`；`commands/pools.rs:778-788` | 弹窗显示 TS 结果、路由页显示 Rust 结果，两侧算法不同则结论打架 | Tauri 下以后端返回为准，本地仅纯 mock 预览 |
| P2-19 | `Date.now()` 生成 ID 同毫秒撞键 | `usePoolPrototypeStore.tsx:801,1183,1598,1852` | 快速连点产生重复 member.id/trace.id → 误操作、key 冲突 | `crypto.randomUUID()` 或自增计数器 |
| P2-20 | agent_wiring 测试全部注入 `base_dir_override`，未覆盖真实 `$HOME` 与权限位 | `agent_wiring/mod.rs:1462+` | 真实目录/0600 权限/注释保留等场景无回归护栏 | 补真实临时 HOME 集成测试 |

---

## 6. P3 — 低危 / 工程卫生

1. **迁移吞错**：`storage/mod.rs:188-207` 用 `let _ = conn.execute("ALTER TABLE ...")`，磁盘满/占用等真实错误被忽略；建议 `PRAGMA table_info` 判列 + `user_version` 版本化迁移。
2. **缺索引/外键**：`account_discovered_models.account_id` 无 FK（删号残留）；`pool_members(account_id)` 无索引（CASCADE/JOIN 全表扫）；`agent_bindings.mounted_pool_ids_json`、`gateway_request_logs.pool_id` 无 FK。
3. **字节切片 panic**：`storage/pools.rs:1757-1760` `created_at[11..19]` 遇多字节 UTF-8 可能落在非字符边界；改 `get(11..19)` 或 `chrono` 解析。
4. **全局锁 Poison 级联**：`gateway/mod.rs:75` 等 `lock().expect("gateway lock")`，持锁 panic 后所有请求级联 panic；建议 `unwrap_or_else(|e| e.into_inner())`。
5. **启动不自动恢复网关/端口**：`lib.rs` 启动流程无 `start_server`；重启后已挂载 Agent 指向未监听端口（设计为默认关闭可接受，但需在 UI 明示“需重新启动网关”）。
6. **`session_affinity` 无上限增长**：`gateway/mod.rs:435-439` 每次成功写 `pool::model::session` 从不清理，长时运行内存缓增；加 LRU/上限。
7. **`finish_reason`/`stop_sequences`/`top_p`/`response_format` 等采样参数丢失**：`ir.rs` 构造器不含；Anthropic 侧不写 `temperature`。
8. **`count_tokens` 估算粗糙**：`gateway/mod.rs:1825-1836` 用 `body_bytes.len()/4`，偏差致上下文裁剪/超限 400。
9. **前端定时器未清理**：`AgentBindingsView.tsx:355`、`PoolsView.tsx:164` copyText 的 `setTimeout` 卸载不清理。
10. **`expandedId` 初始化不随异步加载更新**：`RoutingTraceView.tsx:28` 真机首屏不展开、新 trace 不跟随。
11. **`主力旗舰` 计数与筛选谓词不一致**：`AgentBindingsView.tsx:301-306` vs `274-282`，标签数字偏大。
12. **类型漂移**：`types/pool.ts` 字面量联合 vs Rust `Option<String>`；`PoolWorkspaceState` 在 store、`types/pool.ts`、`domain/pools.rs` 三处重复定义。
13. **主密钥明文镜像**：`platform/mod.rs:989-1072` 钥匙串成功时仍保留 0600 明文 `master.key` 于数据目录（密钥与密文同盘）；建议 UI 明示或用钥匙串导出的加密 blob。
14. **`-wal/-shm` 权限仅在打开瞬间存在的才 chmod**：`storage/mod.rs:85-93`，后续新建的 sidecar 可能按 umask 落盘（数据目录 0700 兜底）。
15. **自定义客户端路径允许落在 `$HOME`**：`platform/mod.rs:198-219`，同名恶意程序可被 `Command::new` 执行；建议收窄到真实安装目录。
16. **依赖重复大版本**：`base64 0.21/0.22/0.23`、`rand 0.8/2.x`、`sha2 0.10/0.11`、`ring 0.17/3.x` —— 非漏洞，但增大审计面。
17. **单 chunk 531 KB**：`vite build` 告警，建议 `manualChunks` 或路由级 `import()` 拆包。
18. **文档漂移**：README「134 Rust / 122 Vitest / 36 命令」实测「185 / 132 / 70」；`POOL_GATEWAY_DESIGN` 标 v0.6.x 而代码 v0.5.1；建议发布前统一口径。
19. **未提交改动**：工作区 3 个文件未入库（Dashboard 网关快捷启停 UI 优化 + AgentBindingsView 去重上提）。经核对为合理重构、`tsc`/lint/测试通过、无死 import，但其中删掉了「客户端下拉菜单实时预览（跨工具汇总）」这一唯一跨工具一览能力，需产品确认后决定是否补回，并尽快提交入库。

---

## 7. 上一轮安全修复回归核对（均未回归 ✅）

| 修复项 | 现状 | 证据 |
| --- | --- | --- |
| update 标签 `trim_start_matches(['v','V'])` + 官方仓库/重定向逐跳/64 位 hash | ✅ 仍生效 | `update.rs:252,89-134,472-502` |
| 外部插件目录名==id + 内容哈希固定 + 环境隔离 + stdin 超时 | ✅ 仍生效 | `adapter/external.rs:246-310,543-615` |
| Zip 纯 Rust 安全解压 + TempDirGuard + 0600/0700 | ✅ 仍生效 | `commands/profile.rs:11-225` |
| MiniMax 快照清空 `tokens`/`user` | ✅ 仍生效 | `minimax.rs:715-717` |
| 凭据文件 `write_private_file_atomic` 0600 | ✅ 仍生效 | `adapter/mod.rs:82-131` |
| `uid_hash` HMAC-SHA256 + 迁移幂等 | ✅ 仍生效 | `crypto.rs:194-201`；`storage/mod.rs:701-755` |
| AES-GCM AAD(`QWA1`) + 旧密文迁移 | ✅ 仍生效 | `crypto.rs:32-175` |
| 日志脱敏（伪脱敏邮箱 / Authorization / JWT） | ✅ 仍生效 | `crypto.rs:490-522` |
| 数据目录 0700 / DB 0600 / secure_delete / busy_timeout | ✅ 仍生效 | `storage/mod.rs:85-94,123-133` |
| CSP `script-src 'self'` / 无 fs·shell·http 越权 capability | ✅ 良好 | `tauri.conf.json:42`；`capabilities/*.json` |

---

## 8. 分阶段修复路线

### 批次 A：发布前止血（必须先做）
1. **P0-1** 网关透传 `tools`/`tool_choice`（+ 单测）。
2. **P0-2** `cargo fmt` + 修 8 处 clippy，恢复 CI 绿。
3. **P1-1** 网关每装机随机 token + 全路由鉴权 + 禁止任意 bearer 透传。
4. **P1-3** Agent 配置「解析失败即 Err 中止」，杜绝空文档覆盖。
5. **P2-1/P2-2** 写库加事务、配置多文件写入原子提交/回滚。

### 批次 B：核心体验与隔离
6. **P1-2** 真流式 + 长读超时。
7. **P1-4** 结束原因/usage 双向映射。
8. **P1-5** 确认 `PoolsView` 去留（接线或删除）并清理死代码与误导性测试。
9. **P2-5** 路由按 mounted 绑定隔离；**P2-4/P2-10/P2-11/P2-12/P2-13/P2-14** 号池口径与逻辑修正。

### 批次 C：加固与工程卫生
10. **P2-6/7/8/9** 网关 HTTP 解析（chunked/上限/超时/并发/SSRF/轮询）。
11. **P2-16~20** 前端 Mock 门禁、并发快照序号、双跑统一、ID 生成、测试补齐。
12. **P3 全部**：迁移、索引/外键、切片 panic、锁 poison、session_affinity 上限、文档口径统一、拆包、未提交改动入库。

---

## 9. 一键复验清单（建议 CI 与本机同跑）

```bash
# 前端
npm ci
npm run lint && npm run build && npm test
npm audit --omit=dev

# 后端（在 src-tauri 下）
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked --all-targets
cargo audit --file Cargo.lock            # 本机先 cargo install cargo-audit

# 网关端到端（新增回归）
# 1) 带 tools 的 /v1/chat/completions → 断言上游 body 含 tools
# 2) 无/错/对 token → 401/401/200
# 3) 慢速分块上游 → 首字节 <1s 且总时长可 >20s
# 4) 含尾逗号 JSONC 执行挂载 → 原配置不被覆盖、返回明确错误
# 5) 重复账号入池 create_pool → 无半成品池（事务回滚）
```

---

> **总体判断**：上一轮审计的**安全基座依然稳固**（更新白名单、插件零信任、加密 AAD/HMAC、权限位均未回归），但本轮「号池 + 网关」大版本在**功能正确性（tools 丢失、伪流式/超时）、访问控制（网关无鉴权）、数据完整性（无事务、配置覆盖）与发布门禁（fmt/clippy 双红）**四个维度存在阻断级问题，且带入了约 2.6k 行生产不可达的死代码与虚假测试覆盖。建议按批次 A 止血后再对外发布。

---

## 11. 修复进度追踪（持续更新）

> 本节记录按批次落地的修复提交与剩余项，作为报告的最新状态口径（截至 2026-11-24 round 3）。

### 已修复并推送（main）

| 提交 | 类别 | 内容 |
| --- | --- | --- |
| `042109f` | A-1 | 网关透传 `tools`/`tool_choice`（工具调用恢复）、Agent 配置解析失败即中止、`create_pool` 事务化、fmt/clippy 恢复 CI |
| `6f98807` | A-2 | 网关每装机随机绑定令牌 + 全路由鉴权（无/错令牌 401）、wiring 失败不再静默 |
| `e11d781` | A-3 | Agent 多文件写入失败按 stash 回滚 |
| `6305563` | B-1 | 结束原因与 usage 跨协议归一，Anthropic 流式捕获 output_tokens |
| `d1d4f77` | P1-5 | 删除生产不可达的 `PoolsView` 及三个号池弹窗死代码 |
| `73bb5a3` | P1-2 | 网关推理改用 600s 长超时专用 client，消除 20s 截断 |
| `f716087` | P2 | 健康计数与调度口径一致、cooldown 钳制、跨池路由按挂载隔离 |
| `d3a6835` | P2-6/7/8 | 网关 HTTP 解析加固（chunked/头体上限/读超时/413）+ 上游 base_url 白名单 |
| `2ca39b1` | P2-16/17/19 | 前端 Mock 生产门禁、命令响应乱序保护、UUID ID |
| `a2a17d9` | Docs | README 测试/命令数口径 |
| `620a0b1` | P3 | 迁移 ALTER 幂等、pool_members 索引、字节切片防 panic、锁 poison 自愈、session_affinity 上限 |
| `75b657d` | P1-2b | OpenAI Chat 同协议真流式透传 + 流式回归测试 |
| `cb5a7d2` | P2-12/15 | 调度重排保留优先级、空池自动禁用 |
| `f16e4ab` | P2-11/14 | 影子 policy upsert（可见可切）、today 统计跨日重置 |
| `8e9e20d` | P3 | bundle 按 vendor/react 拆包消除 500KB 告警、设计文档版本口径 |
| `513c068` | Docs | 追加修复进度追踪章节 |
| `1596858` | P1-2/P2-18 | 真流式扩展到同协议 Anthropic；Tauri 演练不再本地双写、统一以后端刷新为真源 |

### 已知剩余项（非阻断，后续可继续）

- **P1-2 跨协议流式（可选增强）**：同协议（OpenAI Chat、Anthropic Messages）已真流式透传；仅 OpenAI↔Anthropic/Responses **跨协议**仍「缓冲后合成 SSE」（功能正确，仅 TTFT 非增量，不影响正确性）。
- **P2-10**：`set_pool_tool_models` 按工具统一覆盖成员模型集——与 Agent 中心「按工具选模型」语义一致，保留。
- **P2-18**：已消除 Tauri 下本地双写，演练结果统一以后端 `run_pool_simulation_cmd` 落库刷新为准。
- **P3（纵深优化）**：`-wal/-shm` 打开后新建 sidecar 权限（已在打开时收紧，边角）、个别 `let _` 吞错建议补日志、Linux 平台能力等。

### 后续优化（第二轮，goal round）

| 提交 | 内容 |
| --- | --- |
| `8b7c543` | P2-9：RoundRobin 改用进程内原子游标，消除并发同序惊群 |
| `01a28ef` | 回归测试：chunked 请求、create_pool 重复账号去重、Agent 配置解析失败不覆盖 |
| `59c0abf` | 上游响应体 >64MB 拒绝（防 OOM）、refresh 锁 poison 自愈 |
| `88a6e00` | 路由 trace 写入失败补日志、跨日统计重置回归测试 |
| `11dd96b` | mock 号池 fixture 用 `import.meta.env.DEV` 守卫，生产构建 DCE 剔除（index 351→334KB，dist 不再含假数据） |
| `efc4dde` | 跨协议增量真流式（OpenAI Chat ↔ Anthropic Messages，纯文本无工具场景）+ 双向回归测试 |

**跨协议流式范围说明**：同协议（OpenAI Chat、Anthropic Messages）走零拷贝透传；跨协议在「无 tools 的纯文本请求」下做 SSE 事件增量翻译（已含双向测试）；**携带 tools 的请求（编码 Agent 常态）以及 OpenAI Responses 协议跨协议**仍走缓冲+合成，以保证 tool_calls/thinking/加密推理块语义正确——这是有意的正确性取舍，非缺陷。
