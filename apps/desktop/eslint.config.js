import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

// Focused lint config: we only want the classic React Hooks correctness rules
// (rules-of-hooks + exhaustive-deps) so hook dependency arrays are tool-verified.
// The rest of eslint-plugin-react-hooks v7 (React Compiler rules) is intentionally
// left off to avoid noisy churn on the existing components.
export default tseslint.config(
  {
    ignores: ["dist/**", "node_modules/**", "src-tauri/**"],
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: {
        ecmaFeatures: { jsx: true },
      },
    },
    plugins: {
      "react-hooks": reactHooks,
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
    },
  },
);
