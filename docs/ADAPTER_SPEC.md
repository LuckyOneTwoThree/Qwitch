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

---

## 2.5 豆包工作（`doubaowork`）

* **服务端点**：`https://www.doubao.com`（业务接口）、`https://accounts.doubao.com`（登录校验）。
* **授权方式**：内置 WebView 登录后轮询 Cookie（`sessionid` / `sessionid_ss` / `sid_tt` 任一存在即可），
  另支持本机快照导入（`doubaowork_profile.zip`）与 Cookie 粘贴。
* **客户端对齐基线**：`DoubaoWork.app` 2.37.19。公共查询串与客户端
  `apps/entry-main/main.js` 的 `UniParamsInit` 保持一致：
  `device_platform=desktop`、`doubao_device_platform=desktop`、`web_platform=desktop`、
  `runtime=web` + `runtime_version=3.46.7`，以及 `device_id` / `iid` / `web_id` / `tea_uuid` / `web_tab_id`。
* **核心接口**：
  * 额度汇总：`POST /alice/commerce/sale/subscription/quota/summary/`
  * 订阅列表（判定活动试用天数）：`POST /alice/commerce/sale/subscription/list/`
  * 签到状态（权威源）：`POST /alice/ug/campaign/v3/detail/`，
    读 `tasks[campaign_code=daily_signin].daily_checkin.checked_in_today`
  * 投放卡片（辅助判断可领）：`POST /alice/ug/campaign/landing/get`
  * 每日签到写接口：`POST /alice/ug/campaign/landing/checkin`
    （官方客户端侧边栏签到按钮 `s2-ug-sidebar-popup-host` 走的就是它，
    `interactionType = CheckIn`）

### 活动试用天数上限（`710010702` 的真实成因）

活动规则第六条：**单用户通过本活动可获得的专业版试用总天数上限为 20 天**。

当账号已持有活动赠送的套餐（`subscription/list` 中
`is_gift = true`、`status = 3`、`campaign_infos[].display_duration_days > 20`，
典型来源 `subscription_source = "pc_membership_gift"`）时，
服务端的发放校验会抛异常并以 AGW 兜底码 `710010702 系统错误` 返回。

该现象的特征：

* `landing/checkin` 对**任意** `scene`（含伪造值）都返回 `710010702` —— 校验发生在参数校验之前；
* 官方客户端手动点击签到同样失败，前端日志
  `sidebar_card_popup_check_in_failed` + `outcome: "toast_error"`；
* 换一个未超限的账号即可正常签到；
* 无 Cookie 时先返回 `710012001`（鉴权层正常），说明问题在业务层。

因此适配器在 `inspect_quota` 阶段就读取订阅数据，超限直接把 `claim_status`
置为 `Unsupported`，UI 不再显示可领按钮；`claim_daily` 命中超限时不再发起写请求，
直接返回可解释的原因文案。

### 错误码语义

| 业务码 | 归类 | 说明 |
|---|---|---|
| `710012001` / `710012006` / `710012020` | `AuthExpired` | 登录态失效 |
| `710022002` | `RiskTriggered` | 安全风控 |
| `710012007` | `RateLimited` | 限流 |
| `710010702` / `710010202` / `710010101` / `625002002` | `UnsupportedCapability` | AGW 兜底码，**不是**网络错误 |

`UnsupportedCapability` 不参与 `CONSECUTIVE_NETWORK_ERROR_BREAKER` 计数，
因此不会因为单个账号不可领而熔断整个 `doubaowork` 工具、连带打断额度查询。

### 已知约束

* `POST /alice/ug/campaign/item/reward/claim/`（客户端 `ClaimCampaignReward`）
  对当前账号返回恒定的 `biz_error.code = 3`（占位响应，连伪造 item/group 也一样），
  官方客户端并不调用它，故**不作为领取路径**。
* `POST /alice/ug/event/report` 会返回 `625002003 暂无活动资格`，可作为资格旁证。
* 活动为定向开放，账号未被投放签到任务时 `v3/detail` 无 `daily_signin` 任务，
  适配器据此返回 `Unsupported`，不发起写请求。

---

## 2.6 码道 CodeArts（`codearts`）—— 额度监控型

> **与其它适配器的本质差异**：CodeArts **没有「每日签到」机制**。
> 对 `app.asar` 主进程代码全文检索 `每日` / `签到` / `打卡` / `daily` 均为 0 命中，
> 仅存在一次性权益领取 `BenefitService.claimBenefit`。因此本适配器
> `supports_daily_claim = false`，`claim_daily` 不做任何网络请求。

* **客户端**：`com.codearts.work` v0.2.3（Electron）。
* **凭据位置**：`~/Library/Application Support/codearts/secretStorage/<sha256(key)>.enc`，
  key 为 `authSession:cn-north-4`。
* **凭据加密**：Electron `safeStorage`，macOS 上等价于 Chromium `v10` 信封：
  AES-128-CBC，密钥 = `PBKDF2-HMAC-SHA1(Keychain密码, "saltysalt", 1003, 16)`，
  IV 为 16 个空格。Keychain 条目为 `codearts Safe Storage` / 账号 `codearts`。
  适配器直接用 `security` 命令取密码后解密，**不依赖客户端运行时**。
* **凭据结构**：`{ session, loginContext }`，`session` 含华为云临时凭据
  `accessKey` / `secretKey` / `securityToken` / `refreshToken` / `expiresAt`；
  `loginContext` 含 `pkcePair` 与 `dpopKeyPair`（refresh 时需要）。
* **认证方式**：不是 Bearer Token，而是华为云 **SDK-HMAC-SHA256** 请求签名：

  ```text
  CanonicalRequest = METHOD\nCanonicalURI\nCanonicalQuery\nCanonicalHeaders\nSignedHeaders\nSHA256("")
  StringToSign     = "SDK-HMAC-SHA256"\nX-Sdk-Date\nSHA256(CanonicalRequest)
  Authorization    = SDK-HMAC-SHA256 Access=<ak>, SignedHeaders=<h>, Signature=<hmac>
  ```

  必带头：`X-Security-Token`、`X-Sdk-Date`、`host`。
  签名错误时服务端返回 `401 APIG.0301 verify ak sk signature fail`，
  适配器据此映射为 `AuthExpired`（而非网络错误）。

* **核心接口**：
  * 额度概览：`GET {snapEngineUrl}/snap-manager/v1/package/overview`
  * 身份校验：`GET {sts}/v5/caller-identity`
  * 积分用量：`GET {snapEngineUrl}/v1/ops/credit/usage?type=&id=`
  * 企业角色：`GET {snapEngineUrl}/snap-manager/v1/member/roles`
  * 一次性权益（**不实现**）：`POST {benefitApiUrl}/api/v1/benefit/claim?channel=`
* **默认端点**：
  `snapEngineUrl = https://snap-access.cn-north-4.myhuaweicloud.com`，
  `iamSts = https://sts.cn-north-4.myhuaweicloud.com`，
  `benefitApiUrl = https://opengw.developer.huaweicloud.com`。

* **额度解析**：`package/overview` 拆成三个桶
  `all_credit_package`（总积分）/ `basic_package`（基础套餐）/ `bonus_credit_package`（赠送），
  字段为 `package_credit_amount` / `package_credit_used` / `package_credit_remain`。

* **已知约束**：
  * `claim_status` 恒为 `Unsupported`，UI 不显示可领按钮，调度器跳过领取。
  * **不支持本地切号**：`apply_session` 返回 `UnsupportedCapability`。
    原因是回写凭据需要生成合法的 `safeStorage` 密文（依赖 Electron 运行时），
    本进程无法产出；仅提供 `backup_session` 备份原文件。
  * **凭据过期：Qwitch 只检测，不代刷。**
    AK/SK 为华为云 STS 临时凭据，`tokenRefreshInterval` 为 1 小时，
    且 **refresh token 是一次性轮换的**（消费后必须立刻持有新 token，
    否则原 token 立即作废）。若 Qwitch 与客户端都尝试刷新，必然有一方
    把对方的 token 废掉，结果是用户被迫重新登录。

    官方客户端在**剩余 1 小时**时就会自行刷新（`startRefreshTimer` 里
    `expiresAt - TOKEN_REFRESH_INTERVAL` 加随机抖动），因此 Qwitch 采取
    「只读监控 + 引导用户」策略：

    - `ensure_not_expired` 仅比对凭据自带的 `expiresAt`，**不发任何网络请求**
    - 剩余 ≤ 5 分钟（`EXPIRY_SKEW_MS`）即判 `AuthExpired`
    - 错误文案直接给出行动指引：打开客户端 → 客户端自动刷新 → 回来重新导入
    - UI 在 `expired` 状态下为 `codearts` 提供「打开客户端」按钮
      （`launch_tool_client` 命令，可执行文件经与切号同一套白名单校验）
    - `claim_daily` **不**做过期检查：它是 no-op，若在此报 `AuthExpired`
      会让账号被误标为过期、连带打断额度监控

    恢复流程（用户视角两次点击）：
    凭据过期 → 点「打开客户端」→ 客户端自动刷新 → 回 Qwitch 重新导入。
  * 实测账号在 `benefit/claim` 上返回
    `error_code=4000 invalid channel, group not found`（不在任何权益组），
    与额度查询无关，不影响本适配器。
