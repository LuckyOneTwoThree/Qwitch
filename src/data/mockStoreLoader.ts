import type { AccountSummary, ActionLogItem, AdapterManifest } from "../types/qwitch";
import { INITIAL_ACCOUNTS, INITIAL_LOGS, MOCK_ADAPTERS } from "./mockStore";

/**
 * Mock 数据的受控入口。
 *
 * 背景：`mockStore` 曾被 `useAccountStore` / `useLogStore` 直接静态 import 并使用，
 * 导致整份假账号与假日志被打进生产 bundle（实测可在 `dist/assets/*.js` 中检索到
 * `acc-minimax-1`、`主力工作号` 等字样）。这不只是包体问题：一旦 Tauri 运行时
 * 探测（`__TAURI_INTERNALS__`）失效，界面会展示假账号，用户点击「领取」会走
 * mock 的 `sleep + bonus` 假成功路径，产生假 Toast 与假日志。
 *
 * 现在的做法：
 * - 所有消费方统一经由本模块取值；
 * - `import.meta.env.DEV` 为 `false` 时（生产构建），函数恒返回空集合，
 *   调用方因此只可能拿到空列表，绝不会拿到假账号；
 * - 生产环境即使探测失效，表现是「空列表」而非「假数据 + 假成功」。
 *
 * 说明：这里仍保留对 `mockStore` 的静态 import，是因为 Vite 对 `import.meta.env.DEV`
 * 常量折叠后，若把引用移到守卫内做动态 `import()` 会引入异步首帧空白。当前守卫
 * 已消除「假数据被当作真实数据使用」这一实际风险；包体层面的进一步裁剪（把
 * mockStore 移出生产包）需要异步化初始化，属于更大的改动，留待后续评估。
 */

const DEV = import.meta.env.DEV;

const EMPTY_ACCOUNTS: AccountSummary[] = [];
const EMPTY_LOGS: ActionLogItem[] = [];
const EMPTY_ADAPTERS: AdapterManifest[] = [];

export function getMockAccounts(): AccountSummary[] {
  return DEV ? INITIAL_ACCOUNTS : EMPTY_ACCOUNTS;
}

export function getMockLogs(): ActionLogItem[] {
  return DEV ? INITIAL_LOGS : EMPTY_LOGS;
}

export function getMockAdapters(): AdapterManifest[] {
  return DEV ? MOCK_ADAPTERS : EMPTY_ADAPTERS;
}
