import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tauri serves the built files; dev server is local only.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { host: "127.0.0.1", port: 1420, strictPort: true },
  build: { target: "es2022", sourcemap: false },
  test: {
    environment: "jsdom",
    globals: true,
    include: ["tests/**/*.test.ts", "tests/**/*.test.tsx"],
  },
});
