import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import i18next from "eslint-plugin-i18next";
import globals from "globals";

export default tseslint.config(
  { ignores: ["dist", "dist-admin", "node_modules", "src-tauri", "coverage", "src/lib/api-schema.ts"] },
  js.configs.recommended,
  ...tseslint.configs.strict,
  { files: ["e2e/**/*.mjs"], languageOptions: { globals: globals.node } },
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: { globals: globals.browser },
    plugins: { "react-hooks": reactHooks, i18next },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // Every user-visible string must come from locales/.
      "i18next/no-literal-string": ["error", { mode: "jsx-only" }],
    },
  },
);
