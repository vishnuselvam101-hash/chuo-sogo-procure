import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import type { Me } from "../lib/types";
import { ErrorText } from "../components/ui";

interface Props {
  serverUrl: string;
  onLoggedIn: (me: Me) => void;
}

export function Login({ serverUrl, onLoggedIn }: Props) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [server, setServer] = useState(serverUrl);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (server !== serverUrl) {
        await bridge.prefsSetServer(server);
      }
      const res = await bridge.login(email, password);
      if (ok(res)) {
        onLoggedIn(res.body);
      } else {
        setError(errorCode(res));
      }
    } catch (err) {
      // The Rust layer rejects invalid server addresses with a stable code.
      setError(typeof err === "string" ? err : "unknown");
    } finally {
      setPassword("");
      setBusy(false);
    }
  }

  return (
    <main className="login">
      <form className="card login-card" onSubmit={submit}>
        <h1>{t("app.name")}</h1>
        <label>
          {t("auth.email")}
          <input
            type="email"
            autoComplete="username"
            required
            value={email}
            onChange={(e) => setEmail(e.target.value)}
          />
        </label>
        <label>
          {t("auth.password")}
          <input
            type="password"
            autoComplete="current-password"
            required
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </label>
        <details>
          <summary>{t("settings:server.title")}</summary>
          <label>
            {t("auth.server")}
            <input type="url" value={server} onChange={(e) => setServer(e.target.value)} />
          </label>
        </details>
        <ErrorText code={error} />
        <button type="submit" className="primary" disabled={busy}>
          {busy ? t("auth.signingIn") : t("auth.signIn")}
        </button>
      </form>
    </main>
  );
}
