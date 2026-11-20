# Qwitch 内置适配器存储路径与接口参考规范

> 本文档归档首批接入工具（**Trae / Trae CN**、**WorkBuddy**、**Qoder / Qoder CN**）在 macOS 与 Windows 上的本地会话存储路径、凭据加密格式、服务端接口及冷切换要点。
>
> **实现状态（2026-11-20）**：当前注册 **4 个**国内版内置适配器：`trae_cn` / `workbuddy` / `qoder_cn` / **`minimax_code_cn`**（见 §2.4）。Trae/Qoder 国际版条目为格式参考归档，尚未注册实现。`trae` / `qoder` 等历史 ID 已被保留，禁止外部插件冒名。

---

## 1. 本地会话存储路径与加密格式

| 工具 | Windows 凭据路径 | macOS 凭据路径 | 凭据格式与加密机制 |
| --- | --- | --- | --- |
| **Trae** | `%APPDATA%\Trae\User\globalStorage\storage.json` | `~/Library/Application Support/Trae/User/globalStorage/storage.json` | Key 为 `iCubeAuthInfo://icube.cloudide`（[trae-local-api](https://github.com/ZedeX/trae-local-api/blob/main/CRACK_TUTORIAL.md)）。SG 国际版为明文 JSON（含 `token`、`refreshToken`、`expiredAt`、`userId`） |
| **Trae CN** | `%APPDATA%\Trae CN\User\globalStorage\storage.json` | `~/Library/Application Support/Trae CN/User/globalStorage/storage.json` | 同 Key `iCubeAuthInfo://icube.cloudide`，CN 版为 Base64 编码的 `"tc"` 自定义加密串（[trae-local-api](https://github.com/ZedeX/trae-local-api/blob/main/CRACK_TUTORIAL.md)） |
| **WorkBuddy** | `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\workbuddy-desktop.info` | `~/Library/Application Support/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info` | JSON 含 `account.uid` 与 `auth.accessToken`（[workbuddy-auto-signin](https://github.com/88lin/workbuddy-auto-signin/blob/main/signin.py)）。新版 `accessToken` 为 `$wbEncrypted: 1`（`sym-v1` AES-256-GCM 信封） |
| **Qoder CN** | `%APPDATA%\com.qodercn.app*\auth.v1.dat`（含同目录 `Local State`、`auth.machine-id`） | `~/Library/Application Support/com.qodercn.app*` | `auth.v1.dat` 为 Chromium `v10` + AES-256-GCM 密文；Windows 端使用 `Local State` 中的 DPAPI `os_crypt.encrypted_key` 解密（[qoder_signin.py](https://github.com/88lin/workbuddy-auto-signin/blob/main/qoder_signin.py)） |

---

## 2. 各工具接口与切换策略

### 2.1 Trae / Trae CN
* **认证请求头**：携带 `Authorization: Cloud-IDE-JWT <token>` 以及 `x-device-*` / `x-app-*`（[magpie #2907](https://github.com/yetone/magpie/issues/2907)）。Token 有效期约 14 天，支持 `ExchangeToken` 刷新（[trae-local-api](https://github.com/ZedeX/trae-local-api/blob/main/CRACK_TUTORIAL.md)）。
* **冷切换策略**：关闭客户端后，将目标账号对应的 `storage.json`（及 `state.vscdb` 认证键）快照写入本地配置目录并重启客户端。
* **后续多开预留**：VS Code 架构支持通过 `--user-data-dir` 配合 `--extensions-dir` 实现多实例并行（[Trae-Work-CN-Account-Manager](https://github.com/xhrxgr/Trae-Work-CN-Account-Manager)）。

### 2.2 WorkBuddy
* **默认服务端点**：`https://copilot.tencent.com`（[signin.py](https://github.com/88lin/workbuddy-auto-signin/blob/main/signin.py)）。
* **必带请求头**：`Authorization: Bearer <accessToken>`、`X-User-Id: <uid>`、`User-Agent: WorkBuddy`（企业版附带 `X-Enterprise-Id`、`X-Tenant-Id`）。
* **核心接口**：
  * 查询签到状态：`POST /v2/billing/meter/checkin-activity-status`（检查 `today_checked_in`、`streak_days`、`total_credits`）。
  * 手动每日签到：`POST /v2/billing/meter/daily-checkin`（成功返回 `{"credit": 100}`；已签幂等返回 `null` 或 `HTTP 400 + code 10001`）。
* **工程注意事项**：
  * WorkBuddy 客户端刷新 Token 时可能短暂独占 `workbuddy-desktop.info`，读取需做短暂重试（如 3 次）。
  * `sym-v1` 加密信封绑定客户端原生模块 `electron_browser_workbuddy_storage`，冷切换直接原样备份/写回 `workbuddy-desktop.info` 字节即可；如需调用签到接口解密 `sym-v1`，可通过 `ELECTRON_RUN_AS_NODE=1` 调用本机客户端运行时解密。

### 2.3 Qoder / Qoder CN
* **默认服务端点**：`https://openapi.qoder.com.cn`，活动按 UTC+8 每天 10:00 刷新（[qoder_signin.py](https://github.com/88lin/workbuddy-auto-signin/blob/main/qoder_signin.py)）。
* **必带请求头**：`Authorization: Bearer <token>`、`Accept: application/json`、`User-Agent: Qoder/claim`、`Cosy-ClientType: 10`。
* **防风控设备头（可选增强）**：`Cosy-MachineId`（来自 `auth.machine-id`）、`Cosy-MachineOS`、`Cosy-MachineHostname`、`Cosy-Version`，以及 Windows 客户端自带 `resources/umid/runtime-info.exe --account-stdin` 输出的 `Cosy-MachineToken` 等字段。
* **核心接口**：
  * 查询活动列表：`GET /sash/api/v1/me/campaigns`（筛选 `actionType == "CLAIM_BENEFIT"` 且处于有效窗口的 `CLAIMABLE` / `CLAIMED` 项）。
  * 手动领取 Credits：`POST /sash/api/v1/me/campaigns/{campaignId}/claim`（幂等，成功返回 `status == "CLAIMED"`）。
  * **请求数说明**：Qoder 领取主路径需重新拉取活动列表（前置快照不携带 `campaignId`，无法直接领取），领取成功后再查一次额度以刷新权威状态；这两次额外请求为已知接受的设计约束。领取成功但领后查额失败/返回 Unknown 时，合成 Claimed 快照避免 UI 残留可领按钮。

---

## 2.4 MiniMax Code CN（`minimax_code_cn`）

* **服务端点**：`https://agent.minimax.cn`（业务接口）、`https://account.minimax.cn`（OAuth）。
* **凭据形态**：客户端 `auth.json` 中为**明文** `records[].accessToken`（无本地加密层），
  因此本适配器是四个内置适配器中唯一**不需要外部运行时解密**的。
* **凭据键派生**：`credential_record_key` 复刻客户端 FileStore 键
  `service\0base64url(sha256(<home>/.minimax/auth\0mcode-public))`。
* **授权方式**：自建 OAuth 2.0 **设备码（Device Code）+ PKCE** 流程，
  端点 `https://account.minimax.cn/oauth2/device/code` 与 `/oauth2/token`。
* **核心接口**（均带 `?timezone_offset=` 参数）：
  * 查询积分：`/minimax-cloud/api/v1/credit/details`
  * 签到状态：`/minimax-cloud/api/v1/signin/status`
  * 每日签到：`/minimax-cloud/api/v1/signin/claim`
* **令牌刷新**：唯一实现 `refresh_credential` 的内置适配器（`accessToken` 有效期约 1 小时）。
* **已知约束**：
  * 设备码轮询的 `expires_in` 未设上限（仅 `interval` 做了 clamp），
    服务端若返回超长有效期会导致轮询长时间空转；
  * `auth-state.json` 在导入时可选、切号时必填，缺失会导致切号失败；
  * 不产出 `DeviceDailyLimitReached`，调度器的设备级短路对其不生效。
