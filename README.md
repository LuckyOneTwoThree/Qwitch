# Qwitch

> 本地优先（Local-First）的 AI 工具多账号会话与每日额度管理桌面应用。

**Qwitch**（**Q**uota + S**witch**）帮助开发者在本机有序管理多个 AI 开发与办公工具账号，查看额度水位、手动领取每日积分，并将选定账号切换到本地桌面客户端。

---

## 核心理念与边界

* **本地优先（Local-First）**：所有账号凭据、会话快照、额度记录与操作日志均 100% 存储于用户本机，零云端服务器、零遥测上报。
* **手动可控（Human-in-the-Loop）**：坚持由用户主动触发查询、领取与切号动作，拒绝后台高频静默轮询，最大程度规避平台风控。
* **安全存储**：主密钥优先托管于系统原生钥匙串（macOS Keychain / Windows Credential Manager），钥匙串不可用时降级为受限权限（0600）本地密钥文件；启动时以密钥指纹校验候选密钥，业务与凭据数据本地加密落盘。
* **可审计开源**：基于 [Tauri v2](https://v2.tauri.app/)（Rust + TypeScript）构建，双平台原生轻量化体验，代码透明可审查。

---

## 首批支持与规划范围

| 工具 | 登录态获取 | 每日领取 / 额度 | 本地客户端切换 |
| --- | --- | --- | --- |
| **Trae CN** | 内置 WebView 登录 / 本机导入 / Token | 额度、每日签到与套餐水位监控 | 一键冷切换（后续扩展多开隔离） |
| **WorkBuddy** | 本机凭据快照导入 / 内置授权 / Token | 手动一键签到与成长中心领取 | 一键冷切换 |
| **Qoder CN** | 本机导入 / 内置授权 / PAT | 手动一键领取每日 Credits | 一键冷切换（后续扩展多开隔离） |

---

## 核心特性

* **双态交互体验**：
  * **系统托盘（Tray / Menu Bar）**：高频轻量入口，随时查看各工具当前活跃账号与剩余额度，支持一键切号与快速触发领取。
  * **主窗口（Dashboard）**：极简卡片式管理面板，支持浅色/深色主题切换（首帧跟随系统偏好），集中管理账号分组、状态诊断、额度进度与本地日志。
* **内置防风控调度**：一键批量领取采用单队列串行 + 随机抖动延迟（Jitter），支持幂等前置校验（已签自动跳过）与异常熔断。
* **模块化与外部扩展**：核心采用 Rust Adapter Trait，并已实现基于标准 JSON-RPC 2.0 的外部适配器扩展协议；外部插件需在设置中显式信任后启用（变更重启生效），内置与保留 ID 禁止冒名。

---

## 平台支持

* **macOS**（Apple Silicon & Intel）
* **Windows**（x64）

---

## 项目文档

* [架构与基线设计文档](docs/ARCHITECTURE.md) — 项目定位、核心架构、适配器协议、防风控规范与分期路线图。
* [核心接口契约与数据模型](docs/CORE_DESIGN.md) — 领域模型、Rust Trait、外部 JSON-RPC 协议与 SQLite Schema。
* [内置适配器参考规范](docs/ADAPTER_SPEC.md) — 各工具本地存储路径、加密格式与接口细节。
* [品牌定位与命名规范](docs/BRAND_IDENTITY.md) — 唯一主品牌 **Qwitch** 的定位、命名规范与视觉原则。
* [前端体验与交互审计报告](docs/FRONTEND_UX_AUDIT.md) — 前端 UI/交互/无障碍/响应式审计定稿及专项优化计划。
* [代码安全与架构审查报告](docs/BACKEND_ARCH_REVIEW.md) — Rust 后端安全、可靠性、性能审查定稿及专项加固计划。
* [综合优化计划与任务排期](docs/OPTIMIZATION_ROADMAP.md) — 前后端拉通对齐的四阶段 Sprint 排期与接口契约变更总表。

---

## 运行与开发

**前置条件**：Node.js ≥ 22、Rust stable（含 `clippy` / `rustfmt` 组件）、macOS 或 Windows 桌面环境（Tauri v2 依赖系统 WebView）。

```bash
npm install          # 安装前端依赖
npm run tauri dev    # 启动桌面应用（开发模式）
npm run build        # 前端类型检查 + 生产构建
npm test             # 前端单元测试（Vitest）
cd src-tauri && cargo test   # 后端单元/回归测试
```

CI（GitHub Actions，macOS + Windows 双平台）执行：生产依赖审计、前端构建与测试、`cargo fmt --check`、`cargo clippy -D warnings`、`cargo test`。

> 开发链依赖中 picomatch 存在上游尚未发布修复版的 ReDoS 公告（仅影响 dev server/测试运行器，不进生产产物），生产依赖审计为 0 漏洞。

## 许可证

[MIT](LICENSE)

---

## 免责声明

本项目仅用于辅助个人管理本人合法持有的多账号本地会话与额度信息，严禁用于批量注册、自动化薅羊毛、绕过计费或任何违反目标平台用户协议的行为。使用本工具所产生的账号异常或合规风险由使用者自行承担。
