# Qwitch API 资产架构设计与迭代基线

> **文档状态**：设计基线（一期：多厂商 API 资产 + 同厂商多 Key 池 + Agent 接入联动）  
> **适用版本**：`v0.9.3` 之后（`v0.10.0` 迭代）  
> **关联文档**：[架构与基线设计](ARCHITECTURE.md) · [双端插件架构设计](PLUGIN_ARCHITECTURE_DESIGN.md) · [号池网关架构设计](POOL_GATEWAY_DESIGN.md)

---

## 1. 背景与产品定位

此前版本中的 `custom_api_key`（「标准 API」）仅预留了末位兜底与网关转发骨架，存在四个结构性短板：
1. 界面中缺少录入入口，用户无法创建/管理通用 API Key；
2. 所有第三方 API 被强行塞进单一 `tool_id = "custom_api_key"` 命名空间，不同厂商的 Base URL、协议与模型目录互相冲突；
3. 被硬编码锁死为 `T3Fallback`，无法作为 T1 主力（例如包月 Coding Plan、官方按量 Key 或中转站）参与调度；
4. 上游协议依赖模型名猜测且 URL 拼接与模型探测不一致。

本次重构将「账号资产」明确拆分为两类心智完全独立的资产：
- **订阅账号**（原「账号资产」）：绑定本地桌面端登录态（7 个内置桌面客户端适配器），核心关注额度水位、每日签到、事务切号与凭据保活。
- **API 资产**（新分类）：厂商 API Key / Coding Plan / 中转站 / 本地模型服务（Ollama 等），核心关注 Base URL、显式协议、连通性测试、模型目录与同厂商多 Key 高可用池。

---

## 2. 核心领域模型与 ID 规范

### 2.1 厂商 ID 规范（`api_<slug>`）
- 厂商 ID 统一采用 `api_<slug>`（例如 `api_deepseek`、`api_openrouter`、`api_zhipu_coding`、`api_custom`）。
- 仅包含 `[a-z0-9_]`，总长度 ≤ 48，**不使用冒号**，完全满足 [PLUGIN_ARCHITECTURE_DESIGN.md](PLUGIN_ARCHITECTURE_DESIGN.md#L111) 的插件 ID 正则 `^[a-z0-9][a-z0-9_-]{0,62}$` 以及网关 `scoped_model_id`（`api_deepseek/deepseek-chat`）与 TOML/JSON 键名约定。

### 2.2 厂商实体（`api_providers` 表）
```sql
CREATE TABLE IF NOT EXISTS api_providers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    preset_id TEXT,
    base_url TEXT NOT NULL,
    wire_protocol TEXT NOT NULL DEFAULT 'openai_chat',
    auth_scheme TEXT NOT NULL DEFAULT 'bearer',
    auth_header_name TEXT,
    extra_headers_json TEXT NOT NULL DEFAULT '{}',
    models_path TEXT,
    manual_models_json TEXT NOT NULL DEFAULT '[]',
    allow_lan_http INTEGER NOT NULL DEFAULT 0,
    accent_color TEXT,
    icon TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_test_status TEXT,
    last_test_latency_ms INTEGER,
    last_test_error TEXT,
    last_tested_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 2.3 API Key 凭据（复用 `accounts` 表）
- 每个厂商可添加 **1 把或多把 API Key**，以 `tool_id = provider.id` 存入 `accounts` 表：
  - `uid_hash = HMAC(master_key, tool_id, api_key)`：同厂商下防止重复导入同一把 Key；
  - `uid_masked = mask_api_key(api_key)`：前端仅展示脱敏前后缀（如 `sk-ab•••89ef`）；
  - `encrypted_credential`：AES-256-GCM + AAD（绑定 `account_id`）加密存储 `CredentialPayload::Token`，`extra` 中同步保存厂商元信息（`base_url`、`wire_protocol`、`auth_scheme`、`auth_header_name`、`allow_lan_http`）；
  - `has_local_snapshot = 0`、`has_api_token = 1`、`renewal_mode = "manual"`。
- 修改厂商的 `base_url` / 协议 / 鉴权方式时，自动同步更新其下属所有 `accounts` 凭据的 `extra` 元数据。

---

## 3. 协议、URL 拼装与安全规范

### 3.1 显式协议与鉴权声明
- 每个厂商显式声明 `wire_protocol`（`openai_chat` | `anthropic_messages` | `openai_responses`）与 `auth_scheme`（`bearer` | `x_api_key` | `custom_header`）。
- 网关调度与出站分发直接读取厂商声明，彻底取消根据模型名猜 Anthropic 协议的脆弱逻辑。

### 3.2 智能 URL 拼装（`build_upstream_endpoint`）
- **严格路径模式**：若 `base_url` 以 `#` 结尾，去掉末尾 `#` 后原样作为最终请求 URL（适配非标中转站）。
- **版本段感知拼装**：去掉末尾 `/` 后检查路径段是否已包含 `v<数字>`（例如 `/v1`、`/api/v3`、`/api/paas/v4`）：
  - `openai_chat`：已有版本段拼 `/chat/completions`，否则拼 `/v1/chat/completions`；
  - `anthropic_messages`：已有版本段拼 `/messages`，否则拼 `/v1/messages`；
  - `openai_responses`：已有版本段拼 `/responses`，否则拼 `/v1/responses`。

### 3.3 网络安全边界
- 默认仅允许 `https://` 与本机回环 `http://127.0.0.1` / `http://localhost` / `http://[::1]`（支持本地 Ollama / LM Studio）。
- 如需接入局域网自建网关（如 `http://192.168.x.x`、`http://10.x.x.x`、`http://*.local`），需在该厂商显式勾选 `allow_lan_http = true`，不全局放开。

---

## 4. 与插件底座、号池及网关的衔接

1. **热同步到 `PluginRegistry`**：应用启动及每次增删改启停 `api_providers` 时，自动将厂商同步为 `PluginRegistry` 中的声明式 `Egress` 记录与 `StandardHttpEgress` 实例（跳过外部脚本校验），立即参与兼容性引擎（🟢/🟡/🔴）评估与网关路由。
2. **T1 平权与同厂商多 Key 池**：解除此前对 API Key 的强制 `T3Fallback` 锁定与 `fallback_api_key_enabled` 限制。同厂商多把 Key 默认为 `T1Primary`，复用现有号池策略（主号优先 / 轮询 / 429 自动冷却 / 会话亲和）。
3. **后台保活与统计隔离**：`TokenKeepaliveWorker`、顶部「一键领取 / 批量查额」与「待完善」KPI 自动排除 `is_api_provider_tool_id` 账号。
4. **存量平滑迁移**：启动时若检测到 `accounts` 表存在历史 `tool_id = "custom_api_key"` 记录，自动迁移至 `api_custom`（「自定义 API」）；若无存量则彻底移除空壳「标准 API」。

---

## 5. Tauri IPC 命令契约（Phase 2）

- `list_api_providers_cmd`：返回全部厂商配置与关联状态。
- `save_api_provider_cmd`：新增或更新厂商（支持同时录入首把 Key），同步 `PluginRegistry` 并广播工作区更新。
- `delete_api_provider_cmd`：删除厂商并级联清理其关联 Key 与模型缓存。
- `toggle_api_provider_cmd`：启停厂商并热刷新 `PluginRegistry`。
- `test_api_provider_cmd`：测试连通性与延迟（ms），尝试拉取 `/models` 并可选落库测试状态。
- `add_api_provider_key_cmd`：为指定厂商追加 API Key（自动 HMAC 去重）。
- `update_api_provider_key_cmd`：更新 Key 别名或替换密钥。
- `save_api_provider_models_cmd`：保存手动维护的模型目录与别名映射。

---

## 6. 分期路线图

- **一期（当前）**：多厂商 API 资产管理、12 组内置预设、连通测试、手动+自动模型目录、同厂商多 Key 号池、接入 6 大 Agent 与三协议流式转发。
- **二期（后续扩展）**：跨厂商聚合号池（支持跨厂商/订阅+API 混编与模型名映射）、高频厂商（如 DeepSeek / OpenRouter）标准余额查询。
