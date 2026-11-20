# Qwitch 全方位深度审查报告

- **审查基线**：分支 `main` (`c35689c`，领先 `v0.2.3` 标签 3 个提交)
- **审查日期**：2026-11-20
- **代码规模**：总计约 29,300 行（4 个内置适配器、33 个 Tauri IPC 命令、6 个后端推送事件）

---

## 1. 审查基线与自动化验证结果

| 验证项 | 实测结果 | 备注 |
| --- | --- | --- |
| **Rust 单元测试** | `63 passed, 0 failed`（macOS/Unix） | Windows 因 2 个 `#[cfg(unix)]` 测试为 `61` 例；[scheduler/mod.rs](../src-tauri/src/scheduler/mod.rs) 为 **0 测试** |
| **Rust 静态检查** | `cargo clippy --all-targets -- -D warnings` 通过 | 编译零告警 |
| **前端类型与测试** | `tsc --noEmit` 通过；Vitest `19 passed`（2 文件） | 仅覆盖纯函数（[selectors.test.ts](../src/domain/selectors.test.ts) 14 例、[settingsUtils.test.ts](../src/context/settingsUtils.test.ts) 5 例），UI 组件与 Context 为 **0% 覆盖** |
| **前端依赖审计** | `npm audit` **0 漏洞**（含 devDependencies） | [README.md](../README.md#L242) 关于 `picomatch` 漏洞说明已过时（上游 CVE-2026-112366 已撤回） |
| **核心文件规模** | 上帝文件膨胀明显 | [commands/mod.rs](../src-tauri/src/commands/mod.rs) 2,913 行、[storage/mod.rs](../src-tauri/src/storage/mod.rs) 2,294 行、[Dashboard.tsx](../src/pages/Dashboard.tsx) 2,258 行、[trae.rs](../src-tauri/src/adapter/trae.rs) 2,199 行、[QwitchContext.tsx](../src/context/QwitchContext.tsx) 1,930 行、[workbuddy.rs](../src-tauri/src/adapter/workbuddy.rs) 1,927 行、[minimax.rs](../src-tauri/src/adapter/minimax.rs) 1,725 行、[platform/mod.rs](../src-tauri/src/platform/mod.rs) 1,681 行、[qoder.rs](../src-tauri/src/adapter/qoder.rs) 1,207 行 |

---

## 2. 九维健康度总览

| 维度 | 评级 | 核心现状与主要风险 |
| --- | :---: | --- |
| **功能完整性** | 🟡 中 | 4 个国内版工具主流程已跑通，但自动更新主链路必败、Trae 零余额账号锁死、Trae 多设备导入与切号存在多处逻辑缺陷、WorkBuddy/MiniMax 存在误判与漏刷 |
| **安全性** | 🟠 偏弱 | 外部插件存在信任劫持、Zip 导入失败泄露明文凭据、MiniMax OAuth 跨账号会话污染、多处凭据文件 `0644` 权限、OAuth 报错泄露 `verifier`/`state` |
| **稳定性** | 🟠 偏弱 | 外部插件与 WorkBuddy 解密存在无超时 `stdin` 管道死锁可冻结全局调度器；SQLite WAL 回滚可致库损；多窗口事件放大与日志竞态串页 |
| **性能** | 🟡 中 | 每次窗口聚焦触发无队列外网请求与 Windows PowerShell DPAPI 解密；48 字段单一大 Context + `ToolBrandIcon` 订阅导致全树重渲染风暴；同步子进程阻塞 Tokio 线程 |
| **架构与代码质量** | 🟡 中 | 领域分层思路清晰，但多轮迭代后上帝文件失控、`platform` 跨层直连 SQLite、跨适配器重复逻辑与状态机口径漂移明显 |
| **兼容性** | 🟡 中 | macOS/Windows 主体可用但 macOS Qoder `v10` 解密为空壳、未签名升级致钥匙串反复授权；Linux 几乎处于半残状态；Rust MSRV 隐式依赖 ≥ 1.88（`slice::as_chunks`）未声明 |
| **测试覆盖** | 🟠 偏弱 | 底层加解密与存储边界测试扎实，但防风控调度器、更新解析主路径、前端 Context 与 14 个组件/页面及辅助脚本均为 0 测试 |
| **依赖与 CI/CD** | 🟡 中 | 前端生产依赖极精简；但 `Cargo.lock` 同时编入 `reqwest 0.12 + 0.13` 等多套重复大版本，Release 流水线缺少版本-标签一致性校验与 Clippy/Audit 门禁 |
| **文档一致性** | 🔴 严重漂移 | 多轮更新（MiniMax、Trae 多设备/zip、删 `hideDockIcon`）后，`README`、全部核心设计文档与 5 张截图均出现明显过时或自相矛盾 |

---

## 3. P0 — 严重问题（阻塞核心功能 / 高危安全与防风控风险）

### P0-1. 自动更新主链路必然失败，且更新 URL 白名单未限定官方仓库
* **位置**：[update.rs](../src-tauri/src/update.rs#L235)、[update.rs](../src-tauri/src/update.rs#L71-L88)、[update.rs](../src-tauri/src/update.rs#L343-L372)
* **影响范围**：全平台软件更新检查、更新日志/包大小展示、Windows 自动安装链路。
* **触发场景**：任意 `v*` 正式发布标签（如 `v0.2.3`、`v0.2.4`）触发 `check_app_update`。
* **根因分析**：
  1. [update.rs](../src-tauri/src/update.rs#L235) 写为 `release.tag_name.trim().trim_start_matches("vV")`。在 Rust 中 `&str` 模式匹配的是**完整字面串 `"vV"`**而非字符集 `['v', 'V']`，导致 `"v0.2.4"` 原样传给 `Version::parse` 并**每次必败**，静默降级至 302 跳转方案，致使 `releaseNotes`、`publishedAt`、`assetSize` 全部为空（与 [README.md](../README.md#L61) 承诺不符）。
  2. `validate_github_url` 放行任意 `github.com` 仓库与任意 `*.githubusercontent.com`，未限定为 `LuckyOneTwoThree/Qwitch`；`verify_checksum` 未校验重定向后最终 URL；`parse_checksum` 未校验 64 位 SHA-256 十六进制格式。
* **建议**：改为 `.trim_start_matches(['v', 'V'])` 并补齐 API JSON 解析单测；收紧 `github.com` 路径前缀至 `LuckyOneTwoThree/Qwitch`，重定向后复核最终 URL，并在校验文件解析时校验 64 位十六进制格式。

### P0-2. 外部插件信任劫持（UI 展示与实际加载的重复 ID 裁决方向相反）
* **位置**：[external.rs](../src-tauri/src/adapter/external.rs#L416-L461)、[adapter/mod.rs](../src-tauri/src/adapter/mod.rs#L209-L217)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L421-L452)
* **影响范围**：所有启用外部 JSON-RPC 插件扩展的用户；涉及任意命令执行与明文凭据外泄。
* **触发场景**：`<data_dir>/adapters/` 下存在两个子目录（如 `a_legit` 与 `z_evil`）声明相同的 `manifest.id`。
* **根因分析**：`scan_external_manifests` 不校验目录名是否等于 `manifest.id`。在设置页 `list_external_adapters` 中采用 `seen.insert` 去重（**字典序第一个目录胜出**展示给用户）；而在加载器 `discover_external_adapters` 中收集到 `HashMap`（**字典序最后一个目录覆盖前者**并实际执行）。同时 `trusted_adapters` 仅记录裸 ID、不固定 `manifest.json`/插件目录内容 SHA-256 哈希，且子进程完整继承宿主环境变量。
* **建议**：强制要求目录名与 `manifest.id` 一致并拒绝重复 ID；信任记录升级为内容哈希固定（`trustedAdapterHashes`），内容变更须重新确认信任；清理子进程危险环境变量并限制相对入口路径不逃逸 `adapter_dir`。

### P0-3. 外部插件与 WorkBuddy 解密的无超时 `stdin.write_all` 死锁可永久冻结全局调度器
* **位置**：[external.rs](../src-tauri/src/adapter/external.rs#L182-L191)、[workbuddy.rs](../src-tauri/src/adapter/workbuddy.rs#L290-L296)
* **影响范围**：全局 `AntiRiskScheduler::queue_lock`（导致所有工具的查额与批量签到永久挂起直至重启应用）。
* **触发场景**：`inspect_quota`/`claim_daily`/`apply_session` 将较大的 `LocalSnapshot`（Base64 编码）或 WorkBuddy 批量 AES-GCM 字段写入子进程 `stdin`，负载超出 OS 管道缓冲区（Windows 默认 4 KiB、macOS 64 KiB）且子进程启动挂起或未先消费 `stdin`。
* **根因分析**：两处 `stdin.write_all(...).await` 均位于 `tokio::time::timeout(...)` **之外**，且早于 stdout/stderr 并发读取启动，一旦阻塞将彻底绕过超时保护。
* **建议**：将 `stdin` 写入与 `stdout`/`stderr` 读取及 `child.wait()` 一并放入 `tokio::time::timeout` 内的 `tokio::join!` 中并发执行，并在超时时主动 `start_kill`；WorkBuddy 子进程剥离 `NODE_OPTIONS` 并防重复 `reply()`。

### P0-4. Trae CN 多设备核心链路存在四重串联缺陷（本机污染、头冲突、跨号覆盖、零余额锁死）
* **位置**：
  * 空路径回退误改本机：[ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx#L187-L208)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L234-L248)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L731-L744)
  * 跨系统请求头矛盾：[trae.rs](../src-tauri/src/adapter/trae.rs#L1200-L1217)、[domain/mod.rs](../src-tauri/src/domain/mod.rs#L294-L324)
  * 切号污染本机 `icube-dc`：[trae.rs](../src-tauri/src/adapter/trae.rs#L1734-L1782)、[trae.rs](../src-tauri/src/adapter/trae.rs#L175-L185)
  * 零余额锁死：[trae.rs](../src-tauri/src/adapter/trae.rs#L993-L998)、[trae.rs](../src-tauri/src/adapter/trae.rs#L1429-L1452)
* **影响范围**：Trae CN 全部多设备导入、跨 OS 导入、桌面切号及免费/零积分账号签到。
* **触发场景与后果**：
  1. **空路径误改本机身份**：在 macOS 上选择「隔离设备 profile → Windows」但未选择 zip/目录即点击确认，`profileDir` 为 `""` 被后端当成本机导入，却依然执行 `override_device_platform("win", true)`，直接把本机账号的 `device_type` 改成 `win` 并抹掉 `device_brand`。
  2. **跨 OS 设备指纹自相矛盾**：导入 Windows zip 到 macOS 时，`override_device_platform` 仅移除了 `device_brand`，**遗漏移除 `os_version`**，导致请求同时携带 `x-device-type: win` 与 `x-os-version: macOS 15.x`，极易触发平台风控校验。
  3. **切号累积外部设备键劫持本机**：切换到外部设备导入的账号时，`apply_session` 既保留本机 `icube-dc:<LOCAL_ID>` 又写入外部快照里的 `icube-dc:<FOREIGN_ID>` 到本机 `storage.json` 与 `state.vscdb`。随着切换累积多个 `icube-dc:*` 键，按字典序取首键的 `device_id_from_storage` 会将外部设备 ID 误认作本机设备 ID。
  4. **零余额账号无法查额与签到**：`fetch_entitlement_quota` 在 `total <= 0.0` 时直接返回 `NetworkError` 且阻断后续 `fetch_checkin_status`，导致 0 积分新号/过期免费号既刷不出额度也永远被调度器跳过签到。
* **建议**：前端强制校验 `profileMode === "device"` 时 `profileDir` 非空，后端仅在确实使用外部 profile 时执行平台覆盖且同步删除 `os_version`；`apply_session` 写入 `auth_entries` 时过滤掉外部快照中的 `iCubeAuthInfo://icube-dc:*` 与 `usertag`；允许 Trae CN `code == 0` 且 `total == 0` 构造 0 余额快照并继续查询签到状态。

### P0-5. 窗口聚焦高频触发无队列外网请求与系统进程调用（违背防风控边界）
* **位置**：[QwitchContext.tsx](../src/context/QwitchContext.tsx#L617-L622)、[QwitchContext.tsx](../src/context/QwitchContext.tsx#L718-L730)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L539-L579)、[minimax.rs](../src-tauri/src/adapter/minimax.rs#L1081)、[qoder.rs](../src-tauri/src/adapter/qoder.rs#L109-L118)
* **影响范围**：所有启用 MiniMax / Qoder / 多窗口（主窗口 + 托盘）的桌面端用户；涉及防风控、性能、电源与状态同步。
* **触发场景**：每次启动应用（主窗口与隐藏托盘各执行一次）或切回 Qwitch 窗口（节流仅 4 秒）。
* **根因分析**：`reconcile_active_accounts` 直接复用重量级 `adapter.import_from_local`：
  1. 对 **MiniMax** 会绕过 `AntiRiskScheduler` 直接向 `https://agent.minimax.cn/matrix/api/v1/user/profile` 发起实时外网 HTTP 请求（最长阻塞 20 秒），违背「无静默后台请求、严格单队列」的产品防风控边界。
  2. 对 **Windows Qoder** 每次窗口聚焦都会拉起 `powershell.exe` 执行 DPAPI 解密。
  3. 当检测到本机当前账号不在库中而执行 `clear_active_on_client`（[commands/mod.rs](../src-tauri/src/commands/mod.rs#L571-L574)）时，**未广播 `qwitch://account-updated`**，UI 仍残留错误“本机使用中”标记。
* **建议**：在 `ToolAdapter` 上拆分轻量的纯本地只读方法 `detect_local_active_uid(&self, host)`（严禁联网、严禁拉起外部解密脚本），仅在主窗口/按需执行且加后端去重节流；`clear_active_on_client` 发生变更后主动发送事件。

### P0-6. MiniMax 跨账号会话污染、风控熔断绕过与违反 Fail-Closed
* **位置**：[minimax.rs](../src-tauri/src/adapter/minimax.rs#L414-L424)、[minimax.rs](../src-tauri/src/adapter/minimax.rs#L608)、[minimax.rs](../src-tauri/src/adapter/minimax.rs#L716-L749)、[minimax.rs](../src-tauri/src/adapter/minimax.rs#L1206)、[minimax.rs](../src-tauri/src/adapter/minimax.rs#L1215-L1267)
* **影响范围**：所有 MiniMax Code CN 多账号用户（OAuth 导入、查额、批量签到与切号）。
* **触发场景与后果**：
  1. **跨账号凭据污染**：设备码 OAuth 调用 `build_session_snapshot` 读取本机当前已登录账号 A 的 `minimax-agent-cn-config.json` 作为模板，仅在键不存在时才将 `tokens` 和 `user` 置空对象，导致账号 A 的 `tokens`/`user` 敏感字段被直接打包进账号 B 的快照并在切号时回写。
  2. **风控熔断绕过**：`inspect_quota` 对 `fetch_signin_status` 使用 `.ok()` 吞掉了 `401`/`403`/`429`；`post_signin_claim` 对所有非 2xx（含 `403`/`429`/`5xx`）都做已签文案匹配，导致调度器无法触发工具级风控熔断。
  3. **违反 Fail-Closed 与状态滞留**：`claim_daily` 忽略 `precheck` 重复请求 `/signin/status`，且当状态为 `Unknown`（Upcoming）或 `Unsupported`（Disabled）时未拦截而直接发起 `POST /signin/claim`；遇到 `already_claimed`（`claim_result == 2`）时 `merge_post_claim_quota` 传入 `confirmed_claimed = false`，一旦领后查额失败即长期停留在 `Available`。
* **建议**：`build_session_snapshot` 强制覆写 `tokens: {}` 与 `user: {}`；`inspect_quota` 向上传播 `AuthExpired`/`RiskTriggered`/`RateLimited`；`post_signin_claim` 仅在 `400`/`409` 检查幂等文案；`claim_daily` 严格仅在 `Available` 时发起领取，并在 `confirmed_claimed || already_claimed` 时合成 `Claimed`。

---

## 4. P1 — 高危问题（功能异常 / 数据与凭据泄漏 / 稳定性隐患）

### P1-1. WorkBuddy 将 HTTP 200 非 JSON 响应（HTML/WAF/劫持页）误判为“今日已签到”
* **位置**：[workbuddy.rs](../src-tauri/src/adapter/workbuddy.rs#L1463-L1490)、[workbuddy.rs](../src-tauri/src/adapter/workbuddy.rs#L350-L355)
* **影响范围**：WorkBuddy 每日签到准确性。
* **触发场景**：网络代理、企业网关、CDN 错误页或 WAF 在 HTTP 200 下返回 HTML 或非 JSON 文本。
* **根因**：`Err(_) if status.is_success() => Value::Null` 将任意解析失败的 2xx 文本转成 `Value::Null`（导致下方 `else` 分支成为死代码），随后 `checkin_claim_idempotent(200, &Value::Null)` 直接返回 `true`，误标为已签并写入 `last_claimed_at`。
* **建议**：仅当 `raw_text.trim().is_empty()` 或字面 `"null"` 时按 `Value::Null` 处理，其余解析错误返回 `NetworkError`。

### P1-2. Zip 导入出错遗留明文凭据临时目录，且缺乏 Zip Bomb / Symlink 防护
* **位置**：[commands/mod.rs](../src-tauri/src/commands/mod.rs#L117-L225)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L715-L722)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L850-L857)、[ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx#L173-L176)
* **影响范围**：Trae CN 导出包导入与跨机导出安全；主线程/Tokio 响应性。
* **触发场景**：导入损坏/无登录态的 `.zip`，或在另一台机器上复制执行引导页导出命令。
* **根因**：
  1. `import_local_account` 与 `preview_local_account` 在 `adapter.import_from_local(...).await?` 失败提前返回时跳过 `remove_dir_all`，使解压出的明文 `storage.json` / `state.vscdb` 永久滞留系统临时目录（默认权限非 `0700`）。
  2. `extract_zip_to` 通过外部 `ditto`/`unzip`/`Expand-Archive` 同步阻塞执行，无压缩包大小与解压体积限制，且 `find_storage_root` 使用 `is_dir()` 会跟随符号链接。
  3. 前端生成的 macOS/Windows 一键导出命令同样把明文拷贝到临时目录（`$(mktemp -d)/Trae CN`、`$env:TEMP\TraeCN-profile`）且打包后从未删除。
* **建议**：引入 RAII `TempDirGuard`（`Drop` 必删 + Unix `0o700`）；使用纯 Rust `zip` crate 在 `spawn_blocking` 中做路径穿越/符号链接拒绝与单文件/总解压上限校验并仅提取白名单文件；前端一键导出命令末尾追加临时目录清理和源文件存在性校验。

### P1-3. Trae 导入向导预检错误契约断裂，错误提示成为死代码
* **位置**：[ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx#L127-L153)、[ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx#L459-L475) vs [commands/mod.rs](../src-tauri/src/commands/mod.rs#L295-L332)
* **影响范围**：Trae CN 两步向导的用户反馈体验。
* **触发场景**：用户选中错误目录、损坏 zip 或输入不存在路径。
* **根因**：后端 `inspect_profile_dir` 遇异常直接返回 `Err(String)` 而非 `Ok(ProfileInspectResult { has_storage: false, .. })`，前端 `catch` 将错误吞掉并 `setInspectResult(null)`，导致界面只渲染空白 `&nbsp;`，464–465 行红字提示在桌面端永远不可达；同时若切换状态时正在预检，`cancelled = true` 会导致 `inspecting` 永远卡在“校验中…”。
* **建议**：在前端保存 `inspectError` 状态并直接展示后端错误信息；`useEffect` 清理和早退分支重置 `setInspecting(false)`；`inspect_profile_dir` 改为 `async` 并下放 `spawn_blocking`。

### P1-4. 签到状态口径漏掉 `minimax_code_cn`、领后刷新失败卡死 `Available` 与导入后缺刷新
* **位置**：
  * 前后端日重置列表遗漏：[storage/mod.rs](../src-tauri/src/storage/mod.rs#L1234)、[selectors.ts](../src/domain/selectors.ts#L40-L59)
  * 领后快照滞留：[qoder.rs](../src-tauri/src/adapter/qoder.rs#L874-L890)、[trae.rs](../src-tauri/src/adapter/trae.rs#L1620-L1630)、[workbuddy.rs](../src-tauri/src/adapter/workbuddy.rs#L1531-L1541)
  * 导入查额未刷新令牌：[commands/mod.rs](../src-tauri/src/commands/mod.rs#L619-L659)
* **影响范围**：MiniMax、Qoder、Trae、WorkBuddy 签到状态显示与自动化决策准确性。
* **触发场景**：MiniMax 签到后服务端缓存短暂延迟或领后查额失败；Qoder 活动在查额与领取间隙已被领完（`claimable` 为空）却把 `precheck` 的旧 `Available` 写回数据库；导入已临近/刚过期的 MiniMax 本机账号（1 小时有效期但 `refresh_token` 有效）直接查额被判 `Expired`。
* **建议**：将 `minimax_code_cn` 纳入前后端 `dailyResetTool` 并统一 `raw === "claimed"` 跨日判定；统一四个适配器采用 `merge_post_claim_quota` 策略（Qoder 空 `claimable` 直接用已获取的 `payload` 重建快照）；`record_import_quota_check` 执行前调用 `adapter.refresh_credential` 并落盘。

### P1-5. 主密钥恢复边界漏洞与未签名 macOS 升级引发钥匙串拒启风险
* **位置**：[platform/mod.rs](../src-tauri/src/platform/mod.rs#L653-L670)
* **影响范围**：旧版 Quiver 升级用户及所有 macOS 覆盖升级用户。
* **触发场景**：
  1. `dev.qwitch.app` 钥匙串残留旧/错密钥而正确密钥仍在 `dev.quiver.app` 时，因 `if persistent && keyring_hex.is_none()` 短路跳过旧条目探测，导致直接拒绝启动。
  2. macOS 发布包未做 Apple Developer ID 签名/公证（ad-hoc 签名），每次升级覆盖 `.app` 后二进制 CDHash 变化，macOS Keychain 原“始终允许”ACL 失效并再次弹窗；若用户误点“拒绝”，而 `master.key` 已在迁移时删除，应用将直接启动失败。
* **建议**：当 `keyring_hex` 无法通过 `key_matches_expectation` 验证时，再按需回退探测 `LEGACY_KEYRING_SERVICE`；长期应配置 Apple Developer ID 稳定签名，短期在启动拒启弹窗中增加针对 macOS 升级后重新点击“始终允许”的具体指引。

### P1-6. 日志管理存在 500 条分页截断、跨视图竞态串页与误删筛选隐藏项
* **位置**：[QwitchContext.tsx](../src/context/QwitchContext.tsx#L528-L605) vs [commands/mod.rs](../src-tauri/src/commands/mod.rs#L1663)；[Dashboard.tsx](../src/pages/Dashboard.tsx#L216-L221)、[Dashboard.tsx](../src/pages/Dashboard.tsx#L1217-L1268)
* **影响范围**：操作日志加载、筛选、批量归档与永久删除。
* **触发场景**：
  1. 加载超过 500 条历史日志后，任意刷新触发 `refreshLogs(limit = 600)` 被后端 `.clamp(1, 500)` 静默截断回 500 条。
  2. 点击「加载更多」或刷新期间切换「活跃 / 已归档」，无请求序号校验导致旧视图日志直接覆盖或追加进新视图。
  3. 勾选 5 条日志后输入搜索词或筛选只显示 1 条，点击「删除已选 (5)」会直接永久删除 4 条当前不可见的日志，且代码先 `setSelectedLogIds([])` 再请求，失败后选区丢失。
* **建议**：后端或前端改用基于 `before_id` 游标分页并统一上限；在日志拉取引入单调 `requestSeq` 与 `scope` 校验；筛选条件变化时自动裁剪 `selectedLogIds` 至可见集合，并等待 IPC 成功后再清空选区。

### P1-7. 批量任务缺少前端乐观锁（最长 20s 静默盲窗）与巨型 Context 渲染风暴
* **位置**：[QwitchContext.tsx](../src/context/QwitchContext.tsx#L784-L814)、[QwitchContext.tsx](../src/context/QwitchContext.tsx#L886-L950)、[QwitchContext.tsx](../src/context/QwitchContext.tsx#L1817-L1921)、[ToolBrandIcon.tsx](../src/components/ToolBrandIcon.tsx#L26)、[AccountCard.tsx](../src/components/AccountCard.tsx#L39)
* **影响范围**：主面板与托盘的操作流畅度及防重复点击可靠性。
* **触发场景**：点击「刷新全部额度」/「一键领取」（首个账号为 MiniMax 需先刷新 OAuth Token，后端最长 20 秒才发首个 `batch-progress`）；以及每次 Toast 弹显/消失、进度条更新、日志刷新。
* **根因**：触发批量任务时前端未立即加同步 ref 锁或写入乐观 `batchProgress` 状态，用户双击触发后端 `try_lock_owned` 报错。同时 48 个字段全堆在单一 `QwitchContext` 中，连纯展示图标 `ToolBrandIcon`（日志页有数百个实例）都调用了 `useQwitch()`，导致 `React.memo(AccountCard)` 完全失效、任何轻微状态更新触发全树重渲染。
* **建议**：后端在获取 `queue_lock` 后立即发出初始 `batch-progress` 事件，前端配合 ref 防抖；立即移除 `ToolBrandIcon` 对 `useQwitch()` 的无效订阅；将 `QwitchContext` 按更新频率拆分为独立 Context。

### P1-8. Qoder macOS 本地解密实为空壳 & `scripts/*.sh` 存在多处阻断与安全缺陷
* **位置**：[qoder.rs](../src-tauri/src/adapter/qoder.rs#L133-L141)、[export_trae_profile.sh](../scripts/export_trae_profile.sh#L35-L71)、[check_device_isolation.sh](../scripts/check_device_isolation.sh#L44-L58)、[trae_probe.sh](../scripts/trae_probe.sh#L61-L146)
* **影响范围**：macOS/Linux Qoder 本地导入体验；运维与多设备排障脚本使用者。
* **根因与后果**：
  1. `try_decrypt_macos_auth_dat` 仅剥离 `b"v10"` 后直接 `JSON.parse`，未实现 Chromium `safeStorage` 的 PBKDF2-HMAC-SHA1 + AES-128-CBC 解密（Linux 无分支），导致 macOS 本地导入始终无 `has_api_token`。
  2. `export_trae_profile.sh --current` 拿刚拷贝的目录与自身本机目录比较 `device_id`，**100% 报错 `exit 1`**；拷贝 `state.vscdb` 未带 `-wal`/`-shm` 且导出凭据权限为 `0644`（文档示例直接指向 `/tmp`）。
  3. `check_device_isolation.sh` 未校验 `MAIN_DEV` 非空，本机未登录或非 macOS 下误报 `✅ PASS`。
  4. `trae_probe.sh` 将明文 JWT 放在 `curl -H` 命令行参数暴露于 `ps aux`；`cmd_claim` 在检查 `403`/`429` 熔断**之前**就向 `--alt-device` 发出二次请求。
* **建议**：修复三个脚本（区分本机导出与对比模式、权限 `0700`/`0600`、`curl -K -` 经 stdin 传 Header、先判熔断再发下次请求）；在文档中明确 Qoder macOS 需走 PKCE 授权或补齐 Keychain 解密。

### P1-9. Release 流水线缺少版本-标签一致性门禁与质量检查
* **位置**：[.github/workflows/release.yml](../.github/workflows/release.yml#L10-L26)、[release.yml](../.github/workflows/release.yml#L71-L73)、[release.yml](../.github/workflows/release.yml#L173-L186)
* **影响范围**：正式版本发布产物准确性与升级链路。
* **触发场景**：推送 `v0.2.4` 标签但忘记同步版本号，或手动触发 `workflow_dispatch`（默认硬编码 `v0.2.3`）。
* **后果**：可能将 `0.2.3` 产物发布到 `v0.2.4` Release（导致应用内更新死循环），或把最新 `main` 覆盖到已有旧标签；且发布流水线跳过 `cargo fmt`/`clippy`/`npm audit`、`x86_64` job 在 arm64 runner 上重复执行无 `--target` 的宿主机测试、`gh release edit` 丢失 `--generate-notes` 自动变更日志。
* **建议**：在 `verify` job 增加 `TAG == v<package.json> == v<Cargo.toml> == v<tauri.conf.json>` 强校验，补齐 `fmt`/`clippy`/`npm audit`/`--locked`，移除 `workflow_dispatch` 写死的旧默认版本并防止静默覆盖已有正式 Tag。

---

## 5. P2 — 中危问题与潜在风险

### 5.1 后端适配器、存储与平台层

| 编号 | 问题摘要 | 位置 | 影响与触发场景 |
| --- | --- | --- | --- |
| **P2-1** | **MiniMax `import_from_local` 丢弃服务端真实 `user_id`、`auth-state.json` 校验不一致及刷新边界问题** | [minimax.rs](../src-tauri/src/adapter/minimax.rs#L1046-L1087)、[minimax.rs](../src-tauri/src/adapter/minimax.rs#L1283)、[minimax.rs](../src-tauri/src/adapter/minimax.rs#L1342-L1381)、[minimax.rs](../src-tauri/src/adapter/minimax.rs#L507) | 缺失 `minimax-agent-cn-config.json` 时 `fetch_user_profile` 拿到 `_p_uid` 却丢弃，退回 token 哈希导致与 OAuth 导入重复建号；`auth-state.json` 导入可选但切号必填；无 `refresh_token` 临期 5 分钟内提前判 `AuthExpired`；`interval * 1000` 未防溢出；[scheduler/mod.rs](../src-tauri/src/scheduler/mod.rs#L174) 静默忽略单次轮换令牌落盘失败 |
| **P2-2** | **MiniMax 设备授权轮询单次网络抖动即关窗中止** | [commands/mod.rs](../src-tauri/src/commands/mod.rs#L2658-L2723) | 与 WorkBuddy/Qoder 容忍 5 次连续错误不同，MiniMax 首次网络抖动即关闭登录窗，且 `SlowDown` 未按 RFC 8628 持续累加轮询间隔 |
| **P2-3** | **WorkBuddy Token 切号丢失企业字段、轮询吞错误、端点白名单过宽** | [workbuddy.rs](../src-tauri/src/adapter/workbuddy.rs#L231-L261)、[workbuddy.rs](../src-tauri/src/adapter/workbuddy.rs#L465-L482)、[workbuddy.rs](../src-tauri/src/adapter/workbuddy.rs#L1574-L1637) | Token 模式 `apply_session` 丢弃 `enterpriseId`/`domain` 且失败不清理 `.workbuddy-desktop.info.tmp`；`poll_oauth_result` 将 401/403/429/500 均返回 `Ok(None)` 失效 5 次熔断并轮询满 180 次；`validate_endpoint` 允许任意路径/端口/泛 `*.tencent.com` 且未禁 HTTP 重定向 |
| **P2-4** | **Qoder `machineid` 文件名拼错、`.qoder-app-status.json` 残留与多活动部分失败丢状态** | [qoder.rs](../src-tauri/src/adapter/qoder.rs#L166)、[qoder.rs](../src-tauri/src/adapter/qoder.rs#L396-L399)、[qoder.rs](../src-tauri/src/adapter/qoder.rs#L896-L972)、[qoder.rs](../src-tauri/src/adapter/qoder.rs#L1046-L1061) | 兜底读取 `"machineid"` 而非 `"auth.machine-id"`；`apply_session` 缺失 status 快照时残留上一账号文件且 `main.sqlite` 查询无 `ORDER BY` 可能串号；多活动第 1 个成功后第 2 个报错会丢弃第 1 个已领状态；缺失 `campaignId` 字段误报“已领取” |
| **P2-5** | **Trae 已登出客户端被导入为幽灵账号 & 预检与导入设备源不一致** | [trae.rs](../src-tauri/src/adapter/trae.rs#L458-L541)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L261-L271) | 登出后 `storage.json` 仍残留 `icube-dc:`/`usertag`，导入会产出无 Token 的假账号；`inspect_profile_dir` 仅读 `storage.json` 而 `trae.rs` 优先读 `aha/TinyStorage` |
| **P2-6** | **通用辅助器边界缺陷：`json_uid_to_string` 接受 `0`/`-1`、幂等文案误杀与路径校验允许 `"."`** | [common.rs](../src-tauri/src/adapter/common.rs#L8-L38)、[common.rs](../src-tauri/src/adapter/common.rs#L40-L162)、[adapter/mod.rs](../src-tauri/src/adapter/mod.rs#L223-L230) | Protobuf 默认值 `0` 会遮蔽真实 fallback UID；`"签到失败：今日已签到"` 因含 `"失败"` 被否定且未扫描 MiniMax `base_resp.status_msg`；`is_valid_adapter_id`/`is_valid_path_segment` 允许 `"."` 及尾点 |
| **P2-7** | **SQLite WAL 回滚潜在库损、读库缺 `busy_timeout` 及切号并发窗口** | [adapter/mod.rs](../src-tauri/src/adapter/mod.rs#L79-L122)、[trae.rs](../src-tauri/src/adapter/trae.rs#L552-L557)、[qoder.rs](../src-tauri/src/adapter/qoder.rs#L391-L395)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L1189-L1455) | `restore_files` 在 `state.vscdb` 恢复失败时继续删除 `-wal` 会破坏数据库；只读 SQLite 缺 `busy_timeout` 易 `SQLITE_BUSY`；切号成功后 1.8s 延迟清标志导致连续切号误报错，且切号与批量查额/签到互不互斥 |
| **P2-8** | **`platform` 层跨层直连 SQLite、macOS 强杀漏清辅助进程、`trusted_adapters` 不热更新** | [platform/mod.rs](../src-tauri/src/platform/mod.rs#L223-L254)、[platform/mod.rs](../src-tauri/src/platform/mod.rs#L952-L996)、[lib.rs](../src-tauri/src/lib.rs#L97-L111)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L885-L915) | `read_custom_client_path` 每次探测直开新 SQLite 连接并反复执行 `create_dir_all`/`chmod`/`migrate_legacy_data_dir`；`terminate_client_gracefully` 超时 `SIGTERM` 分支未调 `cleanup_bundle_leftovers`；取消信任外部插件后需重启才真正卸载；`launch_tool_profile` 已无 UI 调用且 macOS 缺 `open -n` 无法多开 |

### 5.2 安全加固缺口

| 编号 | 问题摘要 | 位置 | 影响与触发场景 |
| --- | --- | --- | --- |
| **P2-9** | **凭据文件写入/回滚权限降级为 `0644`** | [adapter/mod.rs](../src-tauri/src/adapter/mod.rs#L95)、[trae.rs](../src-tauri/src/adapter/trae.rs#L1759)、[qoder.rs](../src-tauri/src/adapter/qoder.rs#L1031)、[workbuddy.rs](../src-tauri/src/adapter/workbuddy.rs#L1630)、[minimax.rs](../src-tauri/src/adapter/minimax.rs#L896-L907) | Unix 上切号写入和 `restore_files` 回滚均以默认 umask（`0644`）落盘，甚至把 MiniMax 原 `0600` 的 `auth.json` 回滚降级为同机其他用户可读；`minimax.rs` 先写后 `chmod` 存在 TOCTOU 窗口 |
| **P2-10** | **OAuth 报错泄露 URL 敏感参数 & 明文 `uid`/Token 在前端内存留存** | [qoder.rs](../src-tauri/src/adapter/qoder.rs#L490)、[workbuddy.rs](../src-tauri/src/adapter/workbuddy.rs#L463)、[commands/mod.rs](../src-tauri/src/commands/mod.rs#L877)、[ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx#L67-L70) | `reqwest::Error` 未调用 `.without_url()`，网络异常把 PKCE `verifier`/`nonce`/`state` 暴露到 `qwitch://oauth-error`；`preview_local_account` 向前端传未使用明文 `uid`；`ImportAccountModal` 取消关闭后不清空 `rawInput` 且文本框未禁用 `spellCheck`/`autoComplete` |
| **P2-11** | **日志脱敏绕过边界 & 外部插件/Windows DPAPI 安全加固不足** | [storage/crypto.rs](../src-tauri/src/storage/crypto.rs#L121-L124)、[storage/crypto.rs](../src-tauri/src/storage/crypto.rs#L215-L247)、[qoder.rs](../src-tauri/src/adapter/qoder.rs#L109-L126)、[external.rs](../src-tauri/src/adapter/external.rs#L125-L254) | 含 `***` 的邮箱（如 `***alice@example.com***`）因 `contains("***")` 直接放行明文；未遮蔽 URL/JSON 内 `token=` 字段；Windows Qoder 通过 `PATH` 调 `powershell` 且未设 `kill_on_drop`；外部插件缺回滚实现、缺 `DEVICE_DAILY_LIMIT_REACHED` 映射且 `stderr` 未脱敏 |

### 5.3 前端交互、状态一致性与兼容性

| 编号 | 问题摘要 | 位置 | 影响与触发场景 |
| --- | --- | --- | --- |
| **P2-12** | **StrictMode 杀死启动更新检查、跨日 `cstDayKey` 未生效、多处定时器未清理** | [QwitchContext.tsx](../src/context/QwitchContext.tsx#L1732-L1791)、[QwitchContext.tsx](../src/context/QwitchContext.tsx#L661)、[QwitchContext.tsx](../src/context/QwitchContext.tsx#L1101) | `updateCheckDoneRef` 在 `setTimeout` 外提前置 `true`，开发态 `StrictMode` 重挂载后更新检查永不执行；`cstDayKey` 无任何页面订阅导致跨零点统计与日志“今天/昨天”不刷新；4 处 `setTimeout` 卸载不清理 |
| **P2-13** | **`UpdateCard` 反复消耗 GitHub API 限额、未校验 `checksumsUrl`、打包完整 `package.json`** | [UpdateCard.tsx](../src/components/UpdateCard.tsx#L14)、[UpdateCard.tsx](../src/components/UpdateCard.tsx#L64-L67)、[UpdateCard.tsx](../src/components/UpdateCard.tsx#L177-L209) | 每次切换进设置页都重发 `check_app_update`（忽略启动检查结果），极易耗尽 60次/h 匿名限流；Windows 自动安装按钮未判断 `info.checksumsUrl`；整份 `package.json` 被打入生产前端包 |
| **P2-14** | **`ColdSwitchModal` / `ImportAccountModal` / `AccountCard` / `BatchClaimBanner` 交互缺口** | [ColdSwitchModal.tsx](../src/components/ColdSwitchModal.tsx#L93-L133)、[ImportAccountModal.tsx](../src/components/ImportAccountModal.tsx#L251-L545)、[AccountCard.tsx](../src/components/AccountCard.tsx#L94-L115)、[BatchClaimBanner.tsx](../src/components/BatchClaimBanner.tsx#L21-L26) | 切号弹窗存在探测竞态、重复点击无守卫、托盘失焦后残留旧确认框、384px 托盘下 3 列步骤文字严重截断；`AccountCard` 的 OAuth/重命名/停用无防重且内置工具无法走卡片补授 Token 粘贴、空桶误提示“暂无额度快照”；`BatchClaimBanner` 丢弃跨设备冷却 `summaryText` 且显示未舍入 `jitterSeconds`（如 `2.347s`） |
| **P2-15** | **`Dashboard` KPI 卡片隐藏 MiniMax、设置草稿永久锁死与 Mock 模式脱节** | [Dashboard.tsx](../src/pages/Dashboard.tsx#L702)、[Dashboard.tsx](../src/pages/Dashboard.tsx#L1762)、[Dashboard.tsx](../src/pages/Dashboard.tsx#L1871-L2003)、[mockStore.ts](../src/data/mockStore.ts#L23-L63) | KPI 摘要硬编码 `visibleTools.slice(0, 3)` 直接裁掉第 4 个工具 MiniMax；Windows 下开机自启说明仍写死 macOS LaunchAgents；未保存的抖动/日志保留草稿永久屏蔽跨窗口设置同步；数字输入框允许超 `u32` 上限；`mockStore` 与 Mock 逻辑漏掉 MiniMax |
| **P2-16** | **Linux 平台实质不可用 & Rust MSRV 未声明** | [Cargo.toml](../src-tauri/Cargo.toml#L30)、[platform/mod.rs](../src-tauri/src/platform/mod.rs#L405-L410)、[platform/mod.rs](../src-tauri/src/platform/mod.rs#L853-L857)、[trae.rs](../src-tauri/src/adapter/trae.rs#L385) | `keyring` 未启 Linux 后端退回明文 `master.key`，进程探测/关闭/可执行探测全空壳，`open_update_url` 在 Linux 直接报错；`trae.rs` 使用 `slice::as_chunks` 隐式要求 Rust ≥ 1.88，未写 `rust-version` |

### 5.4 测试覆盖与文档一致性

| 编号 | 问题摘要 | 位置 | 影响范围 |
| --- | --- | --- | --- |
| **P2-17** | **核心调度器、更新解析与前端组件测试严重缺失** | [scheduler/mod.rs](../src-tauri/src/scheduler/mod.rs)（0 测试）、[update.rs](../src-tauri/src/update.rs#L467-L544)、[src/](../src/) | 串行防风控队列、工具级熔断、跨设备冷却、API 标签解析、全部前端状态机与 UI 组件均无回归护栏，直接导致本次审查发现的多处回归 |
| **P2-18** | **`README.md` 与核心技术文档大面积过时或自相矛盾** | [README.md](../README.md#L57)、[README.md](../README.md#L89)、[README.md](../README.md#L143)、[README.md](../README.md#L184)、[ADAPTER_SPEC.md](./ADAPTER_SPEC.md)、[ARCHITECTURE.md](./ARCHITECTURE.md#L89)、[CORE_DESIGN.md](./CORE_DESIGN.md#L322-L372)、[PROJECT_COGNITION_SUMMARY.md](./PROJECT_COGNITION_SUMMARY.md#L94) | `README` 仍列已删除的 `hideDockIcon`、架构图漏 MiniMax、Rust 测试数写 51（实为 63）、旧 `picomatch` 公告已撤回、截图停留在 v0.1.0（仅 3 工具）；`ADAPTER_SPEC` 完全漏掉 MiniMax 与 Trae 多设备/zip；`ARCHITECTURE` 将 `apply_session` 误写为 `prepare_switch`；`CORE_DESIGN` 33 个命令缺 19 个、6 个事件缺 4 个；`PROJECT_COGNITION_SUMMARY` 命令数（28 vs 33）、测试数（58 vs 63）与切号轮询（15s vs 45s）均已漂移；[BRAND_IDENTITY.md](./BRAND_IDENTITY.md#L23-L25)、[BACKEND_ARCH_REVIEW.md](./BACKEND_ARCH_REVIEW.md#L3) 与 [FRONTEND_UX_AUDIT.md](./FRONTEND_UX_AUDIT.md#L3) 状态说明与代码冲突 |

---

## 6. P3 — 低危优化、架构演进与工程卫生

1. **巨型文件与重复代码治理**：
   * 后端 [commands/mod.rs](../src-tauri/src/commands/mod.rs)（2,913 行）建议按域拆分为 `accounts.rs`、`oauth.rs`、`switch.rs`、`logs.rs`、`settings.rs`、`profile.rs`；[Dashboard.tsx](../src/pages/Dashboard.tsx)（2,258 行）拆分总览、日志台、设置面板子组件。
   * 4 个适配器重复实现 `map_http_status`，且进程内存在约 6 个独立 `reqwest::Client` 连接池，应下沉共享客户端与状态映射至 [common.rs](../src-tauri/src/adapter/common.rs)；[scheduler/mod.rs](../src-tauri/src/scheduler/mod.rs#L467) 汇总文案硬编码 `"Trae CN"` 应改为泛化或读取适配器名。
2. **异步 I/O 与内存优化**：
   * [trae.rs](../src-tauri/src/adapter/trae.rs#L1797) `backup_session` 将可能达数百 MB 的 `state.vscdb` 整文件读入内存 `Vec<u8>`，建议改为磁盘临时目录硬链接/文件拷贝备份；多个适配器 `async fn` 中的同步 `std::fs`/`rusqlite`/`Command` 建议逐步迁移至 `spawn_blocking`。
   * [Dashboard.tsx](../src/pages/Dashboard.tsx#L1524-L1673) 日志勾选从 `number[]` 改为 `Set<number>` 消除 $O(N^2)$ 扫描，并考虑长列表虚拟滚动与账号计数预聚合 `Map`。
3. **依赖与 CI/CD 精简**：
   * `Cargo.lock` 同时编入 `reqwest 0.12.28`（直接依赖）与 `reqwest 0.13.5`（Tauri 传递依赖），以及多版本 `rand`/`base64`/`sha2`；建议将直接依赖 `reqwest` 对齐至 `0.13` 并收窄 `tokio` features（替代 `features = ["full"]`）；在 [ci.yml](../.github/workflows/ci.yml) 增加 `permissions: contents: read`、`cargo audit`、`--locked` 及 Tauri 构建验证。
4. **安全与密码学进阶**：
   * `uid_hash` 升级为 HMAC-SHA256、AES-GCM 绑定 `account_id`/`tool_id` AAD、`master_key` 及解密结构体引入 `zeroize` Drop 清理；CSP 增补 `form-action 'self'`。
5. **仓库冗余资产与 A11y 微调**：
   * 清理未被引用的旧品牌文件 `docs/assets/tuckbase-*`、`docs/assets/qwitch-icon-direction-01*`、未使用移动端图标 `src-tauri/icons/{android,ios}`（约 1.05 MB）及本地未跟踪 `src-tauri/examples/.DS_Store`；统一清除近期回潮的 5 处 `text-[10px]`；补齐侧栏 `aria-current="page"`、日志复选框 `role="checkbox"`/`aria-checked`、错误 Toast `role="alert"` 及切换中弹窗焦点保持。

---

## 7. 分阶段优先处理路线与实施进度

### 第一阶段：止血修复（发布 `v0.2.4` 前必做）

| 序号 | 对应问题 | 修复内容摘要 | 当前状态 |
| :---: | --- | --- | --- |
| **1** | **P0-1** | 修复 `update.rs` `trim_start_matches(['v', 'V'])` 解析，收紧 `github.com` 官方仓库前缀与 Release CDN 精确域名白名单，下载重定向逐跳校验与 64 位 SHA-256 格式校验 | 已完成 |
| **2** | **P1-9** | 加固 `release.yml`：Tag 与版本一致性校验、补全 `fmt`/`clippy`/`npm audit`/`--locked`、最小权限、防覆盖已有正式 Tag 与 Release 说明 | 已完成 |
| **3** | **P0-4 / P1-2 / P1-3** | Trae 空路径防误改本机、跨 OS 删除 `os_version`、切号过滤外部 `icube-dc:`、放行 0 余额账号；纯 Rust `zip` 安全解压 + `TempDirGuard` + `spawn_blocking` + 前端校验与错误展示 | 已完成 |
| **4** | **P0-6 / P1-1 / P1-4** | MiniMax 清空模板 `tokens`/`user`、传播 401/403/429、严格 Fail-Closed、纳入 `dailyResetTool`；WorkBuddy 200 非 JSON 拒绝；统一领后状态合成与导入查额前刷新令牌 | 已完成 |
| **5** | **P0-2 / P0-3** | 外部插件目录名与 ID 强校验 + 内容哈希固定（`trustedAdapterHashes`）+ 环境隔离；外部插件与 WorkBuddy 解密 `stdin.write_all` 纳入异步超时并发任务 | 已完成 |

> **第一阶段回归验证记录**：
> - 前端：`npm test -- --run`（2 个测试文件、21 条用例全部通过）、`npx tsc --noEmit && npm run build` 退出码 `0`。
> - 后端：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings` 均零告警通过；`cargo test --all-targets`（`qwitch_lib` 73 条用例全部通过，新增覆盖 10 条 Stage 1 回归测试）。

### 第二阶段：防风控与核心体验加固（已完成）

| 序号 | 对应问题 | 修复内容摘要 | 当前状态 |
| :---: | --- | --- | :---: |
| **1** | **P0-5 & P1-5** | 为 `ToolAdapter` 新增纯本地只读 `detect_local_active_uid`，`reconcile_active_accounts` 改为 `spawn_blocking` + 3s 原子节流并在 `clear_active_on_client` 后广播事件；完善旧钥匙串条目验证失败回退探测及 macOS Keychain 升级恢复引导 | 已完成 |
| **2** | **P2-7 & P2-8 & P2-9** | 新增 `write_private_file_atomic`（Unix `0600` 原子创建写入）并覆盖各适配器；`restore_files` 主库失败保留 `-wal` 并加 `busy_timeout(2000)`；`switch_desktop_account` 与调度队列共用锁互斥，移除延迟清标志窗口并用事件时间戳替代；`platform` 层改由内存缓存与 `Once` 迁移、`terminate_client_gracefully` 统一清理子进程、macOS 多开传 `open -n` | 已完成 |
| **3** | **P1-8 & P2-1 ~ P2-6 & P2-10 ~ P2-11** | 修复 `scripts/*.sh` 权限/退出码/`curl -K`/403/429 熔断；补全各适配器 `without_url()`、`validate_endpoint`、`poll_oauth_result` 故障熔断、MiniMax `_p_uid`/RFC 8628 轮询、`Trae` 已登出残留识别与 `aha/TinyStorage` 对齐、`Qoder` `auth.machine-id`/多活动状态/PowerShell 绝对路径、`common` UID 与分句幂等检查、`external` `stderr` 脱敏、`crypto` 伪脱敏邮箱与 `Authorization` scheme-preserving KV 脱敏、清理 `LocalAccountPreview` 明文 `uid` | 已完成 |
| **4** | **P1-6 & P1-7 & P2-12 ~ P2-15** | 日志改为 `before_id` 游标分页（上限 `2000`）与竞态序号丢弃、`archiveLogs`/`unarchiveLogs`/`deleteLogs` 返回 `number \| null` 避免失败误清勾选；移除 `ToolBrandIcon` 的 Context 订阅并用 `React.memo` 优化；`vite.config.ts` 注入 `__APP_VERSION__` 并复用全局 10 分钟更新检查缓存与 Windows `checksumsUrl` 门禁；修复 `StrictMode` 启动更新检查、批量乐观锁与跨日 `cstDayKey` 刷新；加固 `ColdSwitchModal`/`TrayPopover`/`ImportAccountModal`/`AccountCard`/`BatchClaimBanner`/`Dashboard`（全部 4 个内置工具 KPI、平台感知自启文案、设置离开重置草稿与 `mockStore` MiniMax 对齐） | 已完成 |

> **第二阶段回归验证记录**：
> - 脚本：`bash -n scripts/export_trae_profile.sh scripts/check_device_isolation.sh scripts/trae_probe.sh` 退出码 `0`。
> - 前端：`npm test -- --run`（2 个测试文件、21 条用例全部通过）、`npx tsc --noEmit` 与 `npm run build` 退出码 `0`。
> - 后端：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings` 均零告警通过；`cargo test --all-targets`（`qwitch_lib` 76 条用例全部通过，`qwitch` 二进制 0 条测试）。

### 第三阶段：测试护栏、架构拆分与文档收敛（已完成）
1. **补齐测试基线**：为 [scheduler/mod.rs](../src-tauri/src/scheduler/mod.rs) 补齐 Mock Adapter 行为测试（串行、抖动、跨设备冷却、熔断、幂等），补全组件集成测试与 Vitest 覆盖率。
2. **文档与资产对齐**：一次性同步 `README.md`、`CORE_DESIGN.md`、`ADAPTER_SPEC.md`、`ARCHITECTURE.md` 与 `PROJECT_COGNITION_SUMMARY.md`，补全 `mockStore` 中的 MiniMax 并重截 5 张展示图，清理无用旧图标资产。
3. **架构拆解与依赖收敛**：拆分 `commands/mod.rs`、`QwitchContext.tsx`、`Dashboard.tsx`，升级对齐 `reqwest 0.13` 并收窄 `tokio` features。


**第三阶段落地记录**：
- 后端：`commands/mod.rs`（原 3,261 行）拆为 `mod.rs` + `accounts/profile/settings/switch/logs/oauth` 6 个子模块；`scheduler/mod.rs` 新增 `SchedulerEventSink` 与 5 个行为测试（幂等/Fail-Closed、工具级熔断、设备日限、凭据轮换与限流熔断、队列互斥）；`adapter/common.rs` 收敛共享 HTTP 客户端与状态映射；`Cargo.toml` 设 `rust-version = "1.88"` 并收窄 `tokio` features（`reqwest` 保持 0.12 以沿用 `ring`，避免 Windows CI 引入 `aws-lc-rs` 构建依赖）；CSP 增加 `form-action 'self'`；CI 增加 `permissions: contents: read`、`bash -n` 脚本检查与 `--locked`；Linux 增加 `xdg-open` 更新入口。
- 前端：`Dashboard.tsx` 由 2,341 行降至约 1,178 行，日志/设置视图拆至 `pages/dashboard/{LogsView,SettingsView,types}`；抽出 `context/logStateUtils.ts`；日志勾选改用 `Set` 的 O(1) 判定并补 `role="checkbox"`；侧栏 `aria-current="page"`；工具账号计数改用 `Map`；`ToastViewport` 错误用 `alert`、其余用 `status`；`text-[10px]` 全部提升为 `text-[11px]`；`mockStore` 的 MiniMax 与后端对齐。
- 资产：删除未被引用的 `docs/assets/tuckbase-*`、`docs/assets/qwitch-icon-direction-01*`、`src-tauri/icons/{android,ios}`。
- 仍保留的后续项：`QwitchContext.tsx` 多 store 拆分与增量同步、`storage/mod.rs` 拆分、组件级（DOM）集成测试、Tauri bundle CI 构建校验、展示图重截。
