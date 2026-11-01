import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import type { Me } from "../lib/types";
import { ErrorText } from "../components/ui";

type Mode = "signin" | "signup" | "code";

/** Sign in, create an account (activation code by email), or set the password from the code. */
export function ShopAuth({
  serverUrl,
  onSignedIn,
  onLegal,
}: {
  serverUrl: string;
  onSignedIn: (me: Me) => void;
  onLegal: () => void;
}) {
  const { t, i18n } = useTranslation();
  const bridge = useBridge();
  const [mode, setMode] = useState<Mode>("signin");
  const [email, setEmail] = useState("");
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [token, setToken] = useState("");
  const [server, setServer] = useState(serverUrl);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function guarded(fn: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      if (server !== serverUrl) await bridge.prefsSetServer(server);
      await fn();
    } catch (err) {
      setError(typeof err === "string" ? err : "unknown");
    } finally {
      setBusy(false);
    }
  }

  const signIn = (e: FormEvent) => {
    e.preventDefault();
    void guarded(async () => {
      const res = await bridge.login(email, password);
      setPassword("");
      if (ok(res)) onSignedIn(res.body);
      else setError(errorCode(res));
    });
  };
  const signUp = (e: FormEvent) => {
    e.preventDefault();
    void guarded(async () => {
      const lang = ["ja", "en", "ta", "hi"].includes(i18n.language) ? i18n.language : "ja";
      const res = await bridge.api("POST", "/api/v1/shop/signup", {
        email,
        display_name: name,
        locale: lang,
      });
      if (!ok(res)) return setError(errorCode(res));
      setNotice(t("shop:auth.checkEmail"));
      setMode("code");
    });
  };
  const setPw = (e: FormEvent) => {
    e.preventDefault();
    void guarded(async () => {
      const res = await bridge.api("POST", "/api/v1/auth/set-password", { token: token.trim(), password });
      setPassword("");
      if (!ok(res)) return setError(errorCode(res));
      setNotice(t("shop:auth.passwordSet"));
      setMode("signin");
    });
  };

  const serverField = (
    <details>
      <summary>{t("settings:server.title")}</summary>
      <label>
        {t("auth.server")}
        <input type="url" value={server} onChange={(e) => setServer(e.target.value)} />
      </label>
    </details>
  );

  return (
    <main className="login shop-login">
      <form
        className="card login-card"
        onSubmit={mode === "signin" ? signIn : mode === "signup" ? signUp : setPw}
        data-testid={`shop-${mode}`}
      >
        <h1>{t("shop:name")}</h1>
        <p className="muted small">{t("shop:tagline")}</p>
        {notice && (
          <p className="ok" role="status">
            {notice}
          </p>
        )}
        {mode === "signup" && (
          <label>
            {t("shop:auth.name")}
            <input
              required
              maxLength={120}
              autoComplete="name"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </label>
        )}
        {mode !== "code" && (
          <label>
            {t("auth.email")}
            <input
              type="email"
              required
              autoComplete="username"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
          </label>
        )}
        {mode === "code" && (
          <label>
            {t("shop:auth.code")}
            <input
              required
              autoComplete="one-time-code"
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
          </label>
        )}
        {mode !== "signup" && (
          <label>
            {mode === "code" ? t("auth.newPassword") : t("auth.password")}
            <input
              type="password"
              required
              minLength={mode === "code" ? 12 : undefined}
              autoComplete={mode === "code" ? "new-password" : "current-password"}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </label>
        )}
        {serverField}
        <ErrorText code={error} />
        <button type="submit" className="primary" disabled={busy}>
          {mode === "signin"
            ? t("auth.signIn")
            : mode === "signup"
              ? t("shop:auth.create")
              : t("auth.setPassword")}
        </button>
        <div className="row">
          {mode !== "signin" && (
            <button type="button" className="link" onClick={() => setMode("signin")}>
              {t("auth.backToSignIn")}
            </button>
          )}
          {mode !== "signup" && (
            <button type="button" className="link" onClick={() => setMode("signup")}>
              {t("shop:auth.createLink")}
            </button>
          )}
          {mode !== "code" && (
            <button type="button" className="link" onClick={() => setMode("code")}>
              {t("shop:auth.haveCode")}
            </button>
          )}
          <button type="button" className="link" onClick={onLegal}>
            {t("shop:nav.legal")}
          </button>
        </div>
      </form>
    </main>
  );
}
