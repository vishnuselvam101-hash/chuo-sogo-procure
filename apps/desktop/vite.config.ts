import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tauri serves the built files; dev server is local only.
// `--mode admin` builds the admin console into dist-admin.
export default defineConfig(({ mode }) => ({
  plugins: [react()],
  clearScreen: false,
  // Build-time constant: each build only contains its own app.
  define: { "import.meta.env.VITE_APP_KIND": JSON.stringify(mode === "admin" ? "admin" : "") },
  server: { host: "127.0.0.1", port: mode === "admin" ? 1421 : 1420, strictPort: true },
  build: { target: "es2022", sourcemap: false, outDir: mode === "admin" ? "dist-admin" : "dist" },
  test: {
    environment: "jsdom",
    globals: true,
    include: ["tests/**/*.test.ts", "tests/**/*.test.tsx"],
  },
}));
