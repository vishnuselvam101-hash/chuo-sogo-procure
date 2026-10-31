import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
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
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void start();
