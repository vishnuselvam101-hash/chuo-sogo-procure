// Standalone mode ("use on this PC only"): the first-run choice, the local start and first
// account, and the pack manager. Everything runs on this PC; the native layer starts and
// stops the local database, server and AI, and only accepts pack ids from this screen.
import { useCallback, useEffect, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { LoginStage } from "../components/LoginStage";
import { ErrorText } from "../components/ui";
import { useBridge } from "../lib/BridgeContext";
import type { LocalStatus, PackInfo, PacksStatus } from "../lib/types";

const GB = 1024 ** 3;
const SERVER = "server";
const LOCAL = "local";

function useGb() {
  const { i18n, t } = useTranslation();
  return (bytes: number) =>
    t("local:gb", {
      n: new Intl.NumberFormat(i18n.language, { maximumFractionDigits: bytes >= 10 * GB ? 0 : 1 }).format(
        bytes / GB,
      ),
    });
}

/** First run of an installation that can run on its own: where does the data live? */
export function ModeChoice({ onChosen }: { onChosen: (mode: "server" | "local") => void }) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [error, setError] = useState<string | null>(null);

  async function choose(mode: "server" | "local") {
    setError(null);
    try {
      await bridge.localChooseMode(mode);
      onChosen(mode);
    } catch (err) {
      setError(typeof err === "string" ? err : "unknown");
    }
  }

  return (
    <LoginStage>
      <div className="card login-card mode-choice">
        <h1>{t("local:choice.title")}</h1>
        <p className="login-lead">{t("local:choice.lead")}</p>
        <button type="button" className="mode-option" onClick={() => void choose(LOCAL)}>
          <strong>{t("local:choice.local")}</strong>
          <span>{t("local:choice.localHelp")}</span>
        </button>
        <button type="button" className="mode-option" onClick={() => void choose(SERVER)}>
          <strong>{t("local:choice.server")}</strong>
          <span>{t("local:choice.serverHelp")}</span>
        </button>
        <ErrorText code={error} />
      </div>
    </LoginStage>
  );
}

function usePoll<T>(load: () => Promise<T>, ms: number): [T | null, () => void] {
  const [data, setData] = useState<T | null>(null);
  const refresh = useCallback(() => {
    void load()
      .then(setData)
      .catch(() => undefined);
  }, [load]);
  useEffect(() => {
    refresh();
    const id = setInterval(refresh, ms);
    return () => clearInterval(id);
  }, [refresh, ms]);
  return [data, refresh];
}

/** The factory and its first administrator, created on this PC. */
function SetupForm({ onDone }: { onDone: (email: string) => void }) {
  const { t, i18n } = useTranslation();
  const bridge = useBridge();
  const [factory, setFactory] = useState("");
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [repeat, setRepeat] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (password !== repeat) return setError("validation_failed/password_mismatch");
    setBusy(true);
    setError(null);
    try {
      await bridge.localSetup(factory, name, email, password, i18n.language);
      onDone(email);
    } catch (err) {
      setError(typeof err === "string" ? err : "unknown");
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="card login-card" onSubmit={submit}>
      <h1>{t("local:setup.title")}</h1>
      <p className="login-lead">{t("local:setup.lead")}</p>
      <label>
        {t("local:setup.factory")}
        <input required maxLength={200} value={factory} onChange={(e) => setFactory(e.target.value)} />
      </label>
      <label>
        {t("local:setup.name")}
        <input required maxLength={200} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
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
        {busy ? t("local:setup.creating") : t("local:setup.create")}
      </button>
    </form>
  );
}

/**
 * Standalone mode before sign-in: waits for the local services, runs the first-run setup and
 * the pack step, then hands over to the normal sign-in screen (`children(serverUrl)`).
 */
export function LocalGate({ children }: { children: (serverUrl: string) => React.ReactNode }) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [status] = usePoll<LocalStatus>(bridge.localStatus, 1000);
  const [packStep, setPackStep] = useState(false);

  if (!status || status.phase === "starting" || status.phase === "off")
    return (
      <LoginStage>
        <div className="card login-card" role="status">
          <h1>{t("local:start.title")}</h1>
          <p className="login-lead">{t("local:start.lead")}</p>
        </div>
      </LoginStage>
    );
  if (status.phase === "failed")
    return (
      <LoginStage>
        <div className="card login-card">
          <h1>{t("local:start.failed")}</h1>
          <p className="error" role="alert">
            {t(`local:errors.${status.error ?? "unknown"}`, { defaultValue: t("local:errors.unknown") })}
          </p>
          <button type="button" className="primary" onClick={() => void bridge.localStart()}>
            {t("local:start.retry")}
          </button>
        </div>
      </LoginStage>
    );
  if (packStep)
    return (
      <main className="content local-first-run">
        <PackManager />
        <button type="button" className="primary" onClick={() => setPackStep(false)}>
          {t("local:packs.continue")}
        </button>
      </main>
    );
  if (status.needs_setup)
    return (
      <LoginStage>
        <SetupForm onDone={() => setPackStep(true)} />
      </LoginStage>
    );
  return <>{children(status.server_url ?? "")}</>;
}

function PackRow({
  pack,
  recommended,
  ramBytes,
  onChanged,
}: {
  pack: PackInfo;
  recommended: string | null;
  ramBytes: number;
  onChanged: () => void;
}) {
  const { t, i18n } = useTranslation();
  const bridge = useBridge();
  const gb = useGb();
  const [variant, setVariant] = useState<string | null>(
    pack.variants.find((v) => v.installed)?.variant ?? recommended ?? pack.variants[0]?.variant ?? null,
  );
  const [error, setError] = useState<string | null>(null);
  const ja = i18n.language === "ja";
  const chosen = pack.variants.find((v) => v.variant === variant);
  const size = chosen?.size ?? pack.size;
  const installedHere = pack.variants.length ? Boolean(chosen?.installed) : pack.status === "installed";

  async function install() {
    setError(null);
    try {
      await bridge.packsInstall(pack.id, pack.variants.length ? variant : null);
    } catch (err) {
      setError(typeof err === "string" ? err : "unknown");
    }
    onChanged();
  }

  const pct = pack.total > 0 ? Math.floor((pack.done / pack.total) * 100) : 0;
  const shownError = error ?? (pack.status === "failed" ? pack.error : null);
  return (
    <li className="card pack">
      <div className="pack-head">
        <h3>{ja ? pack.title_ja : pack.title_en}</h3>
        <span className={`badge ${installedHere ? "badge-active" : "badge-pending"}`}>
          {t(
            `local:packs.status.${pack.status === "installing" ? "installing" : installedHere ? "installed" : "missing"}`,
          )}
        </span>
      </div>
      <p className="muted small">
        {t("local:packs.meta", { size: gb(size), licence: pack.licence })}
        {pack.kind === "core" ? ` · ${t("local:packs.bundled")}` : ` · ${t("local:packs.download")}`}
      </p>
      {pack.variants.length > 0 && (
        <div className="choice-row">
          {pack.variants.map((v) => (
            <label key={v.variant} className={`choice ${variant === v.variant ? "selected" : ""}`}>
              <input
                type="radio"
                name={`variant-${pack.id}`}
                checked={variant === v.variant}
                onChange={() => setVariant(v.variant)}
              />
              <strong>{ja ? v.title_ja : v.title_en}</strong>
              <span className="muted small">
                {gb(v.size)} · {t("local:packs.minRam", { n: v.min_ram_gb })}
              </span>
              {v.variant === recommended && (
                <span className="badge badge-active">
                  {t("local:packs.recommended", { n: Math.round(ramBytes / GB) })}
                </span>
              )}
            </label>
          ))}
        </div>
      )}
      {pack.status === "installing" && (
        <div className="pack-progress">
          <progress max={100} value={pct} aria-label={t("local:packs.progress")} />
          <span className="num">
            {t("local:packs.progressText", { pct, done: gb(pack.done), total: gb(pack.total) })}
          </span>
          <button type="button" onClick={() => void bridge.packsCancel(pack.id).then(onChanged)}>
            {t("local:packs.cancel")}
          </button>
        </div>
      )}
      {shownError && (
        <p className="error" role="alert">
          {t(`local:errors.${shownError}`, { defaultValue: t("local:errors.unknown") })}
        </p>
      )}
      {pack.status !== "installing" && !installedHere && (
        <button type="button" className="primary" onClick={() => void install()}>
          {pack.kind === "large"
            ? t("local:packs.downloadButton", { size: gb(size) })
            : t("local:packs.install")}
        </button>
      )}
    </li>
  );
}

/** Every pack with its size, licence and state; large packs download from their upstream. */
export function PackManager() {
  const { t } = useTranslation();
  const bridge = useBridge();
  const gb = useGb();
  const [packs, refresh] = usePoll<PacksStatus>(bridge.packsStatus, 1500);
  const [status] = usePoll<LocalStatus>(bridge.localStatus, 3000);

  if (!packs) return <p className="muted">{t("state.loading")}</p>;
  return (
    <section className="pack-manager">
      <h2>{t("local:packs.title")}</h2>
      <p className="muted">{t("local:packs.lead")}</p>
      <p>{t("local:packs.machine", { ram: gb(packs.ram_bytes), free: gb(packs.free_bytes) })}</p>
      {status && (
        <ul className="local-services">
          <li>
            {t("local:services.ai")}: {t(`local:services.state.ai_${status.ai}`)}
          </li>
          <li>
            {t("local:services.voice")}: {t(`local:services.state.voice_${status.voice}`)}
          </li>
          <li>
            {t("local:services.ocr")}: {t(`local:services.state.ocr_${status.ocr}`)}
          </li>
        </ul>
      )}
      <ul className="pack-list">
        {packs.packs.map((p) => (
          <PackRow
            key={p.id}
            pack={p}
            recommended={packs.recommended_variant}
            ramBytes={packs.ram_bytes}
            onChanged={refresh}
          />
        ))}
      </ul>
    </section>
  );
}
