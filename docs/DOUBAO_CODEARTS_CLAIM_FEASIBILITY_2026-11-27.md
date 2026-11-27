# 豆包工作「一键多账号领取」与 码道 CodeArts「减少频繁关联本机 + 一键领取」深度可行性报告

> 日期：2026-11-27　范围：Qwitch 本地代码复核 + magpie 参考实现对照
> 结论先行：
> - **豆包**：服务端签到写接口把「设备 + 桌面端当前登录账号」绑死。所谓「一键」在工程上必然等于「逐个冷切号 → 立即领取 → 下一个」，本质是 `switch_desktop_account` 的串行批处理封装，**不是**无副作用的纯网络批量。技术上完全可做，成本在于每个账号都必须持有完整本机快照、且切换要杀掉/重启客户端。
> - **码道（已更正）**：CodeArts **确实有每日签到**！用户截图「活动福利」面板里的「每日签到领100积分 / 每日领取」就是每日签到。**早期本仓库文档「app.asar 全文 0 命中」的结论是错的**——重新逆向 `app.asar` 证实存在完整福利体系：`GET /v1/ops/delivery`（拉活动）→ `POST /v1/ops/claim`（领取）→ `POST /v1/ops/confirm`（确认），活动类型含 `daily_claim`（每日签到）/`student_certify`（学生认证）/`invite`（邀请）。**关键利好**：这些接口的签名算法与本适配器 `sign_headers_with_body` 完全一致、域名就是现有的 `snap-access`（`policyCenterDomain`），实现成本极低。「一键领取」可以做真签到；「频繁关联本机」根因是 STS 短时轮换 + 只读回吸 + 切号需完整 `session_snapshot_b64`，可大幅降低但做不到字面『永不关联』。

> ## ✅ 真机实测更正（2026-11-27 12:28，真实 STS 抓包）
>
> 用真实账号（`GT-HeQiLucky` / `c02b6539…a38f7`）跑通 `claim_daily` 全链路后，实测 delivery 响应**推翻了部分字段假设**，已按真机修正代码：
>
> 1. **每日签到的 `type` 不是 `daily_claim`，而是 `USER_LOGIN`**（`extra.triggerEvent = "user.login"`，标题「每日签到领100积分」，`pageUrl /v1/ops/pages/34`）。早期从 asar 推断的 `daily_claim` 是**误读**，代码已改为 `is_daily_checkin()` 同时识别 `USER_LOGIN`/`daily_claim`/`daily_signin` 并以 `extra.triggerEvent=user.login` 兜底。
> 2. **状态字段是 `status`，取值 `ELIGIBLE`（可领）/`CLAIMED`/`CONFIRMED`/`CONSUMED`**；`claimable:true` + `pendingCount:1` + `pendingTotalAmount:100` 表示今日待领。已领判定 `status ∈ {CLAIMED,CONFIRMED,CONSUMED}`。
> 3. **`campaignId` 是数字**（每日签到=34、学生认证=2/39），非字符串；代码 `activity_campaign_id()` 已兼容数字→字符串化后放入领取请求体。
> 4. **学生认证 `type=STUDENT_CERTIFIED`**（`claimable:false`，需先去官网 `account.huaweicloud.com/.../studentAuth` 认证，非本适配器可自动领）。
> 5. **真实领取已验证成功**：`claim_daily` → `claimed=true，领取 100 积分`；赠送积分 4085.43→4185.43（+100），总积分 4585.43→4685.43。**第二次幂等**：`claim_status=Claimed`、`claim_daily` 返回「今日已领取」，不重复发放。
> 6. **签名/头全部被服务端接受**：`Agent-Type: PromptCenter`、`X-Language: zh-cn`、`X-Security-Token`、`x-snap-traceid`、SDK-HMAC-SHA256(body 哈希) 均通过。
>
> 另：客户端日志再次印证域名 `snap-access.cn-north-4.myhuaweicloud.com` 与真实报错 `campaignId : 参数无效`（PROMPTCENTER.00000001），与实现一致。

---

## 一、现状核对（代码级事实）

### 1.1 豆包工作签到链路

| 环节 | 代码位置 | 事实 |
| --- | --- | --- |
| 能力声明 | `adapter/doubaowork.rs:158-167` | `supports_daily_claim: true`，`supports_client_switch: true` |
| 领取入口 | `claim_daily` `doubaowork.rs:2147` | **第一件事**就 `detect_local_active_uid(host)`，若 `active_uid != cred.uid` 直接返回 `claimed:false`，文案「豆包工作签到绑定桌面端登录账号：请先把该账号『切至桌面端』再领取」 |
| 当前桌面账号识别 | `detect_local_active_uid` `:1967` | 读 `Local State.profile.last_used` → 该 profile 的 `Preferences.saman.last_user`（空 uid 视为登出） |
| 写签到接口 | `post_landing_checkin_claim` `:1101` | `POST /alice/ug/campaign/landing/checkin`，返回 `710010702` = 服务端发放校验异常（设备/账号不匹配或活动试用天数>20 上限） |
| 切号 | `apply_session` `:2412` | **只接受** `CredentialPayload::LocalSnapshot{files}`，否则报「仅含在线 Cookie，缺少桌面端切换快照」。写完文件后 `set_active_profile` 把 `Local State.profile.last_used` 指向该 profile |
| 快照备份 | `backup_session` `:2528` | 备份全部 profile + `Local State`，供回滚 |

**关键结论**：`claim_daily` 的账号门禁在函数最前面，且与写接口服务端校验一致地为「设备 + 桌面在登账号」。因此**对一个不在桌面端在登的账号，无论 Cookie 多新鲜，写接口都会 `710010702`**。这是硬约束，绕不过去（除非拿到该账号在被破解的设备身份上由本机发起，见 §2.3 风险评估）。

### 1.2 批处理引擎现状

`src-tauri/src/scheduler/mod.rs` `run_daily_claim_batch_with_sink`（`~:129`）是一个**纯网络批量**循环：

- 对每个账号：读凭据 → 校验 `Active` + `has_api_token` + `supports_daily_claim` → 前置 `inspect_quota` 拿 `claim_status` → `Available` 才 `claim_daily`。
- 已有防护：工具级熔断（风控/限流后中止该工具剩余账号）、连续网络错误熔断、设备级每日额度记忆（`device_limit_reached` + `remember_device_claim_limit`）、跨设备节流（`last_device_scope`）。
- **它不切号**。批处理里对豆包一旦命中 `detect_local_active_uid != cred.uid` 的分支，`claim_daily` 会**静默返回 `claimed:false`**（`skipped`），于是豆包多账号除「当前桌面在登那一个」外全部被跳过——**这正是用户说的「多账号领取存在问题」的技术根因**。

单账号切换链路是完整且可用的：`commands/switch.rs:switch_desktop_account`（`_104`）已经实现「结束进程 → `backup_session` → `apply_session` 写快照 → `set_active_profile` → `launch_client`」，有并发锁、有回滚。**批处理缺的不是切号能力，而是把切号编排进批量循环**。

### 1.3 码道 CodeArts 链路

| 环节 | 代码位置 | 事实（**2026-11-27 重新逆向 `app.asar` 更正**） |
| --- | --- | --- |
| 签到能力 | `adapter/codearts.rs:239` 附近 | `supports_daily_claim = false`（**已过时，需改 true**） |
| `claim_daily` | `:1400` | **纯 no-op**，返回「无每日签到」——**与真实客户端不符，需重写** |
| 模块注释 | `:3-8`（已同步更正） | 旧称「0 命中」是错的；实锤存在 `/v1/ops/delivery`+`/v1/ops/claim`+`/v1/ops/confirm` 福利链路 |
| **每日签到真实入口** | `BenefitService.getWelfareDelivery` `app.asar` | `GET {policyCenterDomain}/v1/ops/delivery?channel=DESKTOP` → `items[]`，其中 `type=daily_claim` 就是「每日签到领100积分/每日领取」 |
| **领取两步** | `claimWelfare` / `confirmWelfare` | `POST /v1/ops/claim {campaignId,idempotentKey,channel:"DESKTOP"}` → 返回 `userBenefitId` → `POST /v1/ops/confirm {userBenefitId}` |
| **活动类型** | `welfare_ui.js` `normalizeActivityType` | `daily_claim`（每日签到）/`student_certify`（学生认证，每月5000，截止2026-12-31）/`invite`（邀请） |
| **域名** | `product.json` `kernel.envConfig.policyCenterDomain` | `https://snap-access.cn-north-4.myhuaweicloud.com`（与现有 `snap_base_urls` 同 host，带 `.cn` 回退） |
| **签名** | `app.asar` `sign()` (SDK-HMAC-SHA256) | 与适配器 `sign_headers_with_body` **同一算法、同一 canonical 构造**，无新增密码学成本 |
| 请求头 | `BenefitService` | `X-Security-Token`(=STS) / `Agent-Type: PromptCenter` / `Content-Type: application/json` / `X-Language: zh-cn`（delivery 另加 `x-snap-traceid`） |
| 幂等判定 | `mapActivityItem` | `status ∈ {CLAIMED,CONFIRMED,CONSUMED}` 视为已领；`claimable===false` 视为禁用；`campaignId` 为活动键 |
| 额度来源 | `inspect_quota` `:1400+` | `package/overview` 四个 bucket（all/basic/bonus/ondemand credit） |
| 关联本机根因1 | `envelope_from_session` `:318` | 只有「本机导入」才拿到 `{session, loginContext}` 完整快照 → 存 `EXTRA_SESSION_SNAPSHOT`；独立 OAuth 只有 AK/SK/STS，**无快照 → 不能切号** |
| 关联本机根因2 | `refresh_credential_inner` `:1110+` | 非 `isolated_oauth`（即本机导入）账号**绝不消费客户端 refresh token**，只在临期时 `import_from_local` 只读回吸本机磁盘更新的 STS（`CODEARTS_REFRESH_LEAD_MS = 30min`，`EXPIRY_SKEW_MS = 5min`） |
| 保活循环 | `lib.rs:244-390` | 45s 后启动，每 20min 巡检；`is_active_on_client` 且 LocalImport 的账号先 `sync_active_local_credential_if_fresher` 只读回吸，否则 `refresh_credential`；AuthExpired 则标记 Expired + 6h 退避 |
| 数据面 | `snap_base_urls` + `sign_headers_with_body` | `POST {snap-access}/api/v2/chat/completions`，SDK-HMAC-SHA256 |

**关键结论（更正版）**：
1. **码道确有每日签到**，是纯服务端接口（`/v1/ops/delivery` + `/v1/ops/claim` + `/v1/ops/confirm`），**与豆包不同——不需要桌面端在登、不需要切号、不需要驱动客户端**，只需 AK/SK/STS 签名请求即可。这是比豆包简单得多的「真一键领取」。
2. 早期 `codearts.rs` 的「0 命中」结论源于**只搜了 `每日`/`签到` 等中文关键词、且搜的主进程代码段不含渲染层 i18n**，属检索盲区导致的误判。
3. 码道「关联本机」不是 bug，而是本机导入模式的设计后果——为了复用桌面端一次性登录态、避免消费 refresh token 被踢，Qwitch 只做「只读回吸」，STS 周期轮换 → 长时间不回吸/换机必须回客户端重新导入。

---

## 二、magpie 参考对照（社区既有方案）

实地核对 `~/.config/magpie`（v 缓存、`plugin-*`、`credits-daily.json`）与 `magpie` 仓库源码：

1. **magpie 的签到机制是「按账号 API 直签」而非「驱动桌面端」**：`internal/provider/checkin.go` + `workbuddy_checkin.go`、`trae_checkin.go`、`minimax_checkin.go` 都是 `meter(site,a,"/v2/billing/.../daily-checkin")` 这种**凭证直连签到接口**，`plugin_checkin.go` 让插件通过 `auth.checkin` 钩子自签。签到产物记在 `plugin-checkin.json`（按 `provider|signin-key` 分账号），`credits-daily.json` 记录每日额度变化。
2. **magpie 有多账号签到编排**：`checkiner.accounts()` 遍历所有在登账号，`checkinNow` 对每个账号只签一次（`asked[key]`），支持 `only`（手动「Check in now」单账号）。**这套「遍历多账号、各自签一次、按账号记结果」正是豆包多账号批处理要对标的产品形态**。
3. **但 magpie 的豆包插件 `opencode-doubao-work-auth` 没有签到**：`index.mjs` 的 `auth` 块只有 `usage`（读 `subscription/quota/summary`）和 `methods[oauth]`，**没有 `checkin` 钩子**；它的打法是用 CDP 驱动桌面客户端跑「工作任务」（`cdp.mjs` 独立 profile + remote-debugging-port），根本不碰签到。
4. **magpie 没有 codearts 签到插件**，codearts 在 magpie 里是网关型 provider，无每日签到。

**结论**：magpie **没有可直接抄的豆包多账号签到方案**，也没有 codearts 每日签到方案。可借鉴的是它的**「遍历多账号、各自签一次、按账号存结果、支持单账号手动补签」的产品模型**，以及它「凭证直连签到」的简洁性——但豆包服务端把签到绑死在桌面在登账号，无法照搬「凭证直连」，必须走「冷切号」。

---

## 三、豆包「一键多账号领取」可行性与方案

### 3.1 硬约束（不可绕过的事实）

写接口 `POST /alice/ug/campaign/landing/checkin` 的发放校验 = `f(设备身份, 桌面端当前登录账号)`。只有 **B 账号在桌面端处于登录态** 时，B 的签到写请求才会被接受。因此：

- ❌ 不能在「A 保持桌面在登」时给 B 发签到（会 `710010702`）。
- ❌ Cookie 直写切号是死路：上一轮已实测——豆包客户端读的是 **CHIPS 分区 Cookie**（`top_frame_site_key` / `has_cross_site_ancestor`），删了重插非分区副本，客户端视为登出。
- ✅ **唯一可行路径**：让目标账号真正成为桌面端在登账号 → 领取 → 切下一个。即 `apply_session`（完整快照）冷切号。

### 3.2 方案：`claim_daily_batch_with_switch`（串行冷切批量）

在现有 `run_daily_claim_batch_with_sink` 旁增一条「可切号」分支，或在 `commands/accounts.rs::claim_daily_batch` 上层加编排。伪代码：

```rust
// 仅对 supports_client_switch && supports_daily_claim 的工具（当前=豆包）启用
async fn claim_batch_with_switch(accounts: Vec<AccountId>, ...) -> BatchSummary {
    let _guard = SwitchFlagGuard::try_acquire(switching_in_progress)?;   // 与手动切号互斥
    let _q = scheduler.queue_lock().try_lock_owned()?;                    // 与查额/签到批任务互斥

    // 0) 预检：哪些账号有完整快照（LocalSnapshot + SNAPSHOT_ZIP_KEY），否则列入"不可自动领取"清单
    let (switchable, unsupported) = partition(accounts, |a| has_full_snapshot(a));

    // 1) 记录"当前桌面在登账号"，循环结束后切回，避免把用户日常账号踢掉
    let original_active = detect_local_active_uid(host);

    for acct in switchable {
        // 1.1 冷切到该账号（复用 switch_desktop_account 的内核：terminate→backup→apply_session→launch）
        switch_to(acct).await?;                    // 已有 12s 超时；失败则记录并 continue（隔离）
        // 1.2 等待客户端就绪（短暂轮询 detect_local_active_uid == acct.uid，最多 N 秒）
        wait_active(acct.uid, 15s).await?;
        // 1.3 领取（此时 active_uid == cred.uid，claim_daily 不再被门禁拦截）
        let res = adapter.claim_daily(&cred, precheck).await;
        // 1.4 记录 + 反风控抖动（复用现有 BatchClaimBanner 的 jitter_wait 相位）
        record(res); jitter_wait().await;
    }

    // 2) 恢复用户原桌面账号
    if let Some(orig) = original_active { switch_to_uid(orig).await.ok(); }

    BatchSummary { switched_claimed, unsupported_no_snapshot, failed, ... }
}
```

要点：

- **必须串行**：同一设备同一时刻只能有一个桌面在登账号，无法并行；每个账号「切→等就绪→签→抖动」约需数秒。N 个账号≈N×（切换 2-12s + 就绪 2-5s + 签到 1-2s）。
- **前置条件**：每个账号都需 `LocalSnapshot`（含完整 profile 快照 ZIP）。仅 Cookie 在线授权 / 纯 WebviewOauth 的账号**没有快照**，无法冷切，必须先在豆包桌面端登录并「本机导入」一次。→ 产品上要**明确区分「可自动领取（有快照）」与「需先补快照」**两类，并在 UI 引导补全，而不是让批处理静默跳过。
- **回滚/安全**：切换前 `backup_session`，失败 `restore_session`；循环结束切回原账号；与手动切号共用 `SwitchFlagGuard` 防并发。
- **幂等**：靠 `inspect_quota` → `claim_status == Claimed` 幂等跳过 + 现有 `mark_account_claimed`。
- **设备额度**：冷切号不改变设备身份（设备标识在 `Local State` 机器级字段，切号只改 `profile.last_used`），所以**同设备多账号共用一份每日设备额度**——若服务端确实按设备限领，则多账号本就用同一设备签，额度约束仍在；这点需用真实多账号验证（见 §5 验证清单）。

### 3.3 可行性评级

| 维度 | 评级 | 说明 |
| --- | --- | --- |
| 技术可行 | ✅ 高 | 切号内核 `switch_desktop_account` 已存在且稳定（超时已降到 12s、有回滚） |
| 工程成本 | 中 | 需新增「批处理编排 + 就绪轮询 + 恢复原账号 + 补快照引导 UI」 |
| 体验 | 中（受限） | 一键背后是数十秒的连续杀/启客户端；建议明确进度条 + 可取消 |
| 约束 | 强 | 仅对有完整快照的账号有效；纯 Cookie 账号必须补快照 |

**结论：豆包多账号一键领取「可做」，但必须是「有快照账号的串行冷切批量」，并显式管理「无快照账号 → 引导补全」。这与用户「多账号领取存在问题」的痛点精确对口。**

---

## 四、码道 CodeArts「一键领取 + 减少频繁关联本机」可行性与方案

### 4.1 纠正前提：码道**有每日签到**，且是纯服务端接口（比豆包简单得多）

重新逆向 `app.asar` 证实存在完整「活动福利」体系，用户截图中的「每日签到领100积分 / 每日领取」即为每日签到。三步接口：

```text
① GET  {policyCenterDomain}/v1/ops/delivery?channel=DESKTOP
       → { data: { items: [ {campaignId, type, status, claimable, benefitAmount, displayConfig, ...} ] } }
② POST {policyCenterDomain}/v1/ops/claim   body {campaignId, idempotentKey, channel:"DESKTOP"}
       → { data: { id / userBenefitId, ... } }
③ POST {policyCenterDomain}/v1/ops/confirm body {userBenefitId}      // claim 成功后自动确认
```

- **域名**：`policyCenterDomain = https://snap-access.cn-north-4.myhuaweicloud.com`（`product.json` `kernel.envConfig`，与现有 `snap_base_urls` 同 host、带 `.cn` 回退）。Qwitch 已具备该 host 解析。
- **签名**：`app.asar` 的 `sign()` 是 SDK-HMAC-SHA256，`CanonicalRequest = METHOD\nURI\nQuery\nCanonicalHeaders\nSignedHeaders\nSHA256(body)`，与适配器 `sign_headers_with_body` **完全一致**。→ **零新增密码学成本**，直接用现有签名器。
- **固定头**：`X-Security-Token`(STS) / `Agent-Type: PromptCenter` / `Content-Type: application/json` / `X-Language: zh-cn`；delivery 额外加 `x-snap-traceid`(随机 UUID)。
- **每日签到判定**：`items[]` 中 `type == "DAILY_CLAIM"|"daily_claim"`；已领判定 `status ∈ {CLAIMED,CONFIRMED,CONSUMED}` 或 `claimed`；`claimable===false` 为禁用（如需学生认证才解锁）。
- **幂等**：`idempotentKey = "claim_<campaignId>_<timestamp>"`；服务端按 `campaignId` 判重。

**关键差异对照**：

| 维度 | 豆包签到 | 码道签到 |
| --- | --- | --- |
| 是否需桌面端在登 | **是**（绑设备+在登账号） | **否**（纯 STS 签名） |
| 是否需切号/驱动客户端 | **是**（冷切） | **否** |
| 多账号批量 | 串行冷切，重 | **纯并发网络，轻** |
| 我们的密码学成本 | 已有 | **已有（同算法）** |

→ **码道的「一键领取」是真正意义上的并发一键，比豆包容易得多**。当前 `codearts.rs` 把它当「无签到」是误判，必须重写 `claim_daily`。

### 4.2 码道「一键领取」落地方案（新增 `claim_daily` 实现）

```rust
// codearts.rs：把 claim_daily 从 no-op 改为真实签到
async fn claim_daily(&self, cred, _precheck) -> ClaimOutcome {
    let base = Self::policy_center_base(cred);          // 复用 snap_base_urls 同 host
    // 1) 拉活动
    let delivery = self.signed_get(format!("{base}/v1/ops/delivery?channel=DESKTOP"), cred).await?;
    let items = delivery.data.items;
    // 2) 找未领的每日签到
    let daily = items.iter().find(|it| it.kind()==DailyClaim
                    && !it.claimed() && it.claimable != Some(false));
    let Some(a) = daily else { return ClaimOutcome::AlreadyClaimedOrUnavailable };
    // 3) 领取
    let idem = format!("claim_{}_{}", a.campaign_id, now_ms());
    let r = self.signed_post_json(format!("{base}/v1/ops/claim"),
              json!({"campaignId": a.campaign_id, "idempotentKey": idem, "channel":"DESKTOP"}), cred).await?;
    // 4) 确认
    if let Some(ubid) = r.data.user_benefit_id() {
        let _ = self.signed_post_json(format!("{base}/v1/ops/confirm"),
                   json!({"userBenefitId": ubid}), cred).await;
    }
    ClaimOutcome::Claimed { reward: a.benefit_amount }
}
```

配套改动：
- `manifest`：`supports_daily_claim = true`；`supports_multi_instance` 语义不变（码道签到与实例无关）。
- 复用 `sign_headers_with_body(add extra headers)` 传入 `Agent-Type: PromptCenter`、`X-Language: zh-cn`。
- `inspect_quota` 已可读余额；可把 `/v1/ops/delivery` 结果缓存，用于在 UI 显示「今日已领/待领 N 积分」。
- **无副作用**：不写本机、不切号、不杀客户端，多账号在 `claim_daily_batch` 里天然并发执行（复用现有 `run_daily_claim_batch_with_sink`，无需冷切分支）。

### 4.3 减少「频繁关联本机」的三条路径（服务端签到已证明不依赖本机，故关联仅影响**额度查询/网关调用/切号**，不影响签到本身）

> 重要推论：既然码道签到是纯 STS 接口，**只要 STS 有效，签到就能一键批量完成，完全不需要关联本机**。「频繁关联」影响的是长会话/切号场景，与签到解耦。这大幅降低用户痛感——日常签到零关联。

**路径 1：STS 加密保活池（推荐，风险低）**
- 现状 `lib.rs` 保活循环每 20min 对 `is_active_on_client` 账号做 `import_from_local` 只读回吸；`import_from_local` 需读 `secretStorage` + Keychain 解密，客户端未运行/口令失配则回吸失败 → 误判过期 → 要求关联。
- 改进：本地用 master_key 加密缓存「最近成功解密的 `{session,loginContext}`」，`refresh_credential` 在客户端未运行时用缓存快照 + 现有 STS 延长生命周期，仅缓存 STS 真正过期才要求回客户端。→「频繁关联」变「真过期才关联一次」。

**路径 2：isolated OAuth 作网关凭证（已部分存在，风险低）**
- `refresh_credential_inner` 已对 `isolated_oauth` 走 PKCE + DPoP 刷新（不消费客户端 token、不会被踢）。用独立 OAuth 授权 → refresh_token 自动续期，**不依赖本机**。代价：无 `session_snapshot_b64` → 不能切号，只适合「网关调用/额度/签到」，不适合「桌面切号」。

**路径 3：只读零拷贝切换（中风险，配合路径 1）**
- 切号时用最新 STS 覆盖快照内 AK/SK/STS（`codearts.rs:1470-1490` 已为此设计），配合保活池，做到「有快照 + STS 有效即可切，无需重新关联」。

### 4.4 可行性评级（更正）

| 需求 | 评级 | 说明 |
| --- | --- | --- |
| 码道**每日签到**一键领取 | ✅ **可行（真签到）** | `/v1/ops/delivery`+`/claim`+`/confirm`，纯 STS 签名，**无需关联、无需切号**，多账号可并发 |
| 码道一键刷新额度 | ✅ 已有 | `refresh_quota_batch` 可直接用/优化 UI |
| 学生认证/邀请等其它福利 | ✅ 同一体系 | `type=student_certify`（需先认证）、`invite`（生成邀请码），同接口可取 |
| 减少频繁关联（保活池） | ✅ 中高可行 | 缓存最近快照 + 用满 STS 周期，减少无谓回吸失败 |
| 减少关联（独立 OAuth 网关） | ✅ 可行但有取舍 | 零关联但不支持桌面切号 |

**结论：码道「一键领取」= 实现真·每日签到（`/v1/ops/*`），纯服务端并发，零关联、零切号，工程量远小于豆包；「减少频繁关联」用「STS 加密保活池 + 只读回吸用满周期 + 独立 OAuth 分流」显著缓解，且因签到已不依赖本机，日常签到场景的关联痛感基本消失。**

---

## 五、需要真实多账号验证的点（落地前必测）

1. **豆包同设备多账号额度**：豆包服务端签到额度是按「设备」还是「设备+账号」限？若按设备，冷切多账号可能相互挤占——用 2 个真实账号同机切号连签，看第二个是否 `9095`/`710010702`。
2. **豆包 `710010702` 归因**：区分「设备/账号不匹配」vs「活动试用天数>20 上限」——`fetch_campaign_trial_days` + `campaign_trial_capped` 解释后者；前者靠冷切号解决。
3. **豆包切号就绪时间**：12s 超时 + 就绪轮询窗口是否够，实测最坏耗时。
4. **码道 `/v1/ops/delivery` 实测**：用真实 STS 抓一次响应，确认 `items[]` 里 `type=daily_claim` 的 `campaignId`、`status`、`claimable` 字段名与取值；确认 `Agent-Type: PromptCenter` 头是否必需、`x-snap-traceid` 是否强校验。
5. **码道 `claim`/`confirm` 幂等**：重复调用 `claim` 同一 `campaignId` 的返回码；`confirm` 是否必须且有超时容忍；跨天是否按北京时间重置。
6. **码道 STS 与签到解耦验证**：确认「STS 有效但客户端未登录/未运行」时 `/v1/ops/*` 仍可签（关键假设，决定「零关联签到」是否成立）。
7. **STS 保活池回吸成功率**：客户端不运行时的缓存解密路径是否稳定（Keychain 口令失效场景）。

---

## 六、优化方案（分优先级执行清单）

### P0 — 豆包多账号「冷切批量领取」（直接解决用户痛点）
1. `scheduler` 新增 `claim_daily_batch_with_switch`：串行 `switch_to(acct) → wait_active → claim_daily → jitter`，失败隔离，结束切回原账号；与 `SwitchFlagGuard`/`queue_lock` 互斥。
2. `commands/accounts.rs::claim_daily_batch` 对 `supports_client_switch` 工具分流到可切批次；其余工具仍走现有纯网络批处理。
3. 预检快照：无 `LocalSnapshot` 的账号**不静默跳过**，而是汇总为「需先补快照」清单 + UI 引导（对齐 §3.2 前置条件）。
4. `BatchClaimBanner` 增加 `switching` 相位（关客户端/写会话/拉客户端/就绪），显示逐账号进度与可取消。

### P1 — 码道真·每日签到「一键领取」（工程量小于豆包，可先做）
1. `codearts.rs`：`supports_daily_claim = true`；重写 `claim_daily` 为真实实现：`GET /v1/ops/delivery` → 找 `type=daily_claim` 且未领 → `POST /v1/ops/claim` → `POST /v1/ops/confirm`。用现有 `sign_headers_with_body` + `Agent-Type: PromptCenter` + `X-Language: zh-cn`，host 走 `policy_center_base`（复用 `snap_base_urls`）。
2. 幂等：`idempotentKey=claim_<campaignId>_<ts>`；`status∈{CLAIMED,CONFIRMED,CONSUMED}` 视为已领直接跳过。
3. 多账号：天然走现有 `claim_daily_batch` 并发循环，无需冷切分支。
4. 额度：`refresh_quota_batch` 照旧；可选把 delivery 结果缓存用于 UI「今日已领/待领」。
5. 删/改模块注释与单测（`manifest_declares_no_daily_claim`、`claim_daily_is_noop_without_network` 需反转）。

### P2 — 降低码道「频繁关联本机」
1. STS 加密保活池：本地用 master_key 加密缓存最近 `{session,loginContext}` + 解密派生口令；`refresh_credential` 在客户端未运行时优先用缓存快照，仅在缓存 STS 真正过期才要求回客户端。
2. 保活循环把「临期回吸」窗口用满（30min 提前量已够，重点是回吸失败不要误标 Expired，改为「待确认」并重试）。
3. 产品分流：纯网关账号引导用「独立 OAuth」（近乎零关联），需桌面切号的用「本机导入」。

### P3 — 通用健壮性
- 批量领取/切号的「设备额度记忆 + 跨设备节流」逻辑对冷切场景复核（冷切不换设备身份，`device_scope` 应保持稳定）。
- 所有批处理统一「预检分类 → 执行 → 汇总（成功/跳过原因/失败原因）」三段式结果，UI 分列展示，杜绝静默跳过。

---

## 七、一句话总结

- **豆包**：多账号一键领取 = 「有快照账号的串行冷切批量」，因为服务端把签到绑死「设备+桌面在登账号」，这是唯一正解；工程可落地，公式 `逐一 switch→wait→claim→jitter→恢复原账号`，并把「无快照账号」显性化为补快照引导。
- **码道（更正）**：**确实有每日签到**（`/v1/ops/delivery`+`/claim`+`/confirm`，用户截图的「每日签到领100积分/每日领取」即此）。它是纯 STS 签名接口，**不依赖桌面在登、不需切号、不需关联本机**，多账号可并发一键领取，签名算法与现有 `sign_headers_with_body` 完全一致、域名就是现有 `snap-access`——**这是比豆包简单得多、且真正能一键的真签到，建议优先实现**。早期「无签到」结论是检索盲区误判，已在代码注释与本文更正。
- **码道「频繁关联」**：签到已与本机解耦（STS 有效即可签），日常签到零关联；长会话/切号的关联用「STS 加密保活池 + 只读回吸用满周期 + 独立 OAuth 分流」缓解，STS 服务端轮换决定无法字面『永不关联』。
- **参考价值**：magpie 无豆包签到、无码道签到可直接抄，但它「遍历多账号、各自签一次、按账号存结果、支持单账号手动补签」的产品模型，正是两端批处理要对齐的形态。
