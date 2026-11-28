# Qwitch 号池网关与 Agent 指向架构设计（路线 C）

> **文档状态**：设计基线（支撑产品原型审阅与后续分期落地）  
> **适用版本**：v0.5.1 已落地（原 v0.6.x 规划基线，已在 0.5.1 代码库实现网关/号池/挂载）  
> **关联文档**：[架构与基线设计](ARCHITECTURE.md) · [核心契约与数据模型](CORE_DESIGN.md) · [内置适配器规范](ADAPTER_SPEC.md) · [号池网关 PRD 与 UX 规范](POOL_GATEWAY_PRD_UX.md)

---

## 1. 产品范式与核心定位

### 1.1 「号池做账号底座 · 模型显式直出」vs magpie「订阅×模型散装」

在调研同层级项目 [magpie](../../magpie/README.zh-CN.md) 后，Qwitch 明确了截然不同的主次范式——**号池管账号高可用底座，模型在使用时是首要显式选择项**：

| 维度 | magpie（订阅 × 模型散装） | Qwitch v2（号池底座 · 模型显式直出 → M:N Agent） |
| --- | --- | --- |
| **第一公民** | 散装 `provider/model` 条目 | **分层账号池（T1/T2/T3）+ 显式开放模型清单（`PoolModelPolicy`）** |
| **使用与切换心智** | 下拉框混杂几十上百个不同订阅商前缀的模型 | **客户端直接选具体模型**（如 `claude-sonnet-4.6`、`deepseek-v4`）；网关**严格锁模绝不私自偷换模型**，后台全自动在具备该模型的多个同/跨工具账号间调度与 429 接力 |
| **异构额度处理** | 未区分跨工具百分比量纲差异 | **两层调度架构**：跨工具/跨档严守 `T1 主力 → T2 备选 → T3 付费 API Key 兜底`，**仅在同类型工具多账号间按 `quota_snapshots` 水位竞争** |
| **Agent 挂载方式** | 往所有客户端灌全量模型目录 | **1 Agent 可同时挂载多个号池（M:N）**，只向客户端暴露你勾选开放的精选模型并指定 `★ 默认模型` |
| **系统重量** | Go 100+ 包 + 全套插件市场/MCP 路由/记忆系统 | Tauri v2 + Rust 轻量内核，纯本地、低内存、零遥测、严格 IPC ACL |

### 1.2 不可妥协的工程与安全边界

沿用 [ARCHITECTURE.md](ARCHITECTURE.md#L17-L21) 与安全基线：
1. **Local-First & Human-in-the-Loop**：本地网关**默认关闭**，必须由用户主动开启服务、主动创建号池并确认挂载某 Agent；不静默扫描或劫持未授权配置文件。
2. **零明文插件落盘**：严禁外部插件在磁盘写入 `plugin-auth.json` 等明文凭据；所有 Token 均由 Rust 主进程按需从 AES-256-GCM SQLite 解密后经内存通道单次注入，刷新后由 Rust 单飞加密落盘。
3. **Loopback 隔离与反重绑定**：网关仅绑定 `127.0.0.1:<port>`（默认 `8187`），强校验 `Host` 头与绑定专属 Token（`qwitch-bind-<token>`），拦截浏览器 `Origin` / `Sec-Fetch-*` 跨域与 DNS Rebinding 请求（`403`）。
4. **IPC 最小权限延续**：前端永远不传递任意配置文件写入路径；目标 Agent 的探测、读写、备份与还原均由 Rust 后端硬编码白名单目录或受控环境变量解析完成，并按 `main` / `tray` 窗口拆分 capability。

---

## 2. 总体架构（路线 C：纯 Rust 稳核心 + 极简 OpenCode 凭据桥接宿主）

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  1. 目标 Agent 挂载层（纯 Rust: src-tauri/src/agent_wiring/）              │
│     Codex 桌面版 · Claude 桌面版 · DeepSeek Harness · OpenCode ·           │
│     MiMo Code · ZCode · 通用标准端点（6 大客户端 + 环境变量导出）          │
│     职责：配置探测 · Stash 原值备份 · 最小键注入 · Drift 漂移检测 · 还原   │
└───────────────────────────────────▲────────────────────────────────────────┘
                                    │ HTTP 127.0.0.1:8187 (Loopback + 随机 Token)
┌───────────────────────────────────▼────────────────────────────────────────┐
│  2. 本地多协议网关与号池引擎（纯 Rust / tokio TcpListener: gateway/）      │
│     ├─ 协议解析 & IR 互转：OpenAI Chat ↔ Anthropic Messages ↔ OpenAI Resp. │
│     ├─ 流式与工具：同协议 SSE 透传 · 跨协议增量文本+工具 · tools 完整透传  │
│     ├─ 号池调度器：消费 quota_snapshots（水位优先 / 耗尽 / 顺序 / 原子轮询）│
│     ├─ 稳定性护栏：会话亲和（保 Prompt Cache）+ 首字节前 Failover + 冷却   │
│     └─ 凭据锁隔离：按 account_id 的 RefreshSingleFlight（独立于签到锁）   │
└─────────────────────────┬───────────────────────────────┬──────────────────┘
                          │                               │
                          ▼                               ▼
┌────────────────────────────────────┐   ┌───────────────────────────────────┐
│ 3A. Rust 原生标准上游（零运行时）   │   │ 3B. 原生私有协议 & 外部插件桥     │
│ • 多厂商 API 资产（api_<slug>）    │   │ • 7 个内置订阅 Rust 原生协议       │
│   显式协议 + 同厂商多 Key 池 + T1  │   │   （Trae / WorkBuddy / Qoder /    │
│ • 协议干净的原生内置订阅           │   │    MiniMax / Lobster / 豆包 / 码道）│
│   （MiniMax / WorkBuddy 等直连）   │   │ • 外部插件按需走 NDJSON 子进程桥   │
└────────────────────────────────────┘   └───────────────────────────────────┘
```

### 2.1 为什么选择「路线 C（改良混合架构）」

1. **避免主仓变成逆向维护黑洞**：magpie 的实战教训（[magpie/AGENTS.md](../../magpie/AGENTS.md#L25-L46)）证明，国内 AI 工具客户端几乎每月调整 UA 校验、设备签名、模型列表甚至触发条款封控。把易变的厂商私有推理协议隔离进极简插件桥，主仓只维护网关 IR、号池路由、Agent 挂载与零信任安全边界，可大幅降低发版频率与主仓合规风险。
2. **避免账号分裂与两套状态**：不照搬 magpie 近 4000 行 `host.js` + `plugin-auth.json`。Qwitch 的 `qwitch.db` 仍是全系统唯一的账号真相源（Single Source of Truth），查额、每日签到、桌面冷切与号池推理共享同一套加密凭据。
3. **极简运行时哲学**：未启用桥接类上游时完全零外部依赖（纯 Rust 跑通通用 API Key 与原生 HTTP 通道）；启用插件上游时按需复用本机 `bun` / `node` 或受控独立运行环境。

---

## 3. 核心引擎设计

### 3.1 统一协议中间表示（IR）与双向流式转换

参考 `magpie/internal/gateway/ir.go` 设计，采用星型 IR 避免 `N × N` 两两转换爆炸：

```rust
pub enum WireProtocol {
    OpenAiChat,       // POST /v1/chat/completions
    AnthropicMessages,// POST /v1/messages, /v1/messages/count_tokens
    OpenAiResponses,  // POST /v1/responses, /v1/responses/compact
}

pub enum IrPart {
    Text { text: String },
    Image { media_type: String, data_base64: Option<String>, url: Option<String>, detail: Option<String> },
    File { media_type: String, data_base64: Option<String>, url: Option<String> },
    ToolCall { id: String, name: String, args_json: String, thought_signature: Option<String> },
    ToolResult { call_id: String, content: String, is_error: bool, images: Vec<IrPart> },
    Thinking { text: String, signature: Option<String>, sealed_json: Option<String>, sealed_by: Option<WireProtocol> },
}
```

**关键转换约束**：
- **SSE 背压与首字节前门控（Pre-First-Byte Gate）**：上游返回非 `2xx` 或在首包有效内容（Content/ToolCall/Thinking）写出前断流，网关**不向客户端写状态码**，而是立即触发 Failover 尝试池内下一个候选账号；一旦向 Agent 写出首字节（`200 OK` + SSE chunk），该次请求锁定上游不再切换，避免半截响应拼接损坏对话。
- **推理签名隔离**：Anthropic 的 `thinking.signature` / `redacted_thinking` 与 OpenAI Responses 的 `encrypted_content` 仅在同源协议间原样透传；跨协议调度时自动降级为纯文本思考或安全剥离非法密封块，避免上游返回 `invalid_encrypted_content` 400 错误。
- **Tool Call ID 与连续助手消息归并**：兼容 Anthropic「并行 `tool_use` 拆段」与 OpenAI `call_id` 长度限制（超长 ID 确定性映射回收）。

### 3.2 两层号池调度引擎（锁模 · 梯队分层 · 同工具水位竞争）

每次请求明确携带目标 `requested_model`（严格锁模原则），调度执行 4 步管线：

1. **同模前置过滤**：
   - 仅保留 `supported_models` 包含 `requested_model` 的账号成员；
   - 排除 `enabled == false`、`accounts.status != Active`、处于运行时 `cooldown_until_ms > now` 的成员；
   - 若池策略 `skip_active_client_account == true`，跳过 `is_active_on_client == true` 的成员；
   - 若 `fallback_api_key_enabled == false`，排除 `tool_id == "custom_api_key"`；
   - 强制排除 `codearts` 中标记为 `client_readonly` 的只读监控账号（防一次性 refresh token 互废）。
   - 若主模型无可用成员：**默认严格报错拒绝私自换模型**；仅当用户在该模型的 `PoolModelPolicy` 显式配置了 `fallback_model_id` 时才转入备用模型轨道并透明记录日志。
2. **两层排序（梯队优先 × 梯队内策略）**：
   - **跨梯队铁律**：严格按 **`T1 主力档 → T2 备选降级档 → T3 付费兜底档（API Key 强制锁定）`** 分层；只要 T1 存在健康成员，绝不降级至 T2 / T3。
   - **梯队内部策略**：
     - `quota_first`（**同工具水位优先**）：不同工具额度量纲不同，跨工具严守用户编排顺位，**仅在同类型工具多账号间按 `quota_snapshots` 剩余水位降序竞争**；
     - `drain`（**单号优先耗尽**）：在当前最高可用梯队内持续锁定最靠前可用账号直到耗尽/冷却，最大化上游 Prompt Cache 命中率；
     - `order`（**固定顺位**）：严格按梯队与成员 `priority` 顺序顺延；
     - `round_robin`（**均衡轮询**）：在当前最高可用梯队内轮转分摊并发压力。
3. **会话亲和（Session Affinity）覆盖**：
   - 提取会话键：优先 `X-Conversation-ID` / `session_id` / OpenCode session header，缺失时取首条用户消息摘要哈希；
   - 模式：
     - `auto_cache`（默认）：同一对话在 Cache 未冷却窗口（默认 5 分钟，ChatGPT/DeepSeek 类长缓存设 60 分钟）内优先置顶上次成功账号；
     - `session`：整个会话强绑定上次成功账号（除非其冷却/过期）；
     - `turn`：仅在同一轮 Tool Use 循环中保持一致，新用户轮次允许重新按策略择优；
     - `off`：每次请求独立按策略重排（仅建议无密封思考且无缓存依赖的场景使用）。
4. **Failover 与 状态回写**：
   - 上游返回 `401` 且配置 `auto_refresh_on_401`：进入按 `account_id` 的 `RefreshSingleFlight`，仅一个协程刷 Token，成功则加密写回 `qwitch.db` 并原地重试一次；刷新失败则标记 `accounts.status = Expired` 并切换下一号；
   - 上游返回 `429`：对当前成员写入 `cooldown_seconds`（默认 60s，尊重 `Retry-After`）并无缝切到池内下一号；
   - 上游返回 `403` / 明确风控码：标记 `accounts.status = RiskPaused`，发出 `qwitch://account-updated` 事件同步主窗口与托盘，并无缝切下一号；
   - 末位兜底：若号池配置了标准 API Key 账号，可在所有订阅号均冷却或耗尽时作为最终兜底。

### 3.3 锁隔离与并发安全

- **绝不复用 `AntiRiskScheduler.queue_lock`**：现有的 `AntiRiskScheduler` 是单队列 + 1.5~3.5s 随机抖动的串行防风控队列（见 [scheduler/mod.rs](../../src-tauri/src/scheduler/mod.rs)）。网关长连接 SSE 推理必须并发异步执行，不能被批量签到/查额阻塞，也不能反过来阻塞查额。
- **共享 `RefreshSingleFlight`**：按 `account_id` 粒度建立内存级异步单飞表 `DashMap<AccountId, Arc<Mutex<()>>>`（或 `tokio::sync::Notify` 广播结果），确保「网关并发请求」与「用户手动刷新额度」同一时刻不会对同一账号发出两次 `refresh_credential`。

### 3.4 极简 OpenCode 凭据桥接宿主（`host.mjs`）

- **协议裁剪**：基于 stdio 行级 NDJSON RPC（参考 `magpie/internal/plugin/host.js` 的背压与取消机制，剔除 MCP / 中间件 / 搜索 / Pi 扩展等无关模块），仅保留：
  - `init`：加载已通过 Qwitch 哈希信任校验的插件入口；
  - `list_models`：执行 `config` 钩子采集声明的模型元数据；
  - `fetch_stream`：携带本次选中账号的内存 `auth` 快照，在 `AsyncLocalStorage` 作用域内执行 `auth.loader` 的自定义 `fetch`，流式回传 `head` / `chunk(base64)` / `done`，支持 `abort` 与流控 `credit`。
- **凭据注入与回写契约**：
  - 桥接进程禁用本地 `plugin-auth.json` 文件写入；拦截插件调用的 `client.auth.set(...)`：仅暂存当前请求上下文并通过 `{ event: "auth_refreshed", accountId, authPatch }` 回传 Rust；
  - Rust 侧验证后反向合并入 `CredentialEnvelope`，经 `AES-256-GCM(AAD)` 更新 `qwitch.db`。
- **安全复用**：继续走 Qwitch [adapter/external.rs](../src-tauri/src/adapter/external.rs) 机制——目录内容 SHA-256 哈希绑定、原生系统弹窗一次性信任票据、环境变量严格白名单（仅透传 `PATH`/`HOME`/`TMPDIR` 等必需变量，绝不透传无关凭据）。

---

## 4. 7 大上游订阅接入矩阵与约束

全部 7 个内置工具账号均可直接作为号池成员源，同时支持「通用标准 API Key（OpenAI / Anthropic / Gemini 兼容）」作为稳态兜底：

| 上游工具 (`tool_id`) | 通道策略 | 参考来源 / 模块 | Qwitch 凭据桥接字段 | 关键约束与工程对策 |
| --- | --- | --- | --- | --- |
| **MiniMax Code CN** (`minimax_code_cn`) | **Rust 原生** | `gateway/mod.rs` + `storage/models.rs` | `auth.json` 明文 `accessToken` + `refresh_credential` | Token ~1h 过期，依赖 `RefreshSingleFlight` 提前 5 分钟刷新；实时模型探测对齐 `?region=cn&buildEnv=prod` 与对象映射解析 |
| **WorkBuddy** (`workbuddy`) | **Rust 原生** | `gateway/mod.rs` | `decrypted_token.txt` / `workbuddy-desktop.info` + `uid` | `copilot.tencent.com` 强制校验 `User-Agent: WorkBuddy/<ver>`（错误码 `10085`）；需处理加密快照未解密时的状态引导 |
| **有道龙虾** (`lobsterai`) | **Rust 原生** | `gateway/mod.rs` + `storage/models.rs` | `decrypted_token.txt` + `refreshToken` + `yid` | 已有提前 5 分钟刷新能力并同步回传新 Token 入库；免除活跃桌面端跳过，支持实时模型目录探测 |
| **Qoder CN** (`qoder_cn`) | **Rust 原生** | `adapter/qoder_cosy.rs` + `gateway/mod.rs` | `decrypted_token.txt` + `device_token` + `machine_id` | 对话推理强依赖 `device_token`、`Cosy-MachineId` 与 `rsa 0.10` PKCS#1 v1.5 加密封装及 Cosy 签名；支持工具调用与实时模型目录探测，免除活跃桌面端跳过 |
| **Trae CN** (`trae_cn`) | **Rust 原生 SSE** | `gateway/mod.rs` + `storage/models.rs` | `Cloud-IDE-JWT` + `trae_device.json` | 强绑定账号级设备身份（`x-device-*`），严格复用 `trae_device.json`；实时模型探测对齐 `batch_get_detail_param` / `get_detail_param` 回退；建议开启活跃号互斥 |
| **豆包工作** (`doubaowork`) | **Rust 原生 HTTP/SSE** | `gateway/doubao.rs` | Cookie (`sessionid` / `sid_tt`) + `UniParams` | 字节 AGW 私有流式协议；原生支持会话初始化、增量 SSE/非流式响应、响应后异步清理临时会话，并从本机缓存目录读取模型清单 |
| **华为云码道** (`codearts`) | **Rust 原生签名上游** | `adapter/codearts.rs` + `gateway/mod.rs` | `accessKey` / `secretKey` / `securityToken`（区分 `client_readonly` / `isolated_oauth`） | 华为云 `SDK-HMAC-SHA256` 签名上游（支持 `.myhuaweicloud.cn` 回退）；`client_readonly` 可入池但不主动消耗客户端一次性 `refreshToken`（遇到 `APIG.0602` 过期仅尝试本机磁盘只读重同步），`isolated_oauth` 走单飞 DPoP 刷新；遇到 `TM.00001041` 并发限流做短冷却 |
| **通用标准 API Key** (`custom_api_key`) | **Rust 原生** | `gateway/mod.rs` | API Key + Base URL + Protocol | 零外部依赖、零风控风险；支持放号池末位做免费额度耗尽后的自动兜底 |

---

## 5. 6 大消费端 Agent 挂载规范（Stash + Drift + 无损还原）

所有 Agent 挂载均遵循**四步事务**：`探测 (Detect) → 备份原值 (Stash) → 最小键写入 (Wire) → 漂移校验 (Drift Check)`。解绑时根据 Stash 精确还原。

### 5.1 OpenCode (`opencode`)
- **配置文件**：`$OPENCODE_CONFIG_DIR` 或 `$XDG_CONFIG_HOME/opencode/` 下按优先级查找 `opencode.jsonc` / `opencode.json` / `config.json`。
- **接入协议**：OpenAI Chat Completions（`http://127.0.0.1:<port>/v1`）。
- **核心规范（防坑要点）**：
  - 探测 `opencode --version` 区分 **1.x** 与 **2.x**：
    - **1.x**：写入 `provider.qwitch`（`npm: "@ai-sdk/openai-compatible"`、`options.baseURL`、`options.apiKey`、`models: { "pool/<slug>": ... }`），并设置 `model` / `small_model`。
    - **2.x**：必须写入 **`providers.qwitch`**（`package: "aisdk:@ai-sdk/openai-compatible"`、`settings.baseURL`、`settings.apiKey`），默认模型写入 `agents.title.model` 等 v2 路径。若配置文件同时存在 `provider` 与 `providers`，OpenCode 2 会整块忽略 `provider`，导致更新不可见。
  - **多号池同列映射**：1 个 OpenCode 实例可同时挂载多个 Qwitch 号池，`qwitch` provider 下暴露各号池开启的具体模型清单（`<pool_slug>/<model_id>`，同名模型自动附带号池显示名区分），`model` 设为选定的 `★ 默认模型`。

### 5.2 DeepSeek Harness (`dsh`)
- **配置文件**：`$DSH_HOME`（默认 `~/.dsh`）下的多 Profile 补丁文件：
  - 桌面版：`~/.dsh/profiles/desktop/cordis.patch.yml`
  - Web/CLI：`~/.dsh/profiles/web/cordis.patch.yml`
  - 凭据双处：`~/.dsh/.env` 与 `~/.dsh/.credentials.yaml`
- **接入协议**：OpenAI Chat Completions（`http://127.0.0.1:<port>/v1`）。
- **核心规范（防坑要点）**：
  1. **严禁覆盖 `llm-deepseek` 原生行**：必须修改 `llm-pi-ai`（`@deepseek-ai/dsh-llm-pi-ai`）的 `providers.qwitch` 路由并设置 `agent-default-model`，避免用户在 dsh Models 页改 Key 时互相覆写。
  2. **双写凭据库**：`dsh` **桌面版不读取 `.env`**，必须同时写入 `.env` 与 `.credentials.yaml` 的 `refs.QWITCH_GATEWAY_KEY`；若仅 `.env` 存在而 `.credentials.yaml` 缺失，Drift 检测必须报 `drifted` 并提示一键修复。
  3. **既有会话提示**：`agent-default-model` 仅作用于新建会话，UI 需明确提示已有会话需手动切模型。

### 5.3 Claude 桌面版 (`claude_desktop`)
- **配置文件**：
  - macOS：`~/Library/Application Support/Claude/` 与 `Claude-3p/`（开启独立双开模式时使用隔离目录 `~/Library/Application Support/Claude-Qwitch-3p/` 与隔离启动器 `~/Applications/Claude Qwitch.app`）
  - Windows：`%LOCALAPPDATA%\Claude\` 与 `Claude-3p\`（若检测到 MSIX 商店版，同步覆盖 `LocalCache\Local\` 与真实 `%LOCALAPPDATA%`）
- **接入协议**：Anthropic Messages（`POST /v1/messages`、`POST /v1/messages/count_tokens`、`GET /v1/models`）。
- **核心规范（防坑要点）**：
  1. **`3p` 网关模式写入**：
     - `claude_desktop_config.json` 写入 `"deploymentMode": "3p"`；
     - `Claude-3p/configLibrary/<qwitch-uuid>.json` 写入 `inferenceProvider: "gateway"`、`inferenceGatewayBaseUrl: "http://127.0.0.1:<port>"`、`inferenceGatewayApiKey: "<binding_token>"`、`inferenceGatewayAuthScheme: "bearer"`、`disableDeploymentModeChooser: true`、`coworkEgressAllowedHosts: ["*"]`、`toolSearchEnabled: true`；
     - `_meta.json` 将 `appliedId` 指向该 UUID。
  2. **模型 ID 精准伪装机制（核心！）**：Claude Desktop 客户端内嵌正则黑名单（过滤包含 `deepseek / doubao / glm / gpt / kimi / minimax / qwen / ...` 的 ID）与白名单（必须含 `claude / sonnet / opus / haiku / fable / mythos / anthropic`）。因此网关针对 Claude Desktop 的 `/v1/models` 下发**多号池汇总后的显式模型列表**：
     - 原生含 `claude / sonnet / opus / haiku` 的模型（如 `claude-sonnet-4.6`）：直过白名单无需伪装；
     - 非 Anthropic 名称（如 `deepseek-v4`、`MiniMax-M2.5`、`pangu-coder-pro`）：Claude Desktop ≥ 2.x 底层 ID 自动映射为 **`mythos-qwitch-<pool_model_hash>`**（过白名单且保留 Thinking Effort 滑块），Claude Desktop 1.x 回退为 `anthropic/qwitch-<hash>`；
     - `display_name` 始终保留清晰模型名（当同一 Claude 实例同时挂载多个号池出现同名模型时自动追加 `· <号池名称>` 区分）。
  3. **重启与双开隔离**：配置仅在启动时读取，修改后触发受控重启或引导用户开启 macOS 独立双开副本（保留官方登录态不冲突）。

### 5.4 Codex 桌面版 (`codex_desktop`)
- **配置文件**：`$CODEX_HOME/config.toml`（默认 `~/.codex/config.toml`）。
- **接入协议**：OpenAI Responses API（`POST /v1/responses`、`POST /v1/responses/compact`、`GET /v1/models`）。
- **核心规范（防坑要点）**：
  1. **第一期采用独立 `[model_providers.qwitch]` 模式**：
     - 写入 `model_provider = "qwitch"`、`model = "pool/<slug>"`、以及 `[model_providers.qwitch]`（`name = "Qwitch"`, `base_url = "http://127.0.0.1:<port>/v1"`, `wire_api = "responses"`, `experimental_bearer_token = "<binding_token>"`），同时生成关联模型 catalog JSON（含合理的 `context_window` 与 `auto_compact_token_limit` 防止上下文溢出不压缩）。
  2. **解绑绝不删除 `[model_providers.qwitch]` 表**：Codex 在加载历史线程时若找不到线程记录的 `model_provider` 会直接拒绝加载配置；解绑仅把顶层 `model_provider` 与 `model` 按 Stash 还原，保留 `[model_providers.qwitch]` 定义表。
  3. **WebSocket 升级 `426` 降级**：Codex 对 `/responses` 优先尝试 WebSocket；第一期网关收到 `Upgrade: websocket` 直接返回 `426 Upgrade Required`，Codex 自动平滑降级至 HTTP SSE POST。
  4. **`app-server` 精准热重启**：macOS 上关闭 Codex 窗口时后台 `app-server` 进程不退出导致新配置不生效，Qwitch 提供「仅重启 app-server（不关闭桌面窗口）」动作。
  5. **本机代理 502 诊断**：Codex 底层 `reqwest` 在开启系统全局代理 / `HTTP_PROXY` 时可能把 `127.0.0.1` 发给代理导致 502。挂载与自检流程必须检测本机代理并提示配置 `NO_PROXY=127.0.0.1,localhost`。
  6. **内部辅助任务兜底**：Codex 后台自动发出的 `codex-auto-review`、标题/摘要生成等未知模型名请求，由网关自动回落到该绑定号池的首选模型，避免 404 打断主会话；若为 Codex 自有登录模型且未匹配号池，则透传直连 Codex 自有凭据。

### 5.5 MiMo Code (`mimo`)
- **配置文件**：`~/.mimocode/settings.json`。
- **接入协议**：OpenAI Chat Completions（`http://127.0.0.1:<port>/v1`）。
- **核心规范**：注入 Qwitch 网关端点与绑定令牌；修改模型清单或网关端口后支持从 Qwitch 优雅重启桌面客户端生效。

### 5.6 ZCode (`zcode`)
- **配置文件**：`~/.zcode/settings.json`。
- **接入协议**：Anthropic Messages（`http://127.0.0.1:<port>`）。
- **核心规范**：注入 Anthropic 兼容网关地址与绑定令牌；修改模型清单或网关端口后支持从 Qwitch 优雅重启桌面客户端生效。

---

## 6. SQLite 增量 Schema 与 IPC 契约规划

### 6.1 新增 SQLite 表（不影响现有 4 张表）

```sql
-- 1. 号池主表
CREATE TABLE IF NOT EXISTS account_pools (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT,
    strategy TEXT NOT NULL DEFAULT 'quota_first',      -- quota_first(同工具水位) | drain | order | round_robin
    affinity_mode TEXT NOT NULL DEFAULT 'auto_cache',  -- auto_cache | session | turn | off
    skip_active_client_account INTEGER NOT NULL DEFAULT 1,
    cooldown_seconds INTEGER NOT NULL DEFAULT 60,
    auto_refresh_on_401 INTEGER NOT NULL DEFAULT 1,
    fallback_api_key_enabled INTEGER NOT NULL DEFAULT 1,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 2. 号池开放模型策略表（模型一等公民 + 严格锁模 / 可选显式降级）
CREATE TABLE IF NOT EXISTS pool_model_policies (
    pool_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    fallback_model_id TEXT,                            -- NULL 表示严格锁模，禁止私自换模
    claude_alias_id TEXT,
    PRIMARY KEY (pool_id, model_id),
    FOREIGN KEY (pool_id) REFERENCES account_pools(id) ON DELETE CASCADE
);

-- 3. 号池账号成员表（支持单账号多选模型 + T1/T2/T3 梯队分层）
CREATE TABLE IF NOT EXISTS pool_members (
    id TEXT PRIMARY KEY,
    pool_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    supported_models_json TEXT NOT NULL,               -- JSON 数组：如 ["claude-sonnet-4.6","claude-opus-4.6"]
    tier TEXT NOT NULL DEFAULT 't1_primary',           -- t1_primary | t2_backup | t3_fallback
    priority INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (pool_id) REFERENCES account_pools(id) ON DELETE CASCADE,
    FOREIGN KEY (account_id) REFERENCES accounts(id) ON DELETE CASCADE,
    UNIQUE(pool_id, account_id)
);

-- 4. Agent 指向绑定表（1 Agent 支持同时挂载多个号池 + 默认模型）
CREATE TABLE IF NOT EXISTS agent_bindings (
    agent_id TEXT PRIMARY KEY,                         -- codex_desktop | claude_desktop | dsh | opencode
    mounted_pool_ids_json TEXT NOT NULL DEFAULT '[]',  -- JSON 数组：允许同列多个组合号池
    default_model_key TEXT,                            -- "<pool_id>::<model_id>"
    binding_token_hash TEXT NOT NULL,                  -- HMAC-SHA256 校验用
    options_json TEXT NOT NULL DEFAULT '{}',           -- 双开隔离/Schema版本等专属选项
    status TEXT NOT NULL DEFAULT 'unbound',            -- bound | drifted | unbound | needs_restart
    last_checked_at TEXT,
    updated_at TEXT NOT NULL
);

-- 4. Agent 原始配置 Stash（用于无损解绑还原）
CREATE TABLE IF NOT EXISTS agent_config_stashes (
    agent_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    original_existed INTEGER NOT NULL,
    original_content_encrypted BLOB,                   -- AES-256-GCM 加密，防泄露原第三方 API Key
    captured_at TEXT NOT NULL,
    PRIMARY KEY (agent_id, file_path)
);

-- 5. 网关路由观测流水（独立保留期，默认 7 天 / 上限 2000 条，不存放 prompt/response 明文）
CREATE TABLE IF NOT EXISTS gateway_request_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trace_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    pool_id TEXT NOT NULL,
    incoming_protocol TEXT NOT NULL,
    upstream_protocol TEXT NOT NULL,
    final_account_id TEXT,
    final_tool_id TEXT,
    final_model TEXT,
    status TEXT NOT NULL,                              -- success | failover_success | failed
    cache_affinity_hit INTEGER NOT NULL DEFAULT 0,
    ttft_ms INTEGER,
    total_tokens INTEGER,
    attempts_json TEXT NOT NULL,                       -- 脱敏决策链路
    created_at TEXT NOT NULL
);
```

### 6.2 规划 Tauri IPC 命令与窗口权限边界

- **Main 窗口专属写命令**：`start_gateway` / `stop_gateway` / `get_gateway_status`、`list_pools` / `upsert_pool` / `delete_pool` / `reorder_pool_members`、`detect_target_agents` / `preview_agent_binding_diff` / `apply_agent_binding` / `repair_agent_drift` / `unbind_agent` / `restart_agent_app`、`list_routing_traces`、`run_pool_probe`。
- **Tray 托盘窗口最小授权**：只读查询（`get_gateway_status`、`list_pools`、`detect_target_agents`）与高频热操作（`start_gateway`/`stop_gateway`、`switch_agent_bound_pool`），不允许从托盘删除号池或解绑重配置。
- **实时同步事件**：`qwitch://gateway-status-changed`、`qwitch://pool-updated`、`qwitch://agent-binding-updated`、`qwitch://routing-trace-appended`。

---

## 7. 四阶段落地路线

1. **Phase 1：Rust 网关骨架 + 号池引擎 + OpenCode & DeepSeek Harness**
   - `axum` 网关 + IR（OpenAI Chat ↔ Anthropic Messages）+ 号池调度/亲和/Failover + 标准 API Key 上游 + OpenCode (v1/v2) 与 `dsh`（`.env` + `.credentials.yaml` 双写）挂载与 Stash 还原。
2. **Phase 2：Claude 桌面版 + 极简 OpenCode 凭据桥接（首批 3 家上游）**
   - Claude Desktop `3p` 挂载（含伪装 ID `mythos-qwitch-*` 与 macOS 双开隔离）+ 极简 `host.mjs` 内存凭据桥接 + 打通 `minimax_code_cn` / `workbuddy` / `lobsterai`。
3. **Phase 3：补齐剩余 4 家上游**
   - 接入 `qoder_cn`（设备令牌链）/ `trae_cn`（固定设备头）/ `doubaowork`（AGW 流式）/ `codearts`（强制隔离 OAuth），实现 7 大工具全部自由混编。
4. **Phase 4：Codex 桌面版专攻 + 观测闭环**
   - OpenAI Responses 双向流式转换（WS 426 降级 HTTP SSE）+ `~/.codex/config.toml` 独立表挂载 + `app-server` 精准重启 + 系统代理 502 检测 + 路由决策 Timeline 面板。
