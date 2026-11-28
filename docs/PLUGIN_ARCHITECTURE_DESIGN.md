# Qwitch 双端插件化与入口/出口自动适配架构设计方案

> **文档状态**：架构设计基线（供 Phase 1–4 实施落地）  
> **适用版本**：基于 `v0.8.0`（`4f65eab`）代码库深度审计  
> **关联文档**：[架构与基线设计](ARCHITECTURE.md) · [号池网关与 Agent 指向架构设计](POOL_GATEWAY_DESIGN.md) · [内置适配器规范](ADAPTER_SPEC.md)

---

## 1. 文档定位与背景

经过多轮迭代，Qwitch 已具备扎实的本地优先底座：AES-256-GCM 加密 `qwitch.db`、外部插件 SHA-256 内容哈希钉扎、两层号池调度（T1/T2/T3 梯队 + 同工具水位竞争 + 会话亲和）以及 Agent 配置的 `Detect → Stash → Wire → Drift` 事务机制。

但经过代码级审计，当前架构存在 **4 个核心耦合痛点**，导致每新增一个订阅出口或 Agent 入口都需要修改主仓多处硬编码：

1. **出口（Egress）只做了“半截插件化”**：
   - [adapter/external.rs](../src-tauri/src/adapter/external.rs) 已具备 `manifest.json` + stdio JSON-RPC 的安全外部插件宿主，但仅覆盖账号生命周期（导入、查额、每日签到、桌面冷切）。
   - 真正承接流量的网关推理分发（[gateway/mod.rs](../src-tauri/src/gateway/mod.rs) 达 **6,225 行**）、模型目录探测（[storage/model_discovery.rs](../src-tauri/src/storage/model_discovery.rs)）以及模型映射/协议判断（[domain/pools.rs](../src-tauri/src/domain/pools.rs) 达 **1,730 行**）全部是 `match tool_id` 硬编码，外部插件无法接入号池推理。
2. **入口（Ingress / Agent 接入）完全未插件化**：
   - 6 个内置 Agent 写死在 `enum TargetAgentId`（Rust 6 个文件共 **219 处**引用，其中 [agent_wiring/mod.rs](../src-tauri/src/agent_wiring/mod.rs) 占 148 处且文件达 **3,594 行**；TypeScript 8 个文件共 **100 处**引用，其中 `usePoolPrototypeStore.tsx` 占 80 处、`src/types/pool.ts` 占 5 处）。
3. **网关 IR 协议转换不对称**：
   - [gateway/ir.rs](../src-tauri/src/gateway/ir.rs) 的入站解析（Ingress → IR）支持 `OpenaiChat`、`AnthropicMessages`、`OpenaiResponses` 3 种协议，但出站编码（IR → Egress）目前仅支持 `OpenaiChat` 与 `AnthropicMessages`，**缺少 `IR → OpenaiResponses` 编码器与 Responses 上游 SSE 反向翻译**；同时 `IrPart` 尚未承载图片/多模态内容。
4. **插件注册表不可变且内置 ID 死锁**：
   - [commands/mod.rs](../src-tauri/src/commands/mod.rs#L120) 中 `AppState.adapters` 为不可变 `Arc<HashMap<...>>`，无法热增删；[adapter/mod.rs](../src-tauri/src/adapter/mod.rs#L404-L413) 通过 `RESERVED_ADAPTER_IDS` 锁死内置 ID，导致内置工具无法被用户停用或由外部热修复插件覆盖。

---

## 2. 总体架构与核心边界（Kernel vs Ingress Plugin vs Egress Plugin）

参考 Magpie 的入口/出口解耦思想，同时守住 Qwitch 的本地安全与轻量定位，主仓收口为**「薄内核 + 星型 IR 总线 + 双端统一插件」**：

```text
┌────────────────────────────────────────────────────────────────────────────┐
│  Ingress Plugins（入口 / Agent 接入插件）                                   │
│  • 默认内置：claude_desktop · codex_desktop · dsh · opencode · mimo · zcode│
│  • 外部扩展：qoder · cursor · continue · 自定义 CLI（支持纯声明式零代码）   │
└──────────────────────────┬─────────────────────────────────────────────────┘
                           │ 声明 IngressProfile（协议/硬性需求/可选特性/怪癖）
                           ▼
┌────────────────────────────────────────────────────────────────────────────┐
│                       Qwitch Core Kernel（Rust 稳定内核）                   │
│  1. PluginRegistry (Arc<RwLock<...>>) — 内置/外部平权、热启停、SHA-256 验签│
│  2. CompatibilityEngine — 三跳自动判定（Ingress → IR Bridge → Egress）     │
│  3. Star IR Bus (gateway/ir.rs) — 三协议全双向互转 + 约束自动整形           │
│  4. Pool Scheduler & Vault — qwitch.db 单一凭据源、单飞刷新、探针结果缓存   │
└──────────────────────────┬─────────────────────────────────────────────────┘
                           │ 声明 EgressProfile（协议/能力供给/上游约束）
                           ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  Egress Plugins（出口 / 订阅与上游插件）                                    │
│  • 默认内置：trae_cn · workbuddy · qoder_cn · minimax_code_cn · lobsterai  │
│              doubaowork · codearts · custom_api_key                        │
│  • 外部扩展：codex_sub · 社区订阅插件 · 单文件打包的 OpenCode 薄包装插件    │
└────────────────────────────────────────────────────────────────────────────┘
```

### 2.1 不可妥协的 4 条内核铁律
1. **内置与外部平权**：现有 7+1 个出口与 6 个入口全部封装为内置默认插件，与外部插件实现同一套 `PluginManifest` 与 Trait 契约，统一受 `PluginRegistry` 管理。
2. **热生效注册表**：将 `AppState.adapters` 升级为 `plugins: Arc<RwLock<PluginRegistry>>`，增删/启停/覆盖插件无需重启应用，网关与调度器实时读取最新快照。
3. **零明文凭据落盘**：严禁外部插件自建明文 auth 文件；所有凭据由 Rust 内核从 AES-256-GCM 加密的 `qwitch.db` 按需解密后通过内存通道单次注入，刷新后的新 Token 通过 `auth_refreshed` 事件回传 Rust 单飞落盘。
4. **安全基线继承**：所有可执行子进程插件强制走目录 SHA-256 哈希钉扎、原生系统弹窗一次性信任票据、受控解释器白名单与环境变量白名单。

### 2.2 三层插件运行时策略（为什么不把 Magpie/OpenCode npm 宿主耦合进内核）
OpenCode/Magpie 的 npm 插件生态仅解决“获取 Token + 拦截 `fetch` 发推理请求”，无法覆盖 Qwitch 的本机导入、查额、每日防风控签到、桌面端冷切与 `IngressProfile`/`EgressProfile` 声明；且 `node_modules` 动辄数千文件会击穿 SHA-256 哈希钉扎上限（500 文件 / 32 MiB）。因此 Qwitch 内核只认**一套统一插件契约**，按复杂度分三层运行：

| 层级 | 插件形态 | 适用场景 | 核心优势 |
| --- | --- | --- | --- |
| **Layer 0：纯声明式** | 零代码 `manifest.json`（`transport: "declarative"`） | • **80% 简单入口**：仅需修改 JSON/JSONC/TOML/YAML 的 Agent（如 Qoder、Cursor、OpenCode、MiMo、ZCode 等）<br>• **标准协议出口**：兼容 OpenAI / Anthropic / Responses 的 API Key 或中转站 | 零外部运行时、零安全风险、Rust 原生执行、维护成本最低 |
| **Layer 1：受控子进程** | NDJSON 流式 RPC + SHA-256 钉扎（`transport: "subprocess"`） | • **私有协议出口**：需特殊签名、Cookie、加密头、查额/签到/冷切的订阅（如 Codex 订阅、豆包、Trae、Qoder）<br>• **复杂入口**：需镜像目录或双开隔离（如 Claude Desktop 3p） | 语言无关（Python/Node/Bun/单文件二进制）、单插件包内聚账号与推理全闭环、支持 `daemon` 空闲回收池 |
| **Layer 2：生态薄包装**（可选，不进内核） | 用 `bun build` / `esbuild` 打包成单文件的 Layer 1 插件 | 快速复用现成 `@magpie-community/opencode-*-auth` 社区插件 | 无 `node_modules` 污染、外壳套一层 ~100 行 Qwitch NDJSON 适配器并补齐 `egressProfile`，对内核完全透明 |

---

## 3. 最终裁定的两项关键协议决策

### 3.1 决策 A：外部插件导入与卸载协议（支持目录 + `.zip` 压缩包 + 单文件 `.json`）

经核查 [Cargo.toml](../src-tauri/Cargo.toml#L43-L44)，主仓已内置 `zip = "9"`（deflate）与 `rfd = "0.15"`，**无需新增任何第三方依赖**即可直接落地三轨导入：
- **目录导入**：直接选择已解压的插件目录（适合开发者本地调试）。
- **`.zip` 压缩包导入**：面向普通用户分发可执行（Layer 1）或带图标资源的插件包。
- **单文件 `.json` 导入**：面向纯声明式（Layer 0）插件，用户导入一个 `*.json` 即可自动包装生成 `<data_dir>/plugins/<id>/manifest.json`。

#### `.zip` 导入的 6 条强制安全防线
1. **Zip Slip 防护**：严格使用 `entry.enclosed_name()` 解析相对路径，拒绝一切绝对路径、`..` 父目录穿越、控制字符与非法文件名。
2. **拒绝符号链接**：检查 ZIP 条目的 Unix permission mode，凡包含 symlink 一律中止并报错拒绝。
3. **Zip Bomb 配额限制**：与现有 `adapter_content_hash` 安全上限对齐——解压总文件数 **≤ 500**、解压总字节数 **≤ 32 MiB**、目录层级深度 **≤ 4**、单文件 **≤ 16 MiB**，流式读取边解压边计数，超限立即中止并清理临时目录。
4. **智能单层根目录剥离**：自动兼容“直接打包 `manifest.json` 在根目录”与“外层套了一层 `<folder>/manifest.json`”（如 GitHub Release 自动打包）的两种压缩包结构；若既无根目录 `manifest.json` 也无唯一子目录 `manifest.json`，则明确报错。
5. **Staging 原子落盘与信任确认**：先解压至隔离临时目录 `<data_dir>/plugins/.staging-<uuid>/` → 完整校验 `manifest.json` Schema、入口命令合法性并计算目录 SHA-256 → 弹出系统原生确认框向用户展示插件 ID、名称、能力声明与 SHA-256 指纹（若覆盖内置插件则追加醒目警告）→ 用户确认签发一次性信任票据后，再原子重命名移入 `<data_dir>/plugins/<id>/`。
6. **卸载依赖检查与账号保护**：
   - 若卸载的**入口插件**当前仍处于 `bound` / `drifted` 状态，卸载前强制先执行 `unwire`（按 Stash 无损还原客户端配置）再删除插件目录。
   - 若卸载的**出口插件**在 `qwitch.db` 中仍有关联账号，**默认保留加密账号为只读归档**（防止用户误删插件导致辛苦导入的账号凭据丢失），同时在确认弹窗提供可选勾选「同步清理关联账号与号池成员」。
   - 启动时自动兼容扫描旧目录 `<data_dir>/adapters/` 并提示/平滑迁移至 `<data_dir>/plugins/`。

### 3.2 决策 B：默认内置插件的生命周期（逻辑停用/隐藏 + 外部同 ID 热覆盖 + 一键恢复内置）

- **为什么不对默认插件做物理文件删除**：内置默认插件的代码逻辑编译在 Qwitch 主二进制内，若将默认插件强行拆成磁盘外部脚本，不仅会破坏 macOS 应用签名公证、引发 Windows 杀软对释放脚本的误报，还会导致主版本升级时难以同步内置修复。
- **统一的插件管理体验**：
  1. **逻辑停用 / 隐藏**：用户在「插件中心」关闭任意默认插件后，状态写入 SQLite `plugin_states (enabled = false)`。停用后该插件从账号导入、侧边栏、号池候选、Agent 绑定列表及网关路由中**彻底隐藏/摘除**，体验上与卸载完全一致，且可随时一键重新启用。
  2. **外部同 ID 热覆盖（解除死锁）**：废除 [adapter/mod.rs](../src-tauri/src/adapter/mod.rs#L404-L413) 中阻断外部插件使用内置 ID 的硬编码限制。允许用户导入与内置插件同 ID 的外部插件（例如某厂商月底改了签名协议，用户无需等 Qwitch 发版，直接导入社区热修复 `.zip` 覆盖官方内置实现）；导入时触发**二次原生弹窗警告**，明确提示“正在覆盖内置官方插件”并展示 SHA-256 指纹。
  3. **一键恢复内置**：删除或停用覆盖用的外部插件后，系统自动回退恢复为官方内置插件，零残留。
  4. **停用前保护**：停用已挂载/绑定的内置入口插件前，引导先执行还原（unwire）客户端配置；停用出口插件时保留已有账号数据但暂停其参与号池调度与探测。

---

## 4. 统一插件契约与通信协议

### 4.1 统一 `PluginManifest` v2 Schema（`<data_dir>/plugins/<plugin_id>/manifest.json`）

```jsonc
{
  "schemaVersion": 2,
  "id": "codex_sub",                     // 全局唯一，^[a-z0-9][a-z0-9_-]{0,62}$
  "name": "Codex 订阅",
  "version": "1.0.0",
  "kind": "egress",                      // "egress" | "ingress" | "hybrid"
  "transport": "subprocess",             // "builtin" | "declarative" | "subprocess"
  "platforms": ["macos", "windows"],     // 空数组表示全平台
  "brand": {
    "shortTagline": "OpenAI Codex 订阅",
    "icon": "assets/icon.png",           // 插件目录内相对路径或内置图标键
    "accentColor": "#10a37f"
  },
  "entry": {                             // 仅 transport == "subprocess" 必填
    "command": "bun",
    "args": ["dist/plugin.bundle.js"],
    "mode": "daemon",                    // "oneshot"（短进程）| "daemon"（空闲 60s 自动回收）
    "timeoutMs": 30000
  },
  "egressProfile": { /* 见 4.2 */ },
  "ingressProfile": { /* 见 4.3 */ }
}
```

### 4.2 `EgressProfile`（出口能力与约束契约）

```jsonc
{
  "wireProtocols": ["openai_responses"], // 上游支持的线协议（按优先级排序）
  "authMethods": ["local_import", "token_paste"],
  "accountCapabilities": {
    "supportsQuotaQuery": true,
    "supportsDailyClaim": false,
    "supportsClientSwitch": false,
    "claimRequiresClientSwitch": false,
    "supportsMultiInstance": false,
    "concurrentWithDesktopClient": true  // 取代 gateway/scheduler.rs 的硬编码豁免列表
  },
  "provides": {
    "streaming": true,
    "tools": true,
    "toolChoice": true,
    "vision": true,
    "thinking": true,
    "encryptedReasoning": true,          // 同源协议可透传加密推理块
    "jsonSchema": true,
    "countTokens": false,
    "compact": true                      // 如 /v1/responses/compact
  },
  "constraints": {
    "mustStream": true,                  // 非流式请求需由网关强制开流后聚合
    "requiresInstructions": true,        // 缺省 system 时由网关补默认 instructions
    "disallowStore": true,               // 强制注入 store: false
    "stripTemperatureOnReasoning": true, // 推理模型自动剥离 temperature
    "systemPromptPolicy": "standard"     // "standard" | "sanitize_agent_openers"（如 WorkBuddy）
  },
  "declarativeHttp": {                   // 仅 transport == "declarative" 生效
    "baseUrl": "https://api.example.com",
    "authScheme": "bearer",              // "bearer" | "x_api_key"
    "modelsPath": "/v1/models",
    "extraHeaders": {}
  },
  "defaultModels": []                    // 可选兜底模型目录与 vendor_wire_id 映射
}
```

### 4.3 `IngressProfile`（入口需求与声明式挂载契约）

```jsonc
{
  "wireProtocols": ["openai_chat"],      // 客户端发往 Qwitch 网关的协议
  "requires": {                          // 硬性依赖（缺一项即判 ❌ 不兼容）
    "streaming": true,
    "tools": true,
    "vision": false,
    "thinking": false
  },
  "optional": {                          // 增强特性（缺失时判 ⚠️ 降级可用并提示）
    "vision": true,
    "thinking": true,
    "toolChoice": true,
    "countTokens": false,
    "compact": false
  },
  "quirks": {
    "supportsCustomBaseUrl": true,       // false 表示该客户端本身无法接入网关
    "modelIdPolicy": "scoped",           // "scoped" | "anthropic_whitelist_alias"（Claude 伪装）
    "requiresWs426Fallback": false,      // Codex Desktop 专有
    "supportsOwnAuthPassthrough": false  // Codex Desktop 自有登录透传
  },
  "declarativeWiring": {                 // 仅 transport == "declarative" 生效（覆盖 80% 新 Agent）
    "detect": {
      "macosConfigPaths": ["~/.qoder/config.json"],
      "windowsConfigPaths": ["%USERPROFILE%\\.qoder\\config.json"],
      "macosProcessPatterns": ["/Applications/Qoder.app/"],
      "windowsExeNames": ["Qoder.exe"]
    },
    "patch": {
      "format": "json",                  // "json" | "jsonc" | "toml" | "yaml"
      "targetFileIndex": 0,
      "setKeys": {
        "providers.qwitch.baseURL": "{{baseUrl}}/v1",
        "providers.qwitch.apiKey": "{{gatewayToken}}",
        "providers.qwitch.models": "{{modelsMap}}"
      },
      "driftAssertContains": ["qwitch", ":{{port}}"]
    }
  }
}
```

### 4.4 升级版子进程 NDJSON 流式 RPC 协议

保留现有单次调用方法（`import_local`、`parse_manual_token`、`inspect_quota`、`claim_daily`、`apply_session`、`refresh_credential`），新增网关推理、模型发现与复杂入口方法：

| 方法 | 方向 | 说明 |
| --- | --- | --- |
| `discover_models` | Rust → Plugin | 传入 `{ credential }`，返回 `UpstreamModelOption[]`（含 `vendorWireId` 与可选模型级能力覆写） |
| `invoke_stream` | Rust → Plugin | 传入 `{ requestId, credential, protocol, wireModel, body, stream }`；插件按行流式回传：`{ event: "head", status, headers }` → 多个 `{ event: "chunk", dataBase64 }` → 可选 `{ event: "auth_refreshed", authPatch }` → `{ event: "done" }` 或 `{ event: "error", code, message }` |
| `abort` | Rust → Plugin | 传入 `{ requestId }`，客户端断开或首字节前超时立即中止上游请求，避免浪费额度 |
| `ingress_detect` / `ingress_wire` / `ingress_check_drift` / `ingress_unwire` | Rust → Plugin | 仅非声明式复杂 Ingress 插件（如双开隔离）实现 |

---

## 5. 入口 × 出口自动适配与兼容性判定引擎（`CompatibilityEngine`）

不维护 `N × M` 手工适配矩阵，而是由 Rust 内核 `src-tauri/src/plugins/compat.rs` 基于**「入口需求 × 内核 IR 桥接能力 × 出口供给与约束」**三跳自动推导。

### 5.1 核心数据结构

```rust
pub enum CompatibilityLevel {
    Compatible,   // 🟢 完全兼容（原生直通或无损翻译）
    Degraded,     // 🟡 降级可用（列出具体降级项与影响）
    Incompatible, // 🔴 不兼容（列出阻断原因与修复建议）
}

pub enum TransportMode {
    Passthrough,          // 同协议直通
    IncrementalTranslate, // 跨协议增量流式翻译
    BufferedTranslate,    // 跨协议全量缓冲翻译（如带 tools）
    Unsupported,          // 无可用桥接路径
}

pub enum ProbeStatus {
    Unverified,
    Verified,
    ProbeFailed,
}

pub struct CompatibilityIssue {
    pub severity: IssueSeverity, // Blocker | Warning | AutoShaped
    pub code: String,            // 如 "NO_PROTOCOL_BRIDGE", "MISSING_TOOLS", "BUFFERED_TOOL_STREAM"
    pub title: String,
    pub detail: String,
    pub suggestion: Option<String>,
}

pub struct CompatibilityReport {
    pub level: CompatibilityLevel,
    pub selected_ingress_protocol: Option<WireProtocolKind>,
    pub selected_egress_protocol: Option<WireProtocolKind>,
    pub transport_mode: TransportMode,
    pub auto_shapers: Vec<String>,
    pub issues: Vec<CompatibilityIssue>,
    pub probe_status: ProbeStatus,
}
```

### 5.2 内核 `BridgeMatrix`（协议转换能力表）

| 入站协议 \ 出站协议 | `openai_chat` | `anthropic_messages` | `openai_responses` |
| --- | --- | --- | --- |
| `openai_chat` | ✅ 直通透传 | ⚠️ 翻译（带 tools 需缓冲） | ❌ **当前缺失** → **Phase 1 补齐后变为 ⚠️/✅** |
| `anthropic_messages` | ⚠️ 翻译（带 tools 需缓冲） | ✅ 直通透传 | ❌ **当前缺失** → **Phase 1 补齐后变为 ⚠️/✅** |
| `openai_responses` | ⚠️ 翻译（带 tools 需缓冲） | ⚠️ 翻译（带 tools 需缓冲） | ✅ 直通透传 |

### 5.3 四步自动判定算法
1. **Step 1：协议桥接寻路**：在 `ingress.wireProtocols × egress.wireProtocols` 中寻找最优路径（`直通 > 跨协议增量流式 > 跨协议缓冲`）。若 `ingress.quirks.supportsCustomBaseUrl == false`，直接返回 `Incompatible(CLIENT_NO_CUSTOM_ENDPOINT)`；若无合法桥接路径，返回 `Incompatible(NO_PROTOCOL_BRIDGE)`。
2. **Step 2：硬性能力求交**：核对 `ingress.requires`（`streaming`、`tools`、`vision`、`thinking`）。若出口 `provides` 为 `false` 且内核无法模拟（如 `tools` 无法凭空模拟，而 `streaming: false` 可由内核聚合模拟），追加 `Blocker` → `Incompatible`。
3. **Step 3：软降级与约束自动整形（Auto-Shaping）**：
   - 跨协议带 `tools` 走缓冲 → `Warning(BUFFERED_TOOL_STREAM)`；
   - 跨协议推理 → `Warning(REASONING_SIGNATURE_STRIPPED)`；
   - `optional` 特性缺失（如 `vision`）→ `Warning(OPTIONAL_VISION_UNSUPPORTED)`；
   - 命中内核已实现的 `constraints`（`mustStream`、`requiresInstructions`、`disallowStore`、`stripTemperatureOnReasoning`、`anthropic_whitelist_alias`）→ 记录为 `AutoShaped`，不影响绿灯。
4. **Step 4：运行时能力探针覆写（Active Preflight Probe）**：
   - 针对 `(egress_plugin_id, model_id)` 运行 3 个极轻探针（总消耗 < 100 tokens），实测结果存入 `plugin_capability_probes`（TTL 7 天）：
     - **Probe A（基础流式）**：验证鉴权、Header、协议翻译与 SSE 帧完整性；
     - **Probe B（最小 Tool Call）**：携带 1 个极简 `ping` 函数声明，验证上游是否真实支持工具调用；
     - **Probe C（Agent System Prompt）**：验证上游是否触发通道风控拦截（如 WorkBuddy 特征）。
   - **实测为 `false` 时强制覆写静态声明中的 `provides: true`**。

### 5.4 端到端走查：`Codex 订阅（Egress） × Qoder（Ingress）`
1. **协议路径**：`qoder` 讲 `openai_chat`，`codex_sub` 讲 `openai_responses`。Phase 1 补齐 `IR → OpenaiResponses` 与 Responses SSE 反向翻译后，自动命中跨协议翻译路径（若未补齐则自动报红 `NO_PROTOCOL_BRIDGE`）。
2. **刚需校验**：`qoder` 要求 `tools: true` + `streaming: true`，`codex_sub` 均满足（Probe B 实测通过后标为 `Verified`）。
3. **自动整形**：内核根据 `codex_sub.constraints` 自动把 `system` 转为 `instructions`、强制 `store: false`、补齐 `stream: true`，并将 Responses SSE 转回 OpenAI Chat SSE。
4. **体验提示**：若当前 IR 尚未透传图片则给出 🟡 提示“图片上下文暂未透传”；补齐 `IrPart::Image` 后升为 🟢。全程无需任何手工适配表。

### 5.5 运行期调度器与 UI 联动
- **调度器过滤**：在 [gateway/scheduler.rs](../src-tauri/src/gateway/scheduler.rs) 中为 `select_candidates_for_request` 传入当前请求的 `RequestFeatures { has_tools, has_images, stream, incoming_protocol }`，自动跳过不满足本次请求硬性特征的成员并记录新增枚举 `RoutingAttemptOutcome::SkippedIncompatible`。
- **UI 提示规范**：
  - 号池挂载到 Agent 时，按模型展示 🟢 **完全兼容** / 🟡 **降级可用** / 🔴 **不兼容** 徽章；不兼容模型默认灰显不可选，Hover/点击展示具体原因与修复建议（高级/调试模式下允许强制勾选）。
  - 路由观测（Routing Trace）清晰展示 `SkippedIncompatible` 原因与自动整形记录。

---

## 6. 代码库解耦地图与 SQLite 增量 Schema

### 6.1 精确重构改造点

| 耦合点 | 当前位置与规模 | 重构方案 |
| --- | --- | --- |
| **`TargetAgentId` 枚举** | Rust 6 文件 **219 处**；TS 8 文件 **100 处** | 平滑替换为 `AgentId = String`（兼容保留原 6 个默认 ID，SQLite `agent_bindings.agent_id` 本身已是 `TEXT`，零库迁移破坏） |
| **`gateway/ir.rs` 出站不对称** | 缺 `to_upstream_openai_responses_body`、缺 Responses SSE 反向解析、缺 `IrPart::Image` | 补齐三协议全双向 IR 编码/流式转换、`IrPart::Image` 与 `EgressConstraints` 自动整形管线 |
| **`gateway/mod.rs` 巨石分发** | `resolve_upstream_auth` / `send_upstream_stream` / `call_upstream_member` 硬编码 `match tool_id` | 抽离 `EgressTransport` Trait；内置 7+1 个出口迁入 `src-tauri/src/plugins/egress/<id>.rs` |
| **`split_scoped_model_id`** | [domain/pools.rs](../src-tauri/src/domain/pools.rs#L501-L520) 写死 9 个前缀 | 改为查询 `PluginRegistry` 中已注册的 Egress 插件 ID 集合 |
| **`scheduler.rs` 活跃号豁免** | [gateway/scheduler.rs](../src-tauri/src/gateway/scheduler.rs#L111-L116) 硬编码 4 个工具 | 改为读取 `EgressProfile.accountCapabilities.concurrentWithDesktopClient` |
| **`platform/mod.rs` 路径探测** | 34 处硬编码 `tool_id` | 内置插件复用现有 `PlatformHost` 实现，外部插件通过 Manifest 声明受控路径规则并经白名单校验 |
| **`AppState.adapters`** | [commands/mod.rs](../src-tauri/src/commands/mod.rs#L120) 不可变 `Arc<HashMap>` | 改为 `Arc<RwLock<PluginRegistry>>`，网关与命令层共享同一可变注册表 |
| **前端品牌与图标** | `src/domain/toolRegistry.ts`、`src/components/AgentBrandIcon.tsx` | 改为由后端 `list_plugins_cmd` 下发 `PluginManifestDto`（含品牌色、tagline、图标与 fallback） |

### 6.2 新增 SQLite 表（向后兼容，不改动现有表结构）

```sql
-- 1. 插件生命周期与启停/覆盖状态表（覆盖内置默认插件与外部插件）
CREATE TABLE IF NOT EXISTS plugin_states (
    plugin_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,                          -- egress | ingress | hybrid
    source TEXT NOT NULL,                        -- builtin | external
    enabled INTEGER NOT NULL DEFAULT 1,
    pinned_hash TEXT,                            -- 外部插件目录 SHA-256
    overrides_builtin INTEGER NOT NULL DEFAULT 0,
    installed_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 2. 运行时能力探针缓存表（实测结果覆写静态声明，TTL 7 天）
CREATE TABLE IF NOT EXISTS plugin_capability_probes (
    egress_plugin_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    probe_json TEXT NOT NULL,                    -- 实测 streaming / tools / system_prompt 结果
    verified_at TEXT NOT NULL,
    error_summary TEXT,
    PRIMARY KEY (egress_plugin_id, model_id)
);
```

---

## 7. 分期实施路线图与验收标准

### Phase 1：IR 补齐 + 兼容性引擎 + Trait/Registry 地基
- **核心任务**：
  1. 新建 `src-tauri/src/plugins/`：`manifest.rs`、`registry.rs`（`RwLock<PluginRegistry>`）、`compat.rs`（`CompatibilityEngine` + `BridgeMatrix`）。
  2. 补齐 [gateway/ir.rs](../src-tauri/src/gateway/ir.rs)：`to_upstream_openai_responses_body`、Responses 上游 SSE → OpenAI Chat / Anthropic 反向流式翻译、`IrPart::Image` 与约束自动整形器。
  3. 将 `TargetAgentId` 在 Rust 与 TS 中平滑重构为 `String`；改造 `split_scoped_model_id` 与 `scheduler.rs`；新增 `plugin_states` 与 `plugin_capability_probes` 表。
- **验收命令**：
  - `cd src-tauri && cargo test && cargo clippy -- -D warnings`
  - `npx tsc --noEmit && npm test`
  - 新增单测覆盖 `CompatibilityEngine`（`codex_sub × qoder`、无 `tools` 拦截、Claude 伪装）及 `IR ↔ Responses` 双向转换。

### Phase 2：默认插件拆分（7 出口 + 6 入口全插件化）
- **核心任务**：
  1. 抽离 `EgressPlugin` Trait，将现有 7+1 个内置出口迁入 `src-tauri/src/plugins/egress/`，大幅瘦身 `gateway/mod.rs`。
  2. 抽离 `IngressPlugin` Trait，将现有 6 个内置入口迁入 `src-tauri/src/plugins/ingress/`，大幅瘦身 `agent_wiring/mod.rs`。
  3. 接入 `plugin_states` 实现默认插件逻辑停用/隐藏与同 ID 覆盖机制。
- **验收命令**：
  - 全量 Rust 单元测试、集成测试（`wiremock` 7 态矩阵）与前端测试 100% 回归通过。

### Phase 3：外部插件完整闭环（声明式引擎 + `.zip` 导入 + 流式子进程宿主）
- **核心任务**：
  1. 实现 `DeclarativeIngressPlugin`（JSON/JSONC/TOML/YAML 注入与 Stash/Drift）与 `DeclarativeEgressPlugin`（标准 HTTP/SSE 上游）。
  2. 实现 `.zip` / `.json` / 目录三轨安全导入器（含 6 条安全防线与旧 `adapters/` 目录平滑迁移）。
  3. 升级 [adapter/external.rs](../src-tauri/src/adapter/external.rs) 为 `SubprocessPluginHost`：支持 `discover_models`、`invoke_stream`、`abort`、`auth_refreshed` 回写与 `daemon` 空闲回收池。
  4. 提供参考外部插件示例（`examples/plugins/ingress-qoder/` 声明式入口 + `examples/plugins/egress-codex-sub/` 子进程出口）并完成端到端联调。
- **验收命令**：
  - Zip Slip / Zip Bomb / Symlink / 篡改哈希拦截测试全绿；`codex_sub → qoder` 端到端集成测试全绿。

### Phase 4：运行时探针 + 插件中心与兼容提示 UI
- **核心任务**：
  1. 新增 Tauri IPC 命令：`list_plugins_cmd`、`toggle_plugin_cmd`、`install_external_plugin_cmd`、`remove_external_plugin_cmd`、`check_pool_agent_compatibility_cmd`、`probe_egress_capabilities_cmd`。
  2. 在 [AgentBindingsView.tsx](../src/pages/dashboard/AgentBindingsView.tsx) 落地 🟢/🟡/🔴 兼容徽章、原因说明与一键探活；在路由观测中展示 `SkippedIncompatible`。
  3. 设置页新增「插件中心」（入口/出口分栏、默认插件停用/启用/恢复、`.zip`/目录/单文件导入、卸载、SHA-256 指纹展示）。
- **验收命令**：
  - `npx tsc --noEmit && npm test && npm run build` 与 `cd src-tauri && cargo test && cargo clippy -- -D warnings` 全绿。
