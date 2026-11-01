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

type Mode = "signin" | "token" | "forgot";
const SIGNIN: Mode = "signin";
const TOKEN: Mode = "token";
const FORGOT: Mode = "forgot";

/** Activation and password reset both use the one-time token from the emailed link. */
function TokenForm({ onDone, onBack }: { onDone: () => void; onBack: () => void }) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [token, setToken] = useState("");
  const [password, setPassword] = useState("");
  const [repeat, setRepeat] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (password !== repeat) return setError("validation_failed/password_mismatch");
    setBusy(true);
    setError(null);
    try {
      const res = await bridge.api("POST", "/api/v1/auth/set-password", { token: token.trim(), password });
      if (ok(res)) onDone();
      else setError(errorCode(res));
    } catch (err) {
      setError(typeof err === "string" ? err : "unknown");
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="card login-card" onSubmit={submit}>
      <h1>{t("auth.setPasswordTitle")}</h1>
      <p className="muted small">{t("auth.setPasswordHint")}</p>
      <label>
        {t("auth.token")}
        <input
          required
          autoComplete="one-time-code"
          value={token}
          onChange={(e) => setToken(e.target.value)}
        />
      </label>
      <label>
        {t("auth.newPassword")}
        <input
          type="password"
          required
          minLength={12}
          autoComplete="new-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </label>
      <label>
        {t("auth.repeatPassword")}
        <input
          type="password"
          required
          autoComplete="new-password"
          value={repeat}
          onChange={(e) => setRepeat(e.target.value)}
        />
      </label>
      <ErrorText code={error} />
      <button type="submit" className="primary" disabled={busy}>
        {t("auth.setPassword")}
      </button>
      <button type="button" className="link" onClick={onBack}>
        {t("auth.backToSignIn")}
      </button>
    </form>
  );
}

function ForgotForm({ onBack }: { onBack: () => void }) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [email, setEmail] = useState("");
  const [sent, setSent] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      const res = await bridge.api("POST", "/api/v1/auth/password-reset", { email });
      // Always the same answer, whether or not the address exists.
      if (ok(res)) setSent(true);
      else setError(errorCode(res));
    } catch (err) {
      setError(typeof err === "string" ? err : "unknown");
    }
  }

  return (
    <form className="card login-card" onSubmit={submit}>
      <h1>{t("auth.forgotTitle")}</h1>
      {sent ? (
        <p className="ok" role="status">
          {t("auth.resetSent")}
        </p>
      ) : (
        <>
          <label>
            {t("auth.email")}
            <input type="email" required value={email} onChange={(e) => setEmail(e.target.value)} />
          </label>
          <ErrorText code={error} />
          <button type="submit" className="primary">
            {t("auth.sendReset")}
          </button>
        </>
      )}
      <button type="button" className="link" onClick={onBack}>
        {t("auth.backToSignIn")}
      </button>
    </form>
  );
}

export function Login({ serverUrl, onLoggedIn }: Props) {
  const { t } = useTranslation();
  const [mode, setMode] = useState<Mode>(SIGNIN);
  const [notice, setNotice] = useState<string | null>(null);
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

  if (mode === TOKEN)
    return (
      <main className="login">
        <TokenForm
          onBack={() => setMode(SIGNIN)}
          onDone={() => {
            setNotice(t("auth.passwordSet"));
            setMode(SIGNIN);
          }}
        />
      </main>
    );
  if (mode === FORGOT)
    return (
      <main className="login">
        <ForgotForm onBack={() => setMode(SIGNIN)} />
      </main>
    );

  return (
    <main className="login">
      <form className="card login-card" onSubmit={submit}>
        <h1>{t("app.name")}</h1>
        {notice && (
          <p className="ok" role="status">
            {notice}
          </p>
        )}
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
        <div className="row">
          <button type="button" className="link" onClick={() => setMode(TOKEN)}>
            {t("auth.haveToken")}
          </button>
          <button type="button" className="link" onClick={() => setMode(FORGOT)}>
            {t("auth.forgot")}
          </button>
        </div>
      </form>
    </main>
  );
}
