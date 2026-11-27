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

### 额度解析：绝对额度优先，百分比只作附注

`quota/summary` 的 `window_limit_section` 在套餐开通瞬间只回 `used_percent`（且常数 0），
此时 `total` 只能按 100 折算 —— 这正是历史上「豆包明明有额度却显示剩余 100%」的来源。
现行规则：

* 只有窗口段回百分比时，仍然用顶层 `quota_blance_usage` 的绝对额度补桶（旧实现只在
  「完全没有桶」时才补，于是活活把绝对额度挡掉）；
* 绝对值桶排在主读数位，百分比桶带 `hasTotal = false`，前端只展示百分比、不画进度条；
* 解析出「全是百分比」时，`inspect_quota` 会等 1.2s 重取一次（套餐数据异步下发，
  正常响应零额外开销）。

### 签到绑定「设备 + 桌面端当前登录账号」

`landing/get` 只反映卡片投放，是账号维度的；`landing/checkin` 的发放校验则要求
该账号是**本机桌面端当前登录的账号**（实测：网页登录导入的第二个账号 landing 显示
「今日签到领 200 额度」，写接口恒定返回 `710010702 系统错误`；同一个 Cookie 换成
桌面端在登账号即返回 `checked_in: true`）。

因此：

* `claim_daily` 会先比对 `detect_local_active_uid`，不一致直接返回可执行提示
  「先把该账号切至桌面端再领取」，不再打写接口；
* 前端对「非本机在登」的豆包账号禁用「领取」按钮并给出同样的提示。

### 切号：只接受整份 profile 快照（不做 Cookie 直写）

曾经尝试「只把 passport Cookie 加密写进 `Default/Cookies`」来绕过「先在客户端登录一次」，
**结论是不成立**（2026-11-26 实测）：

* 写入本身是可复现的：Keychain `DoubaoWork Safe Storage` → PBKDF2 → AES-128-CBC，
  明文为 `SHA256(host_key)`(32B) + cookie 值，前缀 `v10`。写入后用 openssl 独立解密，
  与源值 SHA-256 完全一致；直接用这份 Cookie 调 `quota/summary` 也能成功。
* 但客户端**不会**因此进入登录态：拉起客户端后 `Preferences.saman.last_user` 始终为空，
  随后客户端把自己的登录态写成「已登出」（`user_id` / `sec_user_id` 空串、`credit=0`）。
  再次用这批 Cookie 打 passport 校验，返回 `AuthExpired`。
* 原因：我们用的是 `DELETE` + `INSERT`，把客户端原有的**分区 Cookie**（`top_frame_site_key`
  非空 / `has_cross_site_ancestor=1`，CHIPS）替换成了非分区副本；客户端按分区读取，
  自然读不到登录会话。Cookie 值对「直连 HTTP」有效，对「客户端自身」无效——这两件事
  此前被混为一谈，属于误判。

因此 `apply_session` 只接受 `LocalSnapshot`：切号 = 写回整份快照（含 Preferences、
Cookies、分区属性、sdk_storage）。要新增账号，必须在该账号的本机客户端登录过一次。

### 多账号：客户端内多账号 = 多 Chromium profile

豆包工作客户端**自带多账号**：每加一个账号就多一个 Chromium profile 目录
（`Default`、`Profile 2`、…），当前账号由 `Local State` 的
`profile.last_used` 指定。早期实现把路径写死成 `Default/...`，于是用户在客户端
切到第二个账号后，Qwitch 仍然读第一个账号的 `Preferences`，
「关联本机」就会报「本机当前登录的是另一个账号」——**这是 bug，不是用户操作错误**。

现在的规则：

* `active_profile_name(root)`：读 `Local State.profile.last_used`
  （再退 `last_active_profiles[0]`、再退 `Default`、最后退任意存在的 profile）；
  带 `Preferences` 校验，避免指到空目录。
* 所有路径统一以「profile 目录」为基准：`<profile>/Preferences`、
  `<profile>/Cookies`、`<profile>/Network/Cookies`、`<profile>/Local Storage/leveldb`、
  `<profile>/saman_shell_persistent_storage_v1`；`sdk_storage/**` 与 `aha/users/**`
  是设备级共享，仍在根目录。
* 导入时把 profile 名写进快照（`profile_dir.txt`）；切号写回后调
  `set_active_profile()` 把 `Local State.profile.last_used` 指向它——
  否则客户端重启后还是打开原来那个账号。
* `backup_session` 覆盖**所有** profile + `Local State`，回滚才能回到切号前的账号；
  老快照没有 `profile_dir.txt`，按 `Default` 处理（向后兼容）。

### 关联本机：身份来源与登出判定

`import_from_local` 的身份来源按优先级：

1. 当前 profile 的 `Preferences` 里 `saman.last_user`（客户端自报；`user_id` 为空串视为**没有**记录）；
2. 否则读客户端 Cookies 库（safeStorage 解密，自动剥 32B 域名哈希）拼成 Cookie 头，
   打一次 `passport/account/info/v2` 拿真实 uid；
3. 两者都拿不到 → 返回可执行提示「客户端 Preferences 没有登录记录，Cookies 库里也没有
   可用会话；请先在客户端登录一次再关联本机」。

成功时会把这份在线 Cookie 一并写进快照（`decrypted_token.txt`），关联完立刻能查额/签到。

另：客户端登出后 `last_user` 会被清空，而 `detect_local_active_uid` 返回 `None` 同时代表
「没登录」和「本机没数据」，导致「本机使用中」标记永远清不掉（卡片既没有「关联本机」
也没有「切至桌面端」）。新增 `ToolAdapter::detect_local_logged_out`（默认 `None`，
豆包实现为「Preferences 无 last_user 即明确登出」），校准任务据此清空该工具的在登标记。

### 本地网关推理通道

* 已通过 `src-tauri/src/gateway/doubao.rs` 接入原生 HTTP/SSE 网关上游：自动完成会话初始化，支持流式 SSE 与非流式响应转换，并在响应结束后异步清理临时会话。
* 模型目录支持直接从本机插件/客户端缓存目录读取真实模型清单。

---

## 2.6 码道 CodeArts（`codearts`）

> **每日签到机制**：CodeArts 通过活动交付与领取接口提供 `USER_LOGIN` 每日签到
> （`supports_daily_claim = true`，走 `/v1/ops/*` 纯服务端签名接口，无需冷切桌面端）。

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

* **已知约束与本机切号**：
  * **本机登录态与登出检测**：实现 `detect_local_active_uid`（按 `.enc` 文件路径 + `mtime` + 大小缓存解密结果，避免高频调用 `security`）与 `detect_local_logged_out`（`.enc` 缺失即判定已登出）。
  * **本地切号（仅 macOS）**：`supports_client_switch = cfg!(target_os = "macos")`。
    `apply_session` 用同一套 safeStorage 参数**加密**把 `{session, loginContext}` 重新写回 `secretStorage/<sha256(key)>.enc`：
    * 切号覆盖前先无网络只读回吸即将被切走账号的最新本机快照（保存客户端轮换后的新 `refreshToken` 与 `expiresAt`）；
    * 写回时仅在库中凭据比快照更新时才覆写 `accessKey`/`secretKey`/`securityToken`/`expiresAt`（非 `isolated_oauth` 同步更新 `refreshToken`），并更新 `updatedAt`；若 `expiresAt` 已过期但快照携带 `refreshToken`，允许写回由码道启动自动续期；
    * 前提是「本机导入」时留下的完整会话快照（`extra.session_snapshot_b64`）；独立 OAuth 授权的账号没有 `loginContext`，`apply_session` 返回 `UnsupportedCapability`。Windows 因 DPAPI 限制暂不支持。
  * **凭据过期与软过期容错**：
    AK/SK 为华为云 STS 临时凭据（`refreshToken` 一次性轮换），客户端在临近过期时自行刷新。`ensure_not_expired` 仅在超过硬过期容差时拦截；`isolated_oauth` 账号支持网关内单飞 DPoP 刷新（且后台本机回吸 `sync_active_local_credential_if_fresher` 会跳过 `isolated_oauth` 账号，防止覆盖独立 OAuth 令牌）。
  * 实测账号在 `benefit/claim` 上返回
    `error_code=4000 invalid channel, group not found`（不在任何权益组），
    与额度查询无关，不影响本适配器。

### 本地网关推理通道

* 已通过华为云 `SDK-HMAC-SHA256` 签名上游接入 `/api/v2/chat/completions`（支持 `.myhuaweicloud.cn` 域名回退）。
* 遇到 `APIG.0602` 令牌过期时：`isolated_oauth` 账号走单飞 DPoP 刷新；`client_readonly` 账号绝不消耗客户端一次性 `refreshToken`，仅尝试从本机磁盘只读重同步最新 STS。
* 遇到 `TM.00001041` 并发限流时触发短冷却，交由号池自动切换候选账号。
