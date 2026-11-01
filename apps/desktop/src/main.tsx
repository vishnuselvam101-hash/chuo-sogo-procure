import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { initI18n, isLanguage } from "./i18n";
import { bridge } from "./lib/bridge";
import "./styles.css";

// Forward uncaught UI errors to the native log (no data, just the message).
window.addEventListener("error", (e) => void bridge.log("error", String(e.message)));
window.addEventListener("unhandledrejection", (e) => void bridge.log("error", String(e.reason)));

async function start() {
  const prefs = await bridge.prefsGet().catch(() => null);
  await initI18n(isLanguage(prefs?.language) ? prefs.language : undefined);
  const root = document.getElementById("root");
  if (!root) return;
  // Each build contains only its own app: the condition is a build-time constant.
  const Root =
    import.meta.env.VITE_APP_KIND === "admin"
      ? (await import("./admin/AdminApp")).AdminApp
      : (await import("./App")).App;
  if (import.meta.env.VITE_APP_KIND === "admin") document.title = "Chuo-Sogo Procure Admin";
  createRoot(root).render(
    <StrictMode>
      <Root />
    </StrictMode>,
  );
}

void start();
