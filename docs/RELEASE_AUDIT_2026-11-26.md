# v0.7.0 发版前深度审查（2026-11-26）

审查对象：`main` @ 本轮改动（模型方案复用 / 豆包多账号 / 切号稳定性 / 额度口径）。
方法：读实际代码与配置 + 静态检查 + 真机运行验证 + 流水线模拟，不依赖"应该没问题"的判断。

门禁基线（全部通过）：

| 项 | 结果 |
| --- | --- |
| `cargo fmt --check` / `cargo clippy --all-targets -D warnings` | 通过 |
| `cargo test` | **293 passed**（含新增批量命令的 ACL 校验） |
| `npx tsc --noEmit` / `npm run lint` | 0 告警 |
| `npm test` | **187 passed / 30 files** |
| `npm run build` | 395.9 KB（gzip 105.5 KB） |
| `npm audit --omit=dev` | 0 vulnerabilities |
| 发布校验模拟（tag=v0.7.0） | 版本三处一致 + CHANGELOG 段落存在 |

---

## 一、发版阻断项（必须在打 tag 前处理）

### B-1 版本号与 CHANGELOG 段落必须同时对齐

- **等级**：阻断（P0）
- **涉及**：`package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、
  `src-tauri/tauri.conf.json`、`CHANGELOG.md`、`.github/workflows/release.yml`
- **触发条件**：打 tag 时三处版本号与 tag 不一致；或 `CHANGELOG.md` 缺少 `## [<版本>]` 段落
- **影响**：`verify` job 直接失败，不会产出任何安装包（属于"安全失败"，不会发错版本）
- **处理**：本次已统一为 **0.7.0** 并写入 `## [0.7.0]` 段落（模拟校验通过）。
  若最终决定用 0.6.4，只需同步改这 5 个文件 + CHANGELOG 标题。
- **验证**：本地执行 release.yml 里的两段校验脚本（已执行，通过）

---

## 二、需要在本版修掉的高优先级问题

### H-1 余额型额度的「按额度排序」失效（已修复）

- **等级**：高（P1，功能退化）
- **涉及**：`src-tauri/src/storage/pools.rs::format_quota_stats`
- **触发条件**：对 LobsterAI（跨池积分）这类 `hasTotal = false` 的账号点「按额度排序」
- **影响**：`quota_percent` 恒为 100，排序退化为原始顺序；多账号时无法挑出余额最多的那个
- **建议**：候选结构补 `quota_remaining`，排序优先用 `hasTotal ? percent : -remaining`；
  或在 `format_quota_stats` 内按「剩余额度相对全池最大值」折算百分比（仅用于排序展示）
- **验证**：新增单测覆盖两条分支；真机上对龙虾两个账号点排序看顺序

### H-2 批量「全部订阅」写命令放大（已修复）

- **等级**：高（P2，性能）
- **涉及**：`src/context/usePoolPrototypeStore.tsx`（`copyAgentAllToolModelPlans` /
  `syncAllToolModelPlansToAllAgents`）
- **触发条件**：Agent 数 × 已配置订阅数较大时（实测组合可达 6 × 4 = 24 条）
- **影响**：每条 `set_agent_tool_models_cmd` 都重建一次 workspace（约 36ms + ~200KB 回传），
  一次性 20+ 条会造成 ~1s 后台负载与数 MB 序列化；弱机上可感知卡顿
- **建议**：新增 `set_agent_tool_models_batch_cmd`（一次落库 + 一次接线 + 一次回传），
  前端批量路径改走它；单条路径保持不变
- **验证**：DevTools Performance 计时；后端命令计数断言（1 条而非 N 条）

### H-3 豆包「关联本机」新增网络依赖（已修复）

- **等级**：高（P2，可用性）
- **涉及**：`src-tauri/src/adapter/doubaowork.rs::import_from_local`
- **触发条件**：客户端 `Preferences.saman.last_user` 为空（刚登出/刚切换），且机器离线或
  passport 接口不可达
- **影响**：关联本机失败；错误文案写成「Cookie 校验也失败」，用户会误以为是凭据问题
- **建议**：区分 `NetworkError` 与凭据错误——网络错误提示「本机 Cookie 已读到但校验需要联网，
  请检查网络后重试」，并允许 `preview_local_account` 返回"身份待确认"而不是硬失败
- **验证**：断网后点关联本机，确认提示语准确；恢复网络后成功

### H-4 Windows 侧本轮改动未在本地编译验证（无法本地消除，依赖 CI）

- **等级**：高（P2，风险项）
- **涉及**：`src-tauri/src/adapter/doubaowork.rs`、`platform/mod.rs`（新增 `cfg(macos)` 分支）、
  `adapter/safe_storage.rs`
- **触发条件**：Windows 构建
- **影响**：若某个 cfg 分支在 Windows 下引用了仅 macOS 存在的变量，会编译失败（CI 能拦住，
  但会浪费一次发版窗口）
- **建议**：发版前在 CI 里先跑 `cargo check --target x86_64-pc-windows-msvc`（或直接看
  release 的 Windows job）；本地无法验证这一点，已如实标注
- **验证**：GitHub Actions `build (windows-latest)` 通过

---

## 三、中低优先级问题（可排到 0.7.1）

| 编号 | 等级 | 问题 | 位置 | 触发条件 / 影响 | 建议 |
| --- | --- | --- | --- | --- | --- |
| M-1 | P3 | `restore_session` 先清空**所有** profile 的 LevelDB/WAL 再还原 | `adapter/doubaowork.rs` | 备份之后新建的 profile 不在备份中，其 WAL 被删，客户端需重新加载该账号本地缓存 | 只清理「备份中存在」的 profile，或清理前记录受影响的 profile 集合 |
| M-2 | P3 | 常驻子进程按名字子串 `pkill -f openclaw-gateway` | `platform/mod.rs::stop_mac_sidecar_processes` | 同名进程（非本客户端）会被一并终止，概率极低 | 匹配 `--openclaw-state` 参数或校验进程 cwd 落在客户端数据目录 |
| M-3 | P3 | 「明确登出」判定只实现了豆包 | `adapter/doubaowork.rs`、`adapter/mod.rs` | 其它客户端登出后仍可能停留在「本机使用中」 | 按各客户端同类标记逐个实现（Trae/Qoder 有各自登录态） |
| M-4 | P3 | `Local State` 写回权限固定 0600 | `adapter/doubaowork.rs::set_active_profile` | 若该文件原带自定义 ACL，会被重置；同用户读取无影响 | 保留原文件权限（先 stat 再 chmod） |
| M-5 | P3 | `ps` 不可用时 fail-open | `platform/mod.rs` | 极端残留 helper 会被判「运行中」，切号多等 ~3s | 已可接受；后续可用 `proc_pidpath`（libproc）替代 ps 做精确判定 |
| M-6 | P3 | 豆包快照体积接近上限时无指引 | `adapter/doubaowork.rs` | 多账号 + 大量 SDK 存储时可接近 80 MiB，超限只报「超出上限」 | 报错里给出「哪些目录占体积」的摘要 |
| M-7 | P3 | 历史 GitHub Release 说明只有下载指引 | GitHub Releases v0.6.0–v0.6.3 | 应用内「更新说明」展示的就是 release body，用户看不到修了什么 | 已补齐 CHANGELOG；如需回溯，可把对应段落回填到历史 release |

---

## 四、安全与权限核查

| 项 | 结论 |
| --- | --- |
| 新增解密逻辑 | `adapter/safe_storage` 只做 AES-128-CBC 信封处理，密钥来自 Keychain，明文不落盘、不进日志 |
| 凭据落库 | 仍为 AES-256-GCM（`crypto::encrypt_credential`），新增字段 `profile_dir.txt`、`session_snapshot_b64` 都在同一密文内 |
| 快照写入白名单 | 从写死 `Default/...` 改为「`Default` 或 `Profile N` + 固定文件名」，仍拒绝 `..`/绝对路径/非白名单（zip-slip 测试通过） |
| IPC 权限 | 删除 `set_account_manual_models_cmd` 后同步移除 `capabilities/default.json` 中的 allow 项；`command_acl` 测试通过 |
| 依赖 | `npm audit --omit=dev` 0 漏洞；Rust 侧未新增依赖（`aes`/`cbc`/`pbkdf2` 原已存在） |
| 审计可观测性 | **缺口**：读取客户端 Cookie 属敏感操作，但未写 action_log。建议加一条脱敏事件（tool + 时间，不含值） |
| 日志脱敏 | 新增错误文案只带工具名/uid 掩码；`crypto::sanitize_log_message` 仍在写入前统一过滤 |

---

## 五、GitHub 版本更新记录现状

检查命令与结论：

```bash
gh release list --limit 5          # v0.6.0 … v0.6.3 均存在
gh release view v0.6.3 --json body # 仅「下载说明」+ GitHub 自动 Full Changelog
```

- **确实缺少变更说明**：历史 4 个 release 的 body 都只有下载/签名说明，没有「新增 / 修复 /
  破坏性变更」；而应用内更新提示读取的正是 `release.body`（`update.rs` →
  `release_notes`），等于用户升级时看不到任何变更内容。
- **已补**：新增 `CHANGELOG.md`（Keep a Changelog 格式，回填 0.6.0–0.6.3，本次 0.7.0 完整分节）。
- **已接流水线**：
  1. `verify` job 新增「CHANGELOG 必须有对应 tag 段落」，缺失直接失败（避免发一个没有说明的版本）；
  2. `publish` job 从 CHANGELOG 抽取该版本段落 → 拼接下载说明 → 写入 Release body；
  3. 重跑发布（已有 release）也会用 `--notes-file` 刷新说明，不再出现"旧说明覆盖不掉"；
  4. 末尾自动附 `compare/<上一 tag>...<新 tag>` 链接。
- **本地已验证**：段落抽取对 0.7.0 / 0.6.3 均正确，不存在的版本返回空（触发失败分支）。

---

## 六、发版前检查清单

```bash
# 1) 门禁
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cd .. && npx tsc --noEmit && npm run lint && npm test && npm run build && npm audit --omit=dev

# 2) 版本与变更记录
grep -m1 '"version"' package.json src-tauri/tauri.conf.json
grep -m1 '^version' src-tauri/Cargo.toml
grep -nE '^## \[0\.[0-9]+\.[0-9]+\]' CHANGELOG.md | head -3

# 3) 打 tag（触发发布）
git tag -a v0.7.0 -m "Qwitch v0.7.0" && git push origin v0.7.0
```

人工验收（发版前真机过一遍）：

1. 豆包工作：两个账号在客户端内互切后，对当前账号点「关联本机」应成功；切号 2 秒内完成。
2. 码道：本机导入过的账号出现「切至桌面端」，切号后客户端自动登录同一账号。
3. 有道龙虾：连续切号 3 次，客户端不再弹「AI 引擎网关启动失败」。
4. 额度：豆包显示「2123 / 2142」，龙虾显示「可用积分 361」且无百分比进度条。
5. 模型方案：新挂载 Agent →「复制方案 → 某 Agent · 全部 N 个订阅」一次到位。
6. 运行状态：托盘各工具「运行中/未运行」与实际一致（打开/关闭客户端各看一次）。

---

## 六点五、本轮修复记录（对应上文编号）

| 编号 | 修复内容 | 改动位置 | 验证 |
| --- | --- | --- | --- |
| H-1 | 新增 `quota_sort_value`（有总量=剩余百分比、余额型=绝对剩余），`format_quota_stats` 旁统一计算；候选与成员透传；前端「按额度排序」改用该值 | `storage/pools.rs`、`domain/pools.rs`、`src/types/pool.ts`、`usePoolPrototypeStore.tsx` | `cargo test` 全绿；排序单测按新字段比较（余额型不再全部并列 100） |
| H-2 | 新增 `set_agent_tool_models_batch_cmd`：一次落库 + 一次接线 + 一次回传，上限 16 条；前端「全部订阅」改走它，并为每条方案附带默认模型 key | `commands/pools.rs`、`lib.rs`、`build.rs`、`capabilities/default.json`、`usePoolPrototypeStore.tsx` | `command_acl` 测试通过；批量路径由 N 条降为 1 条命令（单条路径逻辑不变） |
| H-3 | 关联本机区分 `NetworkError` 与凭据错误：网络不可用时提示「已读到 Cookie，校验需要联网」，不再误报登录态失效 | `adapter/doubaowork.rs` | 断网场景文案正确（代码分支覆盖） |
| M-1 | 回滚时只清理「备份中确实存在」的 profile，避免误清备份后新建 profile 的 WAL/LevelDB | `adapter/doubaowork.rs::restore_session` | 由 `multi_profile` 相关测试与代码审查覆盖 |
| M-4 | 写回 `Local State` 后恢复原文件权限（不再统一收敛成 0600） | `adapter/doubaowork.rs::set_active_profile` | 代码审查 + 真机切号（权限保持 0644） |
| M-6 | 快照超限报错带实际体积与「清理客户端缓存」指引 | `adapter/doubaowork.rs` | 代码审查 |
| M-7 | 回填 v0.6.0–v0.6.3 的 GitHub Release 说明（变更记录 + 原下载说明），并接入后续自动生成 | GitHub Releases、`CHANGELOG.md`、`release.yml` | `gh release view` 已确认四个版本均有「新增/修复」段落 |

**明确不修（附理由）**：

- **M-2（sidecar 按名字子串匹配）**：实测 `openclaw-gateway` 的工作目录在
  `/Applications/LobsterAI.app/Contents/Resources/cfmind`，按 cwd 校验会直接漏杀；
  改成「检查进程打开的文件是否在数据目录下」需要逐 pid 跑 `lsof`（受限环境可能不可用），
  误判风险反而更高。当前名字足够产品专属，保留现状并在代码注释说明。
- **M-3（其它客户端的「明确登出」判定）**：需要各家客户端各自的登出标记（Trae/Qoder
  自定义存储），在拿到真实样本前实现等于猜；保持默认 `None`（绝不误清）。
- **M-5（`ps` fail-open 的极端误判）**：代价仅为多等 ~3s，且换用 `libproc`
  属于平台层重构，放到 0.7.1 评估。

## 七、已核查但未发现问题的范围

- 存储层：`upsert_account` / `bind_account_credential` 的合并逻辑、`uid_hash` HMAC 匹配、
  旧快照反序列化（`hasTotal` 默认 true，已加回归测试）。
- 网关：`QuotaBucket` 变更不影响网关路由（网关只消费账号与模型清单）。
- 前端：`OverflowMenu` 的 `display`/`disabled`、`AccountCard` 的 `hasTotal` 分支、
  批量复制后的默认模型归属（单测覆盖）。
- 依赖与供应链：生产依赖 0 漏洞；发布脚本只使用 GitHub 官方 action 与 `gh`。

## 八、剩余风险（无法在本地消除）

1. **Windows 实机行为**：本次 `cfg(macos)` 之外的改动（额度、前端）跨平台，但未在 Windows
   真机验证；以 CI 构建 + 用户回归为准。
2. **签名与公证**：macOS 仍为 ad-hoc 签名，未启用 Apple Developer 证书；用户首次打开仍需
   `xattr` 处理（已知限制，Release 说明中已写明）。
3. **客户端私有协议**：豆包多 profile、龙虾 `openclaw-gateway` 的行为均来自实测；客户端升级
   可能改变目录结构/进程名，需要在每次客户端大版本后回归本清单第 1–3 项。
