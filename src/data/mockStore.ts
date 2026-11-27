import type { AccountSummary, ActionLogItem, AdapterManifest } from "../types/qwitch";

const CST_OFFSET_MS = 8 * 60 * 60 * 1000;

function cstTimestamp(daysAgo: number, hour: number, minute: number): string {
  const nowCst = Date.now() + CST_OFFSET_MS;
  const d = new Date(nowCst);
  const utcDayStart = Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()) - CST_OFFSET_MS;
  let targetMs = utcDayStart + ((-daysAgo * 24 + hour) * 60 + minute) * 60_000;
  if (daysAgo === 0 && targetMs > Date.now()) {
    targetMs = Math.max(utcDayStart + 60_000, Date.now() - 5 * 60_000);
  }
  return new Date(targetMs + CST_OFFSET_MS).toISOString().replace(/\.\d{3}Z$/, "+08:00");
}

function nextMonthResetCst(): string {
  const d = new Date(Date.now() + CST_OFFSET_MS);
  const y = d.getUTCMonth() === 11 ? d.getUTCFullYear() + 1 : d.getUTCFullYear();
  const m = String(((d.getUTCMonth() + 1) % 12) + 1).padStart(2, "0");
  return `${y}-${m}-01T00:00:00+08:00`;
}

export const MOCK_ADAPTERS: AdapterManifest[] = [
  {
    id: "trae_cn",
    name: "Trae CN",
    version: "0.1.0",
    isBuiltin: true,
    capabilities: {
      authMethods: ["local_import", "webview_oauth", "token_paste"],
      supportsQuotaQuery: true,
      supportsDailyClaim: true,
      supportsClientSwitch: true,
      supportsMultiInstance: false,
    },
  },
  {
    id: "workbuddy",
    name: "WorkBuddy",
    version: "0.1.0",
    isBuiltin: true,
    capabilities: {
      authMethods: ["local_import", "webview_oauth", "token_paste"],
      supportsQuotaQuery: true,
      supportsDailyClaim: true,
      supportsClientSwitch: true,
      supportsMultiInstance: false,
    },
  },
  {
    id: "qoder_cn",
    name: "Qoder CN",
    version: "0.1.0",
    isBuiltin: true,
    capabilities: {
      authMethods: ["local_import", "webview_oauth", "token_paste"],
      supportsQuotaQuery: true,
      supportsDailyClaim: true,
      supportsClientSwitch: true,
      supportsMultiInstance: false,
    },
  },
  {
    id: "minimax_code_cn",
    name: "MiniMax Code CN",
    version: "0.1.0",
    isBuiltin: true,
    capabilities: {
      authMethods: ["local_import", "webview_oauth", "token_paste"],
      supportsQuotaQuery: true,
      supportsDailyClaim: true,
      supportsClientSwitch: true,
      supportsMultiInstance: false,
    },
  },
  {
    id: "lobsterai",
    name: "LobsterAI",
    version: "0.1.0",
    isBuiltin: true,
    capabilities: {
      authMethods: ["local_import", "webview_oauth", "token_paste"],
      supportsQuotaQuery: true,
      supportsDailyClaim: true,
      supportsClientSwitch: true,
      supportsMultiInstance: false,
    },
  },
  {
    id: "doubaowork",
    name: "豆包工作",
    version: "0.1.0",
    isBuiltin: true,
    capabilities: {
      authMethods: ["local_import", "webview_oauth", "token_paste"],
      supportsQuotaQuery: true,
      supportsDailyClaim: true,
      supportsClientSwitch: true,
      supportsMultiInstance: false,
    },
  },
  {
    id: "codearts",
    name: "码道 CodeArts",
    version: "0.1.0",
    isBuiltin: true,
    capabilities: {
      // 额度监控 + 每日签到（/v1/ops/* 纯服务端接口）
      authMethods: ["local_import", "token_paste"],
      supportsQuotaQuery: true,
      supportsDailyClaim: true,
      supportsClientSwitch: false,
      supportsMultiInstance: false,
    },
  },
];

export const INITIAL_ACCOUNTS: AccountSummary[] = [
  {
    id: "acc-trae-1",
    toolId: "trae_cn",
    label: "Trae CN · 主力工作号",
    uidMasked: "u_8f3a****91c",
    emailMasked: "lu***@gmail.com",
    status: "active",
    isActiveOnClient: true,
    deviceIdMasked: "9390****7536",
    deviceType: "mac",
    latestQuota: {
      planName: "Pro",
      claimStatus: "claimed",
      fetchedAt: cstTimestamp(0, 10, 40),
      buckets: [
        {
          key: "fast_requests",
          label: "快速请求额度",
          used: 220,
          total: 600,
          remaining: 380,
          unit: "次",
          resetAt: nextMonthResetCst(),
        },
      ],
    },
    updatedAt: cstTimestamp(0, 10, 40),
  },
  {
    id: "acc-trae-2",
    toolId: "trae_cn",
    label: "Trae CN · 个人备用号",
    uidMasked: "u_4c19****02b",
    emailMasked: "de***@outlook.com",
    status: "active",
    isActiveOnClient: false,
    deviceIdMasked: "1102****88aa",
    deviceType: "mac",
    latestQuota: {
      planName: "Free",
      claimStatus: "available",
      fetchedAt: cstTimestamp(0, 9, 10),
      buckets: [
        {
          key: "fast_requests",
          label: "快速请求额度",
          used: 58,
          total: 100,
          remaining: 42,
          unit: "次",
          resetAt: nextMonthResetCst(),
        },
      ],
    },
    updatedAt: cstTimestamp(0, 9, 10),
  },
  {
    id: "acc-trae-3",
    toolId: "trae_cn",
    label: "Trae CN · 早期测试号",
    uidMasked: "u_1d07****77e",
    emailMasked: "te***@proton.me",
    status: "expired",
    isActiveOnClient: false,
    latestQuota: {
      planName: "Free",
      claimStatus: "available",
      fetchedAt: cstTimestamp(5, 10, 0),
      buckets: [
        {
          key: "fast_requests",
          label: "快速请求额度",
          used: 100,
          total: 100,
          remaining: 0,
          unit: "次",
        },
      ],
    },
    updatedAt: cstTimestamp(5, 10, 0),
  },
  {
    id: "acc-wb-1",
    toolId: "workbuddy",
    label: "WorkBuddy · 主号",
    uidMasked: "wb_72a1****8e",
    emailMasked: "wo***@qq.com",
    status: "active",
    isActiveOnClient: true,
    latestQuota: {
      planName: "Plus",
      claimStatus: "available",
      fetchedAt: cstTimestamp(0, 10, 30),
      buckets: [
        {
          key: "credits",
          label: "可用积分",
          used: 550,
          total: 2000,
          remaining: 1450,
          unit: "积分",
        },
      ],
    },
    lastClaimedAt: cstTimestamp(1, 9, 12),
    updatedAt: cstTimestamp(0, 10, 30),
  },
  {
    id: "acc-wb-2",
    toolId: "workbuddy",
    label: "WorkBuddy · 副号",
    uidMasked: "wb_39f0****14",
    emailMasked: "si***@foxmail.com",
    status: "active",
    isActiveOnClient: false,
    latestQuota: {
      planName: "Free",
      claimStatus: "claimed",
      fetchedAt: cstTimestamp(0, 8, 20),
      buckets: [
        {
          key: "credits",
          label: "可用积分",
          used: 380,
          total: 1000,
          remaining: 620,
          unit: "积分",
        },
      ],
    },
    lastClaimedAt: cstTimestamp(0, 8, 20),
    updatedAt: cstTimestamp(0, 8, 20),
  },
  {
    id: "acc-wb-3",
    toolId: "workbuddy",
    label: "WorkBuddy · 备用号 03",
    uidMasked: "wb_91c2****5a",
    status: "risk_paused",
    isActiveOnClient: false,
    latestQuota: {
      planName: "Free",
      claimStatus: "available",
      fetchedAt: cstTimestamp(1, 21, 5),
      buckets: [
        {
          key: "credits",
          label: "可用积分",
          used: 700,
          total: 1000,
          remaining: 300,
          unit: "积分",
        },
      ],
    },
    updatedAt: cstTimestamp(1, 21, 5),
  },
  {
    id: "acc-qoder-1",
    toolId: "qoder_cn",
    label: "Qoder · 日常主力",
    uidMasked: "qd_5e82****4d",
    emailMasked: "al***@aliyun.com",
    status: "active",
    isActiveOnClient: true,
    latestQuota: {
      planName: "Community",
      claimStatus: "available",
      fetchedAt: cstTimestamp(0, 10, 15),
      buckets: [
        {
          key: "daily_credits",
          label: "通用 Credits",
          used: 220,
          total: 500,
          remaining: 280,
          unit: "Credits",
        },
      ],
    },
    lastClaimedAt: cstTimestamp(1, 10, 5),
    updatedAt: cstTimestamp(0, 10, 15),
  },
  {
    id: "acc-qoder-2",
    toolId: "qoder_cn",
    label: "Qoder · 实验号",
    uidMasked: "qd_2b44****9a",
    status: "active",
    isActiveOnClient: false,
    latestQuota: {
      planName: "Community",
      claimStatus: "claimed",
      fetchedAt: cstTimestamp(0, 7, 30),
      buckets: [
        {
          key: "daily_credits",
          label: "通用 Credits",
          used: 90,
          total: 500,
          remaining: 410,
          unit: "Credits",
        },
      ],
    },
    lastClaimedAt: cstTimestamp(0, 7, 30),
    updatedAt: cstTimestamp(0, 7, 30),
  },
  {
    id: "acc-minimax-1",
    toolId: "minimax_code_cn",
    label: "MiniMax Code CN · 主号",
    uidMasked: "mm_91c4****7e",
    emailMasked: "co***@minimaxi.com",
    status: "active",
    isActiveOnClient: true,
    hasLocalSnapshot: true,
    hasApiToken: true,
    latestQuota: {
      planName: "Free",
      claimStatus: "available",
      fetchedAt: cstTimestamp(0, 10, 5),
      buckets: [
        {
          key: "credits",
          label: "可用积分",
          used: 145,
          total: 1000,
          remaining: 855,
          unit: "Credits",
          resetAt: nextMonthResetCst(),
        },
      ],
    },
    updatedAt: cstTimestamp(0, 10, 5),
  },
  {
    id: "acc-lobster-1",
    toolId: "lobsterai",
    label: "LobsterAI · 主号",
    uidMasked: "17****38",
    status: "active",
    isActiveOnClient: true,
    hasLocalSnapshot: true,
    hasApiToken: true,
    latestQuota: {
      planName: "订阅版",
      claimStatus: "available",
      fetchedAt: cstTimestamp(0, 10, 2),
      buckets: [
        {
          key: "credits",
          label: "可用积分",
          used: 420,
          total: 3000,
          remaining: 2580,
          unit: "积分",
          resetAt: nextMonthResetCst(),
        },
      ],
    },
    updatedAt: cstTimestamp(0, 10, 2),
  },
  {
    id: "acc-doubaowork-1",
    toolId: "doubaowork",
    label: "豆包工作 · 主号",
    uidMasked: "dw_71****9e",
    status: "active",
    isActiveOnClient: true,
    hasLocalSnapshot: true,
    hasApiToken: true,
    latestQuota: {
      planName: "免费版",
      claimStatus: "available",
      fetchedAt: cstTimestamp(0, 9, 55),
      buckets: [
        {
          key: "incentive_3_0_0",
          label: "活动奖励额度",
          used: 120,
          total: 1000,
          remaining: 880,
          unit: "额度",
          resetAt: nextMonthResetCst(),
        },
      ],
    },
    updatedAt: cstTimestamp(0, 9, 55),
  },
  {
    id: "acc-codearts-1",
    toolId: "codearts",
    label: "码道 CodeArts · 主号",
    uidMasked: "ca_c02b****8f7",
    status: "active",
    isActiveOnClient: true,
    hasLocalSnapshot: true,
    hasApiToken: true,
    latestQuota: {
      planName: "基础套餐",
      // 每日签到可用（mock 展示为待领取）
      claimStatus: "available",
      fetchedAt: cstTimestamp(0, 9, 50),
      buckets: [
        {
          key: "total",
          label: "总积分",
          used: 0,
          total: 4500,
          remaining: 4500,
          unit: "积分",
        },
        {
          key: "basic",
          label: "基础套餐积分",
          used: 0,
          total: 500,
          remaining: 500,
          unit: "积分",
        },
        {
          key: "bonus",
          label: "赠送积分",
          used: 0,
          total: 4000,
          remaining: 4000,
          unit: "积分",
        },
      ],
    },
    updatedAt: cstTimestamp(0, 9, 50),
  },
];

export const INITIAL_LOGS: ActionLogItem[] = [
  {
    id: 0,
    accountId: "acc-minimax-1",
    toolId: "minimax_code_cn",
    actionType: "query",
    resultStatus: "success",
    messageSanitized: "MiniMax Code CN · 主号 (mm_91c4****7e) 已刷新积分与签到状态",
    createdAt: cstTimestamp(0, 10, 5),
  },
  {
    id: 1,
    accountId: "acc-wb-2",
    toolId: "workbuddy",
    actionType: "claim",
    resultStatus: "success",
    messageSanitized: "WorkBuddy · 副号 (wb_39f0****14) 成功领取今日签到 100 积分",
    createdAt: cstTimestamp(0, 8, 20),
  },
  {
    id: 2,
    accountId: "acc-qoder-2",
    toolId: "qoder_cn",
    actionType: "claim",
    resultStatus: "success",
    messageSanitized: "Qoder · 实验号 (qd_2b44****9a) 成功领取每日 100 Credits",
    createdAt: cstTimestamp(0, 7, 30),
  },
  {
    id: 3,
    accountId: "acc-wb-3",
    toolId: "workbuddy",
    actionType: "claim",
    resultStatus: "failed",
    errorCode: "RISK_TRIGGERED",
    messageSanitized: "WorkBuddy · 备用号 03 收到 403 校验挑战，已触发熔断暂停",
    createdAt: cstTimestamp(1, 21, 5),
  },
  {
    id: 4,
    accountId: "acc-trae-1",
    toolId: "trae_cn",
    actionType: "switch",
    resultStatus: "success",
    messageSanitized: "已将本机 Trae 桌面端切换至 Trae CN · 主力工作号 并重启完成",
    createdAt: cstTimestamp(1, 18, 12),
  },
];
