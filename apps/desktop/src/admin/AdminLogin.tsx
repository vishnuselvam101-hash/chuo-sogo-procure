import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { errorCode, ok } from "../lib/bridge";
import { ErrorText } from "../components/ui";

type Step = "password" | "enroll" | "code";

interface Props {
  serverUrl: string;
  /** Loads the console; resolves to an error code if that failed (the session is then ended). */
  onSignedIn: () => Promise<string | null>;
}

/** Groups a base32 secret in fours so it can be typed into an authenticator app. */
export function groupSecret(secret: string): string {
  return secret.replace(/(.{4})(?=.)/g, "$1 ");
}

/**
 * Password, then the authenticator: first sign-in shows a new secret to add to an
 * authenticator app and confirms it with a code; later sign-ins only ask for the code.
 */
export function AdminLogin({ serverUrl, onSignedIn }: Props) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [step, setStep] = useState<Step>("password");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [server, setServer] = useState(serverUrl);
  const [code, setCode] = useState("");
  const [secret, setSecret] = useState<{ secret: string; otpauth_uri: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function guarded(fn: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (err) {
      // The Rust layer rejects bad input (server address, code format) with a stable code.
      setError(typeof err === "string" ? err : "unknown");
    } finally {
      setBusy(false);
    }
  }

  const submitPassword = (e: FormEvent) => {
    e.preventDefault();
    void guarded(async () => {
      try {
        if (server !== serverUrl) await bridge.prefsSetServer(server);
        const res = await bridge.adminLogin(email, password);
        if (!ok(res)) return setError(errorCode(res));
        if (res.body.step === "mfa") return setStep("code");
        const enrol = await bridge.adminEnroll();
        if (!ok(enrol)) return setError(errorCode(enrol));
        setSecret(enrol.body);
        setStep("enroll");
      } finally {
        setPassword("");
      }
    });
  };

  const submitCode = (e: FormEvent) => {
    e.preventDefault();
    void guarded(async () => {
      const res = await bridge.adminCode(code);
      setCode("");
      if (ok(res)) {
        const failed = await onSignedIn();
        if (failed) {
          setStep("password");
          setError(failed);
        }
        return;
      }
      const code_ = errorCode(res);
      // An expired step token cannot be retried: start again from the password.
      if (code_ === "unauthenticated") setStep("password");
      // The server answers a wrong code like a wrong password; say which one it was.
      setError(code_ === "invalid_credentials" ? "invalid_code" : code_);
    });
  };

  const restart = () => {
    // Drops the step token held by the native layer as well.
    void bridge.logout();
    setStep("password");
    setSecret(null);
    setCode("");
    setError(null);
  };

  const codeForm = (
    <>
      <label>
        {t("admin:login.code")}
        <input
          required
          inputMode="numeric"
          autoComplete="one-time-code"
          pattern="[0-9 ]{6,7}"
          maxLength={7}
          value={code}
          onChange={(e) => setCode(e.target.value)}
          data-testid="totp-code"
        />
      </label>
      <ErrorText code={error} />
      <button type="submit" className="primary" disabled={busy}>
        {t("admin:login.verify")}
      </button>
      <button type="button" className="link" onClick={restart}>
        {t("auth.backToSignIn")}
      </button>
    </>
  );

  if (step === "enroll" && secret)
    return (
      <main className="login">
        <form className="card login-card" onSubmit={submitCode}>
          <h1>{t("admin:login.enrollTitle")}</h1>
          <p className="muted small">{t("admin:login.enrollHint")}</p>
          <p>
            <code className="totp-secret" data-testid="totp-secret">
              {groupSecret(secret.secret)}
            </code>
          </p>
          <details>
            <summary>{t("admin:login.enrollUri")}</summary>
            <code className="wrap small">{secret.otpauth_uri}</code>
          </details>
          {codeForm}
        </form>
      </main>
    );

  if (step === "code")
    return (
      <main className="login">
        <form className="card login-card" onSubmit={submitCode}>
          <h1>{t("admin:login.codeTitle")}</h1>
          <p className="muted small">{t("admin:login.codeHint")}</p>
          {codeForm}
        </form>
      </main>
    );

  return (
    <main className="login">
      <form className="card login-card" onSubmit={submitPassword}>
        <h1>{t("admin:app.name")}</h1>
        <p className="muted small">{t("admin:login.staffOnly")}</p>
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
