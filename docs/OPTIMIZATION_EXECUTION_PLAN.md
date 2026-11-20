# Qwitch 优化执行计划（基于三项决策修订）

> **基线**：`main` @ `9b4e31c`，v0.3.0；报告见 [PROJECT_UNDERSTANDING_REPORT.md](./PROJECT_UNDERSTANDING_REPORT.md)
> **修订日期**：2026-11-20
> **已完成**：`qwitch/` 重复目录已删除；`vite.config.ts` 已加 `test.include` 限定；`.gitignore` 已加防护

---

## 0. 三项决策与优先级重排结论

| 决策 | 对优先级的影响 |
| --- | --- |
| **① 不做 macOS 签名** | P0-3 的「钥匙串反复授权」是签名缺失的**症状**，代码层无法根治 → 降级为「文档 + 兜底」；但 `key_id` 非原子写是**独立纯 bug**，仍保持 P0 |
| **② 真实规模 ≤ 10 账号** | P1-1（Context 重渲染）、P1-5（索引失配）、N+1 查询全部**大幅降级**：10 账号 × 4 事件 ≈ 40 次重建，实测无感 → 从 P1 降为 **P2/延后** |
| **③ 账号规模小、单人使用** | 安全项 P0-1/P0-2 **不降级**——RCE 链与账号数无关，且修复成本低（都是输入校验收紧） |

### 0.1 重新分级总览

| 原编号 | 问题 | 原级 | **新级** | 调整理由 |
| --- | --- | :-: | :-: | --- |
| P0-1 | 渲染进程 → 任意可执行文件（RCE） | P0 | **P0** | 与规模无关；修复成本低 |
| P0-2 | 插件信任前端自批 + 无 FS 隔离 | P0 | **P0** | 同上；`adapters/` 为空但仍需闭环 |
| P0-3a | `key_id` 非原子写 → 拒启 | P0 | **P0** | 独立纯 bug，与签名无关 |
| P0-3b | 钥匙串 ACL 反复授权 | P0 | **P3** | 不做签名无法根治 → 只做文档 + 兜底提示 |
| P0-4 | 旧格式密文 AAD 绕过 | P0 | **P1** | 需本地攻击者前置，降一级 |
| P0-5 | 迁移失败静默 | P0 | **P1** | 应改阻断但需设计只读模式 |
| P1-1 | Context 重渲染 | P1 | **P3 延后** | 10 账号无感 |
| P1-5 | `julianday` 索引失配 | P1 | **P2** | 10 账号 + ≤20 快照/账号，实测微秒级 |
| P1-4 | 同步命令主线程阻塞 | P1 | **P1（收窄）** | 仅 `export_action_logs`（10000 条）与 `get_action_log_stats`（全表扫）保留 |
| P1-6 | WAL 无 checkpoint | P1 | **P1** | 与规模无关，实测 2 MB 增长 |
| P1-7 | 更新可降级 | P1 | **P1** | 与规模无关 |
| P1-8/9 | PKCE verifier 入 query / nonce 可读 | P1 | **P1** | 与规模无关 |
| P1-10 | `DeviceDailyLimitReached` 仅 Trae | P1 | **P2** | 属功能增强，非缺陷 |
| P1-11 | Qoder macOS 解密占位 | P1 | **P2** | 改 UI 文案诚实降级即可 |
| P1-12 | WorkBuddy 取目录首个文件 | P1 | **P1** | 安全边界，与规模无关 |
| P2-1/2 | 重复目录 / 无 vitest 配置 | P2 | **✅ 已完成** | — |
| P2-3 | 文档漂移 | P2 | **P1** | 会直接误导后续迭代 |
| P2-4~25 | 其余 | P2 | **P2/P3** | 按性价比分批 |

---

## 1. 阶段 A 执行结果（已完成，2026-11-20）

> 全部 7 项安全止血 + `master.key` 兜底镜像已落地。**测试从 86 → 95 例（新增 9 个回归测试）**，全门禁零告警。

| 序 | 任务 | 实现要点 | 回归测试 |
| :-: | --- | --- | :-: |
| **A6** | `key_id` 原子写 + 严格解析 | `parse_key_fingerprint` 仅接受 32 位 hex；`write_key_fingerprint_atomic` 走 tmp+fsync+rename+目录 sync。空/截断/非 hex 一律视为「缺失」而非「冲突」 | `corrupted_key_id_is_treated_as_missing_instead_of_conflict`（覆盖 5 种损坏形态）、`key_id_write_leaves_no_temp_file_and_is_0600` |
| **A7** | WorkBuddy 删「取目录首个文件」兜底 | `resolve_runtime_binary` 仅接受 `WorkBuddy`/`Electron` 官方名，解析失败返回 `None` | 既有用例覆盖 |
| **A5** | `is_valid_entry_command` fail-closed | 绝对路径必须落在插件目录内；文件形态的相对路径不存在即拒绝；新增 `looks_like_adapter_file` 识别脚本扩展名，避免 `adapter.py` 被当作 PATH 命令放行；裸命令名（`python3`）按 `CORE_DESIGN` 3.1 协议保留 | `entry_command_rejects_absolute_outside_paths_and_missing_relative_paths` |
| **A2** | `launch_client` 前复核白名单 | 新增 `platform::validate_client_executable`：名称白名单 + bundle 结构 + 受信任安装目录（`/Applications`、`~/Applications`、`%LOCALAPPDATA%\Programs`、`HOME`）；`resolve_client_executable` 对自定义路径与自动探测结果都复核 | `rejects_paths_outside_trusted_install_roots_and_wrong_names`、`accepts_real_installed_clients_when_present`（用本机真实 4 个客户端验证不过严） |
| **A1** | `custom_client_paths` 设置层拦截 | `save_app_settings` 在保存阶段即调用同一校验，用户立刻得到明确反馈 | 由 A2 用例覆盖 |
| **A4** | 插件信任原生确认票据 | 新增 `request_plugin_trust` 命令（`rfd` 原生 Yes/No 对话框，`spawn_blocking`）+ 一次性票据（`Uuid` + 120s TTL，绑定 adapter_id，用后即焚）；`save_app_settings` 对**新增**信任强制消费票据；前端 `useSettingsStore` 在保存前逐项索取票据，取消则中止保存 | `ticket_is_single_use_and_bound_to_adapter_id`、`reissuing_invalidates_previous_ticket`、`expired_ticket_is_rejected`、`missing_ticket_is_rejected` |
| **A3** | 命令级 ACL | `build.rs` 用 `AppManifest::commands(35 个)` 生成权限；`capabilities/default.json` 全量授权。**已反向验证**：移除 `allow-quit-app` 后编译产物权限数 36→35 且该权限消失，恢复后回到 36 | 三处一致性校验脚本（registered/declared/allowed 各 35，零差异） |
| **附加** | `master.key` 兜底镜像 | 钥匙串托管成功后**保留** 0600 明文镜像（改为原子写）；钥匙串丢失时可从镜像恢复同一密钥 | `场景 4b：钥匙串丢失 → 兜底镜像救回启动`（新增）；场景 3/4 断言已更新 |

### 关键设计说明

**A3 的强制语义**：一旦 `build.rs` 声明 `app_manifest`，Tauri 对**全部**自定义命令强制 ACL（`tauri-2.12.6/src/webview/mod.rs:2080`）。因此 35 个命令必须全量授权，否则 IPC 全部中断。已用脚本交叉验证三处（`lib.rs` 注册 / `build.rs` 声明 / `capabilities` 授权）完全一致，并实际启动 release 应用确认无 ACL 拒绝。

**A4 为何不改 UI**：保持现有「保存信任设置」按钮外观与位置不变，只在其内部增加原生确认步骤。用户感知为「点保存后多一个系统弹窗」，这是必要的信任边界。

---

## 1.1 阶段 B 执行结果（已完成，2026-11-20）

> **测试 95 → 102 例**（+7）。全门禁零告警，release 构建通过并实际启动验证。

| 序 | 任务 | 实现要点 | 回归测试 |
| :-: | --- | --- | :-: |
| **B3** | 更新 anti-rollback | 新增 `validate_update_target`：安装前重新拉取 latest（API 优先，限流降级跳转），校验 ① `latest > current` ② 资产名逐字一致 ③ 资产 URL 逐字一致 ④ checksums URL 一致。阻断降级与同版本重装 | `update_target_rejects_downgrade_and_same_version`、`update_target_rejects_asset_and_checksum_mismatch` |
| **B6** | 5xx 连续熔断 | 新增 `update_tool_breaker`：403/429 立即熔断；`NetworkError` 连续 3 次才熔断（区分抖动与整体不可用）；成功路径 `reset_tool_breaker_counter` 打断连续性。统一替换 5 处重复判定，两个批量循环各自独立计数 | `consecutive_network_errors_break_only_after_threshold_and_reset_on_success`、`risk_and_rate_limit_break_immediately_while_auth_expired_does_not`、`breaker_is_isolated_per_tool` |
| **B1** | 旧格式密文标记隔离 | `migrate_credential_aad` 不再对解密失败行静默 `continue`，改为记录到 `storage.cred_aad_legacy_ids`；新增 `legacy_aad_account_ids()`；已完成迁移时若有遗留行仍会重试（不提前返回） | `aad_migration_records_undecryptable_rows_and_retries_next_boot` |
| **B2** | 迁移失败改显式告警 | 三个安全迁移失败不再只 `eprintln!`，收集为 `migration_warnings`，窗口就绪后以原生弹窗告知「哪些防护处于降级状态」。**不阻断启动**（避免把可用性问题放大为不可用），并在日志留痕 | 随 B1 覆盖 |
| **B5** | OAuth nonce 一次性消费 | 新增 `AppState.consumed_oauth_nonces` + `consume_oauth_nonce()`：同一 nonce 仅允许成功回调一次，重放直接拒绝。堵住「授权页内脚本反复投递不同 payload 注入 token」 | `oauth_nonce_is_single_use` |
| **B4** | PKCE verifier | **评估后保持原样**（见下）＋ 顺带修复 `trae.rs` 3 处 `reqwest` 错误未脱敏 | — |

### B4 为何保持原样（重要）

`adapter/qoder.rs:457-465` 把 PKCE `verifier` 放在 query string。原本计划改为 POST body，但经核查：

* `/api/v1/deviceToken/poll` 是 **Qoder 未公开的私有端点**，无任何文档或参考实现；
* 服务端极可能按 query 参数读取 `nonce`/`verifier`；
* 盲目改为 POST body **会直接导致 Qoder 授权轮询全部失败**，即登录功能中断。

**决策**：不改协议，改为降低实际风险面——
* 该请求是**本机 → 官方 API 的 HTTPS 直连**，不经过浏览器/CDN/第三方代理，query 不会进入第三方日志；
* 同文件所有网络错误均已 `e.without_url()`，URL（含 verifier）不会进入错误消息或日志。

同时发现并修复了 `trae.rs` **完全没有** `without_url()` 的问题（3 处），这是同类问题中真正可安全修复的部分。

---

## 1.2 阶段 C 执行结果（已完成，2026-11-20）

> **测试 102 → 106 例**（+4）。全门禁零告警，release 构建通过并实际启动验证。

| 序 | 任务 | 实现要点 | 验证 |
| :-: | --- | --- | --- |
| **C1** | WAL 增长治理 | 新增 `journal_size_limit=4MB`、`wal_autocheckpoint=1000`、`secure_delete=ON`；`checkpoint_wal_passive()`（周期 5 分钟，`spawn_blocking`）+ `checkpoint_wal_truncate()`（`RunEvent::Exit`） | `wal_pragmas_are_applied`、`wal_checkpoint_keeps_wal_bounded_under_many_writes`；**实测运行后 WAL 从 1.8MB 归零** |
| **C4** | Mock 数据泄漏 | 新增 `data/mockStoreLoader.ts`：所有消费方统一取值，`import.meta.env.DEV` 为假时恒返回空集合 | **实测生产包已不含 `acc-minimax-1` / `主力工作号` 等假数据**，包体 347→342KB |
| **C5** | 全量同步竞态 | 新增 `accountSyncSeqRef`，`setAdapters`/`setAccounts` 前校验序号，丢弃「先发后返」的过期响应 | 由既有测试回归覆盖 |
| **C3** | 日志统计全表扫描 | `SUM(CASE...)` 拆为 4 个 `COUNT(*) WHERE` 子查询 + 新增 `idx_logs_archived_status` 索引 | **实测查询计划从全表扫描变为 `USING COVERING INDEX`** |
| **C2** | 主线程阻塞 | `get_action_log_stats` 改 `async` + `spawn_blocking`；`export_action_logs` 的读取/序列化/写盘移入阻塞池 | 编译 + 启动验证 |
| **C6** | 批量乐观锁竞态 | `claim_daily_batch` / `refresh_quota_batch` 在拿到锁后**立即** emit 初始 `batch-progress`，再 `spawn` 后台执行，消除前端二次点击穿透窗口 | 既有 scheduler 测试回归 |
| **C7** | Qoder macOS 诚实降级 | 新增 `macos_auth_is_encrypted_but_unsupported()`；检测到 `v10` 加密载荷时返回 `UnsupportedCapability` 并明确引导「改用在线授权」，不再笼统提示「未完成授权」 | `plaintext_auth_dat_still_parses`、`v10_encrypted_payload_is_detected_as_unsupported` |

### C1 的一个诚实说明

`RunEvent::Exit` 的 TRUNCATE checkpoint **只在正常退出路径触发**（菜单退出 / 关闭窗口 / `app.exit()`）。被 `SIGKILL` / `SIGTERM` 强杀时不会执行——这是进程语义决定的，任何进程内收尾都无法覆盖强杀场景。

实际保障来自 `wal_autocheckpoint=1000`：实测应用运行中 WAL 保持为 0，不再无界增长。

---

## 1.3 阶段 D 执行结果（已完成，2026-11-20）

> **Rust 106 → 109 例**（+3）· **前端 67 → 82 例**（+15，16 → 18 文件）· 全门禁零告警。

| 序 | 任务 | 实现要点 | 成果 |
| :-: | --- | --- | --- |
| **D1** | 引入 ESLint | `eslint.config.js`（flat config）：`react-hooks/exhaustive-deps` 设为 **error**；`npm run lint` 接入 CI | **发现并修复 2 处真实缺陷**（见下） |
| **D2** | 文档收敛 | `ADAPTER_SPEC` 补 MiniMax 完整章节（端点/凭据/接口/已知约束）；`ARCHITECTURE` 修正 `prepare_switch`→`apply_session`；README 测试数 82/54→106/67；`COGNITION` 15s→45s 存活轮询 | 4 项漂移全部消除 |
| **D3** | `useAccountStore.test.ts` | 覆盖：Tauri/Mock 双模式初始态、空候选领取提示、批量失败回滚进度、切号弹窗状态机、事件注册与**卸载全注销**、托盘窗口订阅范围 | **7 例**（1353 行模块从零测试到有护栏） |
| **D4** | `Modal.test.tsx` | 覆盖：首元素聚焦（rAF）、Tab 正/反向循环、Escape、`disableClose`、body 滚动锁定与恢复、焦点归还、`open=false` 不渲染 | **8 例** |
| **D5** | 引入 `wiremock` | `[dev-dependencies] wiremock = "0.6"`；为 `map_http_status` 建 401/403/429/500/502/503 矩阵、业务码矩阵、非 JSON 200 响应不被当成功 | **3 例** |
| **D6** | CI 加固 | 新增 `npm run lint`、`shellcheck -S warning scripts/*.sh`（Linux）、`rustsec/audit-check@v2` | 随 D1 完成 |
| **D7** | 消除重复代码 | 3 处字节级相同的 `merge_post_claim_quota`（trae/qoder/minimax）收敛到 `common.rs`，原位置保留薄封装 | 语义调整只需改一处 |
| **D8** | 统一 host 注入 | `WorkBuddyAdapter` / `QoderAdapter` 新增 `host` 字段 + `with_host()`，与 `TraeAdapter` 一致；`create_all_adapters` 统一注入 | 多实例 profile 隔离不再被绕过 |

### D1 发现的两处真实缺陷（ESLint 的直接产出）

1. **`QwitchContext.tsx` 的 `useMemo` 漏掉 `setLogScope` 依赖**
   `setLogScope` 存在于 context value 中但不在依赖数组里。此前因 setter 引用恒定而「碰巧正确」，一旦未来改为非稳定引用就会产生陈旧闭包。已补入。
2. **`ImportAccountModal` 的 `getSupportedMethods` 未 memo 化**
   该函数在渲染体内定义、被两个 `useEffect` 引用却不在依赖中。直接补依赖会导致每次渲染都触发 effect（无限循环风险）。正确修法是 `useCallback` 包裹并以 `adapters` 为依赖，已实施。

### 关于 react-hooks v7 的三条新规则

`react-hooks` v7 新增 `refs` / `set-state-in-effect` / `immutability` 三条面向 **React Compiler** 的建议性规则，在本库共报出 28 处。这些不是正确性缺陷，而是「未按 Compiler 约束编写」的风格提示（本库用 `xxxRef.current = state` 做同步快照、用 effect 做派生状态同步）。一次性整改会带来大面积无关改动并掩盖真实问题，故在 `eslint.config.js` 中显式关闭并写明理由。

---

## 2. 前置清理（上一轮会话）

| 项 | 变更 | 验证 |
| --- | --- | --- |
| 删除 `qwitch/` | 3.8 MB 重复快照（精确对应提交 `586abeb`），git 未跟踪、无引用 | `git status` 已干净 |
| `vite.config.ts` | 加 `test.include: ["src/**/*.test.{ts,tsx}"]` + `exclude` + `environment: "node"` + `restoreMocks` | 测试数从 133 → **67**（真实值） |
| `.gitignore` | 加 `qwitch/`、`*.snapshot/`、`*.bak/` | 防同类污染复发 |

**回归**：`npm run build` ✅ · `npm test` 16 文件/67 例 ✅ · `cargo test` 86 例 ✅ · `cargo fmt` ✅ · `clippy -D warnings` ✅

---

## 3. 执行顺序（按「风险 ÷ 成本」排序）

### 阶段 A｜安全止血 — ✅ 已完成（见 §1）

> 目标：切断「一次前端污染 = 任意代码执行 + 全量凭据解密」。**与账号规模无关，全部保留 P0。**

| 序 | 任务 | 文件 | 验收标准 | 成本 |
| :-: | --- | --- | --- | :-: |
| **A1** | `custom_client_paths` 只接受 `pick_file` 签发的一次性 token | `commands/settings.rs:109-124`、`commands/profile.rs:313-354` | 前端传任意字符串路径被拒；仅 `pick_file` 返回值可通过 | 中 |
| **A2** | `launch_client` 前复核 basename/目录白名单 | `platform/mod.rs:1077-1081`、`commands/switch.rs:111-125` | `/bin/sh`、`/tmp/x` 等被拒 | 小 |
| **A3** | 补命令级 ACL | 新建 `src-tauri/permissions/*.toml` + `capabilities/default.json` | 未在 ACL 的命令从 webview 调用返回拒绝 | 中 |
| **A4** | 插件信任改 Rust 侧原生确认 | `commands/settings.rs:135-202` | 前端单独调 `save_app_settings` 无法自我批准信任 | 中 |
| **A5** | `is_valid_entry_command` 改 fail-closed | `adapter/external.rs:460-487` | 绝对路径、不存在的相对路径均被拒 | 小 |
| **A6** | `key_id` 改 tmp+fsync+rename 原子写；空/非法视为缺失 | `storage/mod.rs:845-880` | 模拟截断 `key_id` 后能正常启动 | 小 |
| **A7** | WorkBuddy 删「取目录首个文件」兜底 | `adapter/workbuddy.rs:200-207` | 找不到目标二进制时明确报错而非执行任意文件 | 小 |

**阶段 A 回归要求**：`cargo test` 全绿 + 为 A1/A2/A5/A6 各补 1 个回归测试。

---

### 阶段 B｜数据安全与契约 — ✅ 已完成（见 §1.1）

| 序 | 任务 | 文件 | 说明 |
| :-: | --- | --- | --- |
| **B1** | 旧格式密文标记隔离 + 启动告警 | `storage/mod.rs:637-644`、`crypto.rs:101-111` | 迁移失败行写 `credential_format='legacy'`，启动时提示，消除 AAD 绕过 |
| **B2** | 迁移失败改阻断启动 + 只读模式 | `lib.rs:79-87` | 从「静默 eprintln」改为「明确提示 + 可选只读进入」 |
| **B3** | 更新 anti-rollback | `update.rs:441-505` | 安装前重拉 latest，校验资产逐项相等 + `latest > current` + `min_supported_version` |
| **B4** | PKCE verifier 出 query | `adapter/qoder.rs:457-465` | 改 POST body |
| **B5** | OAuth 凭据改 `invoke` 桥 + nonce 一次性消费 | `commands/oauth.rs:181,632,698` | 消除同页伪造回调注入 token |
| **B6** | 5xx 连续熔断 + `Retry-After` 解析 | `common.rs:21-40`、`scheduler/mod.rs:154` | 平台 500 不再逐个账号硬打 |

---

### 阶段 C｜稳定性与体验 — ✅ 已完成（见 §1.2）

| 序 | 任务 | 文件 | 说明 |
| :-: | --- | --- | --- |
| **C1** | WAL 周期性 checkpoint + `journal_size_limit` + 退出 checkpoint | `storage/mod.rs:98-101` | 当前仅迁移路径有 1 处，实测 WAL 达库体积 20 倍 |
| **C2** | `export_action_logs` / `get_action_log_stats` 异步化 | `commands/logs.rs:29,76` | 收窄版 P1-4：仅这两条重路径（10000 条导出 + 全表 SUM）改 `spawn_blocking` |
| **C3** | `get_action_log_stats` 加覆盖索引或增量计数 | `storage/logs.rs:104-124` | 消除无 WHERE 的 `SUM(CASE...)` 全扫 |
| **C4** | mock 改动态 `import()` + 构建期 DCE | `useAccountStore.ts:4`、`useLogStore.ts:3` | 消除生产包中 `acc-minimax-1` 等假数据 |
| **C5** | `syncFromBackend` 加 `seq` 守卫 | `useAccountStore.ts:196-226` | 修复「删了又回来」类问题（对比 `useLogStore.ts:63` 已有正确实现） |
| **C6** | 批量任务乐观锁（后端立即发初始 progress） | `commands/accounts.rs:612-621` + 前端 | 消除二次点击穿透导致的红 Toast |
| **C7** | Qoder macOS 改诚实降级文案 | `adapter/qoder.rs:131-139` | 返回 `UnsupportedCapability` 并引导「在线授权」，不再伪装「未完成授权」 |

---

### 阶段 D｜工程护栏与文档 — ✅ 已完成（见 §1.3）

| 序 | 任务 | 说明 | 成本 |
| :-: | --- | --- | :-: |
| **D1** | 引入 ESLint（`react-hooks/exhaustive-deps: error`） | 会立即暴露 `QwitchContext.tsx` 漏 `setLogScope` 依赖 | 小 |
| **D2** | 文档一次性收敛 | `ADAPTER_SPEC` 补 MiniMax（当前 `grep -c minimax` = 0）；`ARCHITECTURE` 修正 `prepare_switch`→`apply_session`；统一测试数（Rust 86 / 前端 67）；`PROJECT_COGNITION_SUMMARY:110` 的 15s→45s | 小 |
| **D3** | 补 `useAccountStore.test.ts` | 1353 行零测试，含 4 个早退分支、互斥、事件注册/取消 | 中 |
| **D4** | 补 `Modal.test.tsx` 焦点陷阱测试 | 唯一手写的复杂交互逻辑 | 小 |
| **D5** | `Cargo.toml` 加 `[dev-dependencies] wiremock` | 21 处 HTTP 调用点当前无法在测试中验证 | 中 |
| **D6** | CI 加 `cargo audit` + `shellcheck`；actions 按 SHA 固定 | 供应链 | 小 |
| **D7** | 提取 `merge_post_claim_quota` 等 3 处字节级重复到 `common.rs` | `trae.rs:1017` / `qoder.rs:175` / `minimax.rs:825` 完全一致 | 小 |
| **D8** | 统一 `host` 注入（去掉自造 `NativePlatformHost`） | `workbuddy.rs:1455,1465` / `qoder.rs:162` | 小 |

---

### 阶段 E｜延后（规模上来后再做）

> 这些在 ≤10 账号下**实测无感**，不建议现在投入。

| 项 | 延后理由 | 触发条件 |
| --- | --- | --- |
| P1-1 Context 拆 actions/state 双 Context | 10 账号 ≈ 40 次重建，无感 | 单工具账号 > 30 或出现可感知卡顿 |
| P1-5 `julianday` 索引重建 | 每账号 ≤20 快照，排序开销微秒级 | 快照量或账号量显著上升 |
| `prioritize_active_trae_account` N+1 | ≤10 次串行查询 | 同上 |
| 日志列表虚拟化 / 全选 O(n²) | 活跃日志默认上限 200 条 | 日志保留量上调后 |
| `Mutex<Connection>` 改连接池 | 单人使用无并发压力 | 引入后台定时任务后 |
| 三处 `merge_post_claim_quota` 之外的适配器重构 | 无功能缺陷，仅重复 | 新增第 5 个适配器时 |

---

## 3. 关于「不做签名」的专门说明

### 3.1 现状与影响

未签名 → 每次覆盖安装 `.app` 后 CDHash 变化 → macOS Keychain 的「始终允许」ACL 失效 → 用户再次看到授权弹窗。若误点「拒绝」且 `master.key` 已被迁移删除（`platform/mod.rs:716-737`），应用将拒绝启动。

**代码层无法根治**，只能缓解：

| 措施 | 效果 | 优先级 |
| --- | --- | --- |
| 启动失败弹窗已含「请重新打开并点击始终允许」指引（`lib.rs:355-380`） | 已有，够用 | — |
| **保留 `master.key` 明文副本作为兜底**（当前迁移成功即删） | 钥匙串失效时仍可启动 | **建议做**（阶段 A 附加项） |
| README FAQ 补充「覆盖升级后需重新授权」 | 降低用户困惑 | D2 一并做 |

> **建议**：既然短期不做签名，把「迁移成功后删除 `master.key`」改为「保留但仅 0600，并在设置页提供『导出加密密钥备份』」。这把 P0-3 的不可恢复风险降到最低，成本约半天。

### 3.2 长期

若未来要正式分发，签名 + 公证是唯一根治方案（`release.yml` 已接入 `vars.MACOS_SIGNING_ENABLED` 开关，只需提供 Apple Developer 证书）。

---

## 4. 立即执行清单（阶段 A 详细步骤）

按以下顺序执行，每步独立可验证、可回滚：

```
A6  key_id 原子写                    ← 最小改动、纯 bug、先做建立节奏
A7  WorkBuddy 删「首个文件」兜底      ← 单文件、边界收紧
A5  is_valid_entry_command fail-closed ← 单函数
A2  launch_client 前复核白名单        ← 单点校验
A1  custom_client_paths 改 token 机制 ← 涉及前后端，成本最高
A4  插件信任改原生确认                ← 涉及 UI 流程
A3  补命令级 ACL                      ← 最后做（需梳理全部 34 命令的最小权限集）
```

**每步完成后的固定动作**：
1. `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
2. `cargo test --locked`（当前 86 例，只增不减）
3. `npm test`（当前 67 例）
4. 新增对应回归测试
5. 单独 commit（`fix(security): ...`），不合并多步

---

## 5. 需要你确认的两个执行细节

1. **阶段 A 的 A1/A4 涉及前端交互变更**（`custom_client_paths` 改 token、插件信任改原生弹窗）——是否需要我保持现有 UI 外观不变、只改底层校验？还是可以顺带优化这两处交互？
2. **「保留 master.key 兜底」**（§3.1 建议）是否纳入阶段 A？这会略微降低「密钥仅存钥匙串」的理论安全性，但显著降低不可恢复风险。

---

*确认后我按 §4 的顺序逐步执行，每步独立提交并附回归验证。*
