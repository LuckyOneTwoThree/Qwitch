# 额度口径、跨端切号与模型方案复制（2026-11-26）

本轮处理三个问题：Agent 接入里「新 Agent 要重新勾一遍模型」、豆包额度只显示 100%、
LobsterAI 积分对不上客户端；并核实了「豆包/码道是否必须关联本机才能切号与领取」。
所有结论都基于本机真实凭据与真实上游响应（只读探测，除签到写接口外未产生写操作）。

## 1. Agent 模型方案：复制 / 同步

**现象**：模型勾选是「每个 Agent 自己的专属号池」上的字段。新挂载 OpenCode 后，
之前给 DSH 配好的那套 `tool/模型` 必须从头再勾一遍。

**改动**：

* `copyAgentToolModelPlan(from, to, toolId)`：把来源 Agent 的勾选集合复制到目标 Agent，
  并按目标模型目录过滤掉已下架的 ID；默认模型一起复制（`defaultModelKey` 形如
  `<poolId>::<tool>/<model>`，需先剥号池前缀再解析）。
* `syncAgentToolModelPlanToAllAgents(source, toolId)`：把当前方案写给其余全部 Agent。
* **批量范围**：`copyAgentAllToolModelPlans(from, to)` / `syncAllToolModelPlansToAllAgents(source)`
  会遍历来源 Agent 配过模型的**全部订阅**，一次完成；否则 4~5 个订阅要逐个点。
* UI 按作用域分两层，避免「跨订阅动作」出现在某一订阅的内容面板里：
  * **订阅工具区**（顶部）：`全部订阅方案` 下拉 —— 「同步全部 N 个订阅到其余 Agent」+
    「从 X 复制全部 N 个订阅」；
  * **当前订阅面板**（模型区右上角）：`复制本订阅`（只列当前工具下有模型的来源）+
    `同步本订阅` 按钮。
* 批量写入的 Toast 收敛为一条：`PendingModelWrite.silent` 让 N 组写命令不逐条弹提示，
  由批量入口汇总成「已把 4 个订阅的模型方案同步到 5 个 Agent（共 20 组）」。
* 默认模型随方案一起搬，但**不再**额外调一次 `setAgentDefaultToolModel`：
  `setAgentToolModels` 新增 `preferredDefaultModelId`，把「勾选集合 + 默认模型」放进
  同一次状态更新与同一次写命令，避免先写一份「只含默认模型」的临时错误配置。

**注意（踩过的坑）**：必须**先**设置默认模型再写勾选集合。`setAgentDefaultToolModel`
读取的是 `pools` 状态（同一 tick 内尚未刷新），反过来会让它把勾选集合覆盖成只剩默认模型。
单测 `可以把一个 Agent 的模型方案复制或同步到其他 Agent` 覆盖了这个顺序。

## 2. 豆包额度：为什么只显示 100%

**证据链**：

1. 本地库 `quota_snapshots`（主-01，05:22:29 导入后首次查额）落库为
   `[{"key":"window_4_0_0","label":"每月额度","total":100,"remaining":100,"unit":"%"}]`
   —— 一个纯百分比桶，`total` 恒为 100。
2. 同一账号现在实时请求 `quota/summary` 返回的是绝对额度：
   `window_type=4, total_amount="2,142", used_amount="19", used_percent=0`。
3. 默认 `autoRefreshOnStartup=false`、`autoRefreshIntervalMinutes=0`，
   这份降级快照此后没有任何机会被覆盖 → 卡片长期显示「剩余 100%」。

**根因（两层）**：

* **解析层**：`append_balance_usage_buckets` 只在「完全没有桶」时才兜底。只要窗口段回了
  一个百分比桶，顶层 `quota_blance_usage` 的绝对额度就被丢弃。
* **数据层**：套餐数据是异步下发的，导入瞬间的响应可能只有 `used_percent`；
  这份降级结果被持久化后不会自愈。

**修复**：

* 只有百分比桶时也合并 `quota_blance_usage`；绝对值桶排主读数位，百分比桶带
  `hasTotal=false`（前端只显示百分比，不画进度条）。
* `inspect_quota` 在「全是百分比」这种异常形态下等 1.2s 重取一次；正常响应零开销。
* 卡片对百分比桶给出 tooltip：「服务端本次只返回了百分比水位，点查额可重新获取」。

## 3. LobsterAI 积分：哪里不准

**证据**（主-01 实时响应）：

* `/api/user/quota`：`freeCreditsTotal=300, freeCreditsUsed=300, freeCreditsRemaining=0,
  dailyCreditsUsed=837.11`
* `/api/user/profile-summary`：`totalCreditsRemaining=361.53`，
  `creditItems=[每日登录奖励 61.53, 邀请奖励 300]`

旧实现把主桶算成 `total = 361.53(跨池剩余) + 300(免费池已用) = 661.53`、
`used = 300`，于是卡片显示「361.53 / 661.53 积分 · 55%」。而「已发放总量」根本不可观测：
过期条目会从 `creditItems` 里消失，免费池的 `used` 也只代表免费池。这个总量是编出来的。

**修复**：

* `QuotaBucket` 新增 `has_total`（serde 默认 true，旧快照兼容）。LobsterAI 的所有桶置
  `false`，主桶只报剩余；前端不画进度条、不显示「/ 总量」，号池候选显示「剩余 361 积分」。
* 签到摘要不再出现「第 1/36500 天」（服务端把活动配成 36500 天），改为「已连续 N 天」。

## 4. 豆包 / 码道的切号与领取：结论与实现

### 4.1 豆包签到绑定「设备 + 桌面端登录账号」

* 网页登录导入的副-02：`landing/get` 正常投放「今日签到领 200 额度」，
  `landing/checkin` 却恒定 `710010702 系统错误`（body 为空或带 scene 都一样）。
* 桌面端在登的主-01：同一接口返回 `checked_in=true, idempotent=true, reward_amount=200`。

结论：写接口校验的是「本机桌面端当前登录账号」，不只是账号本身的资格。
实现：`claim_daily` 先比对 `detect_local_active_uid`，不一致直接返回
「先把该账号切至桌面端再领取」；前端同步禁用这类账号的「领取」按钮。

### 4.2 豆包切号：Cookie 直写已被推翻（回退，只保留快照切号）

> **更正**：本节初稿写的是「Cookie 直写已实测通过」，结论是**错的**。当晚复测被推翻，
> 实现已回退，证据与复盘见下方。

写入这一层确实可复现：Keychain `DoubaoWork Safe Storage` → `PBKDF2-HMAC-SHA1(1003)`
→ 明文 `SHA256(host_key)`(32B) + cookie 值 → `AES-128-CBC` → `v10`。写入后用 openssl
独立解密，与源值 SHA-256 完全一致；用这份 Cookie 直连 `quota/summary` 也能成功。

但「客户端进入登录态」是另一件事，且实测失败：

1. 16:16 用 33 条 passport Cookie `DELETE` + `INSERT` 覆盖 `.doubao.com` 域；
2. 16:17 拉起客户端，它只追加了 9 条 Cookie，`Preferences.saman.last_user` 仍为空；
3. 16:51 客户端把自己的状态写成「已登出」（`user_id` / `sec_user_id` 空串、`credit=0`）；
4. 16:56 再用客户端库里的 Cookie 打 passport → `AuthExpired`。

**根因**：我们删掉的是客户端原有的**分区 Cookie**（`top_frame_site_key` 非空 /
`has_cross_site_ancestor=1`，CHIPS），重新插入的是非分区副本。客户端按分区读取，
读不到会话；而直连 HTTP 不带分区，所以 API 侧看起来完全正常——这两件事被混为一谈。

对比佐证：主-01 在 05:21 抓的整份 profile 快照，解压后跑 `import_from_local`
仍然校验通过（uid `4311635424712839`，passport 有效），说明会话本身没坏，坏的是
「只写 Cookie」这条路径。

**回退**：删除 `apply_cookie_session`/`parse_cookie_pairs`/`chromium_time` 与相关常量、
`capability_flags` 里对 doubaowork 的强制可切标记；`apply_session` 恢复为只接受
`LocalSnapshot`。如果以后要重做，正确方向是**原地 UPDATE `encrypted_value`，
保留 `top_frame_site_key` / `has_cross_site_ancestor` 等属性**，而不是删行重插。

### 4.2.1 关联本机：两个真实缺陷（已修）

* **身份只认 `Preferences.last_user`**：客户端登出后它被清成空串，`read_last_user`
  会把空串当成有效 uid（或直接报「尚未登录」），用户明明刚登录过也会被拦。
  现在：空 uid 视为无记录 → 读客户端 Cookies（safeStorage 解密）→ 打 passport 拿真实 uid；
  两者都失败才报错，且提示改成可执行的一句话。关联成功时把在线 Cookie 一并写进快照，
  关联完就能查额/签到。
* **登出后「本机使用中」永远清不掉**：`detect_local_active_uid` 返回 `None` 同时代表
  「没登录」和「本机没数据」，校准任务按「绝不误清」跳过 → 卡片停在既没有「关联本机」
  也没有「切至桌面端」的死胡同。新增 `ToolAdapter::detect_local_logged_out`（默认 `None`，
  豆包为「Preferences 无 last_user」），校准任务据此清标记；已在运行中的应用验证：
  `is_active_on_client` 由 1 → 0，并写入一条 reconcile 日志。

### 4.2.2 多账号客户端：只读 `Default` 导致的「登录的是另一个账号」

豆包工作客户端自带多账号，实现方式是 **Chromium 多 profile**：

```
DoubaoWork/
├─ Local State          # profile.last_used = "Profile 2" ← 当前账号
├─ Default/             # 账号 A（Lucky, 4311…39）
└─ Profile 2/           # 账号 B（lemontree, 3704…80）
```

早期实现把所有路径写死成 `Default/...`，于是用户在客户端切到账号 B 后，
Qwitch 读到的仍是账号 A 的 `Preferences.saman.last_user`，
「关联本机」就报「本机当前登录的是另一个账号」。

实测对照（同一时刻）：

| 来源 | 结果 |
| --- | --- |
| `Local State.profile.last_used` | `Profile 2` |
| `Profile 2/Preferences` → uid | `3704729780097880`（lemontree） |
| `Default/Preferences` → uid | `4311635424712839`（Lucky） |
| 修复后的 `detect_local_active_uid` | `3704729780097880` ✅ |

修复内容：

* 新增 `active_profile_name()` / `list_profile_names()` / `set_active_profile()`；
* `read_last_user`、`collect_session_relative_paths`、`clean_session_ephemeral_files`、
  `cookie_db_path`、`read_client_cookie_header`、`detect_local_active_uid`、
  `detect_local_logged_out` 全部改为「profile 目录」入参；
* 快照写入 `profile_dir.txt`，切号写回后更新 `Local State.profile.last_used`
  （只改这一个字段，设备标识等机器级状态原样保留）；
* `backup_session` 覆盖所有 profile + `Local State`，回滚可回到切号前账号；
  老快照无 marker 按 `Default` 处理。

新增单测 `multi_profile_client_reports_and_snapshots_active_account` 覆盖
「跟随 last_used → 导入带 profile 标记 → 切号写回并改 last_used」全链路。

### 4.3 码道切号：safeStorage 重加密（已实现并实测）

原来 `apply_session` 直接返回不支持，理由是「本进程产不出合法 safeStorage 密文」——
但 `load_session` 早就在用同一套参数解密，加密只是同一个算法的反向，完全可以做。

实现要点：

* 本机导入时把完整 `{session, loginContext}` 存进凭据 `extra.session_snapshot_b64`；
* 切号时用最新 AK/SK/STS 覆盖快照内同名字段，再用 safeStorage 加密成 base64 写回
  `secretStorage/<sha256("authSession:cn-north-4")>.enc`；
* `capability_flags` 只在「存在该快照」时把 `has_local_snapshot` 置真 → 才出现切号按钮；
* `supports_client_switch = cfg!(target_os = "macos")`。

验证：导入 → 写回 → 再导入，得到同一 `uid`（`c02b6539…`），说明写回的密文能被客户端
同一路径解出。独立 OAuth 授权的账号没有 loginContext，无法切号（会明确提示先本机导入）。

### 4.4 码道领取：确认不存在可实现的每日签到

对 `app.asar` 全文检索 `每日/签到/打卡/daily` 均 0 命中；`benefit/claim` 实测返回
`invalid channel, group not found`。因此 `supports_daily_claim=false` 保持不变，
这不是「没实现」，而是平台没有该能力。

## 5. 移除「手工维护模型目录」

结论：这个入口的边际价值已经归零，保留只会让用户在「为什么这里能让我乱填模型 ID」上多绕一圈。

* 码道：目录已经能从客户端内核日志自动读出（`source=client`），另有内置推荐目录可一键套用；
* 龙虾 / 豆包：都有推荐目录 + 本机缓存/实时探测（`preset` / `client`）；
* 手工维护还会让「模型 ID 到底是不是上游真实 wire ID」变成用户的责任，出错时表现为上游 400，
  排查成本远高于收益。

移除范围：

* 前端：`ManualModelModal` 组件与单测、模型区右上角「手工维护」、空态「手工维护模型目录」、
  store `setAccountManualModels`、顶部来源徽章「手工维护」（单个模型行上的「手工」标签保留，
  只用于标注历史遗留数据）；
* 后端：`set_account_manual_models_cmd`（`lib.rs` / `build.rs` / `capabilities/default.json`）、
  `save_manual_account_models` 及其单测；`apply_account_preset_models_cmd` 保留；
* 兼容：`load_cached_models_map` 仍识别历史 `source="manual"` 的行（不设 TTL、仍登记 wire ID），
  这样老用户库里已有的手工目录不会被清空或失效，只是不能再编辑。

单测净变化：手工目录的「不随 TTL 过期」覆盖并入预设测试，并补了「实时探测覆盖预设」的断言。

## 6. 客户端存活探测与切号超时（全部工具受影响）

用户反馈「所有客户端都显示未运行」「切号一直卡死」「冷启动 45s 太长」。三者是同一根因链：

1. **存活探测依赖 `ps`**：`is_macos_app_bundle_running` / `has_non_helper_pids` 都要跑
   `ps -axo` / `ps -p`。一旦 `ps` 不可用（受限环境、无 Automation/Process 权限、
   被沙箱拦截），这些函数**静默返回 false** → 所有工具一律「未运行」。
2. **切号因此跳过优雅关闭**：`running=false` 就不会执行 terminate，直接覆盖正在运行的
   客户端的会话文件；随后 `launch` 也起不了第二个实例。
3. **启动后 45s 空等**：存活判定永远 false，轮询跑满 64×700ms 才判失败并回滚——
   UI 上就是「一直卡在重启桌面端」，用户感知为「卡死」。

### 修复

| 项 | 旧 | 新 |
| --- | --- | --- |
| ps 不可用 | 当作「未运行」 | **fail-open**：pgrep 已确认进程存在就按运行中处理 |
| 关闭流程 | osascript quit → 等 5s → `pkill -TERM` → 固定等 500ms → 失败 | quit → （仅在请求真的送达时才等）→ TERM → **轮询 2.5s** → KILL → 轮询 2s |
| 优雅请求失败 | 白等满超时 | osascript 返回非 0（如 -10004 权限违例）直接进入 TERM 阶段 |
| 启动存活轮询 | 700ms × 64 = 45s | **300ms × 40 = 12s**（连续两次确认，间隔 400ms） |
| 提示文案 | 「最长约 45 秒」 | 「通常 1~3 秒，最长等待 12 秒」 |

### 实测（豆包工作，客户端在运行）

| 阶段 | 修复前 | 修复后 |
| --- | --- | --- |
| terminate | 6043ms 且**失败**（客户端其实已退出） | **1113ms 成功** |
| launch 返回 | 39ms | 38ms |
| 存活确认 | 45s 超时 | **763ms** |
| 切号总耗时 | ≈51s 后失败回滚 | ≈2s 成功 |

## 7. 切号留下常驻网关子进程 → 龙虾「AI 引擎网关启动失败」

现象：有道龙虾弹窗

> AI 引擎网关启动失败
> Failed migrating startup compatibility: the Gateway or another SQLite maintenance
> command owns this state directory. Stop the gateway before retrying startup or Quick Repair.

**根因**：龙虾的网关（`openclaw-gateway`）是**独立常驻子进程**，不在
`LobsterAI.app/Contents/` 里。Qwitch 切号只按 bundle 路径终止进程
（`cleanup_bundle_leftovers`），App 被 TERM 后这个子进程变成孤儿，继续持有
`~/Library/Application Support/LobsterAI/openclaw/state`（SQLite owner 锁）。
新实例启动时新网关拿不到所有权，于是拒绝迁移并报上面那句话。

证据（`openclaw/logs/gateway-*.log`）：

```
17:27:07 Another gateway (pid 39292) already owns this state directory; refusing to run automatic startup migrations…
17:28:40 Another gateway (pid 44715) already owns this state directory; …
```

而 Qwitch 的 `action_logs` 在同一分钟内是连续的 `switch → 副-02 / 主-01`（4 次），
每次都是「杀 App → 写入 → 重启」，孤儿网关逐次累积。

**修复**：

* `ClientSpec` 新增 `mac_sidecar_process_names`，龙虾登记 `["openclaw-gateway"]`
  （其它工具为空，行为不变）；
* `terminate_client_gracefully` 在「主进程已停止」的每个出口都调用
  `stop_mac_sidecar_processes`：`pkill -TERM` → 轮询 3s → `pkill -KILL`；
  这样即使 App 已经不在、只剩孤儿网关，切号前也会先把它清掉。

**实测**（客户端在跑、网关健康）：

| 项 | 结果 |
| --- | --- |
| terminate 耗时 | 534ms |
| 残留 `openclaw-gateway` | **false** |
| 重启后网关 | port 18789，HTTP **200** |
| 日志 owner 冲突 | 修复后不再出现（历史仅 2 条，均在修复前） |

如已经踩到（本次就是这样），手动恢复一条命令即可：退出龙虾 → `pkill -f openclaw-gateway`
→ 重新打开客户端（或点弹窗里的「重新启动网关」）。

## 8. 门禁

* `cargo fmt` / `cargo clippy --all-targets -D warnings` 通过；`cargo test` **292 passed**。
* `npx tsc --noEmit` / `npm run lint` 0 告警；`npm test` **186 passed / 30 files**。
* 新增测试：额度百分比合并、`has_total` 语义、Cookie 解析、Chromium 时间戳、
  safeStorage 往返、模型方案复制/同步。

## 9. 仍需用户侧动作

* 豆包：直接点「切至桌面端」即可，无需先在客户端登录；若客户端显示未登录，
  在该账号登录一次即补全快照（之后走整份 profile 快照路径）。
* 码道：现有两个账号是在本次改动前导入的，没有会话快照。请在客户端登录该账号后
  点一次「关联本机」，切号按钮才会出现。
* 建议把「额度自动刷新」打开（设置里 `autoRefreshOnStartup` / 刷新间隔），
  否则任何工具的降级快照都只能靠手动「查额」修正。
