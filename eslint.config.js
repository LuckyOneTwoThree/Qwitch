import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";

/**
 * Qwitch 前端 lint 配置。
 *
 * 重点不是风格，而是**捕获会导致运行时缺陷的规则**：
 * - `react-hooks/exhaustive-deps`：`QwitchContext.tsx` 的 `useMemo` 曾漏掉
 *   `setLogScope` 依赖（靠 setter 引用稳定才没出事），这类问题只有该规则能发现。
 * - `react-hooks/rules-of-hooks`：防止条件调用 Hook。
 *
 * 风格类规则（缩进、引号等）不在此启用 —— 项目未使用 Prettier，风格由
 * 既有代码约定维持，避免一次性引入大量格式噪音掩盖真实问题。
 */
export default tseslint.config(
  {
    // 构建产物、依赖与 Rust 侧不参与 lint
    ignores: [
      "dist/**",
      "node_modules/**",
      "src-tauri/**",
      "coverage/**",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: {
        ...globals.browser,
        ...globals.es2021,
      },
      parserOptions: {
        ecmaFeatures: { jsx: true },
      },
    },
    plugins: {
      "react-hooks": reactHooks,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // 依赖完整性作为错误：这是本项目最需要护栏的一类问题
      // （`QwitchContext.tsx` 的 useMemo 就曾漏掉 setLogScope）
      "react-hooks/exhaustive-deps": "error",

      // 以下三条是 react-hooks v7 面向 React Compiler 的新规则，属**建议性**重构
      // 指引而非正确性缺陷。本代码库未按 Compiler 约束编写，一次性整改会带来
      // 大面积无关改动，掩盖真实问题，故显式关闭并记录理由：
      // - `refs`：禁止在渲染期读写 ref（本库用 `xxxRef.current = state` 做同步快照）
      // - `set-state-in-effect`：禁止 effect 内同步 setState（本库用于派生状态同步）
      // - `immutability`：禁止修改 props/state 对象（本库仅在局部拷贝上操作）
      "react-hooks/refs": "off",
      "react-hooks/set-state-in-effect": "off",
      "react-hooks/immutability": "off",
      // 未使用变量：允许下划线前缀显式忽略
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      // 测试中允许非空断言等宽松写法
      "@typescript-eslint/no-non-null-assertion": "off",
      "no-empty": ["error", { allowEmptyCatch: true }],
    },
  },
  {
    files: ["src/**/*.test.{ts,tsx}"],
    languageOptions: {
      globals: {
        ...globals.node,
      },
    },
    rules: {
      // 测试里 mock 与断言常需要宽松类型
      "@typescript-eslint/no-explicit-any": "off",
    },
  },
  {
    // vite/vitest 配置文件在 Node 环境运行
    files: ["*.config.ts", "vite.config.ts"],
    languageOptions: {
      globals: { ...globals.node },
    },
  }
);
