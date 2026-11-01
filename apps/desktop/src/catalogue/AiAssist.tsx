import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAction } from "../lib/useAction";
import { MAX_UPLOAD_BYTES, readAsBase64 } from "../lib/files";
import { ErrorText } from "../components/ui";

/** The validated output of `extract_listing` (procure_ai::listing::ListingSuggestion). */
export interface ListingAiOutput {
  manufacturer: string;
  model_number: string;
  condition: string;
  title: string;
  title_en: string;
  description: string;
  description_en: string;
  specs: { key: string; value: string; in_source: boolean }[];
  confidence: number;
  flags: string[];
  needs_review: boolean;
}

/** The validated output of `translate_listing`. */
export interface TranslationOutput {
  target: "ja" | "en";
  title: string;
  description: string;
  flags: string[];
}

interface SuggestionResponse {
  id: string;
  status: string;
  output: unknown;
  error_code: string | null;
  input_text: string;
  input_source: string;
}

/** The editor fields a suggestion can fill. */
export interface ListingFields {
  manufacturer: string;
  model_number: string;
  condition: string;
  title: string;
  title_en: string;
  description: string;
  description_en: string;
  specs: { key: string; value: string }[];
}

/**
 * Suggested values fill only empty fields: what the supplier already entered (or applied
 * from an earlier suggestion) is never overwritten. New specification properties are
 * appended; existing ones keep their value unless it is empty. Nothing is saved here.
 */
export function applySuggestion<T extends ListingFields>(f: T, s: ListingAiOutput): T {
  const pick = (current: string, suggested: string) => (current.trim() ? current : suggested.trim());
  const specs = f.specs.map((x) => ({ ...x }));
  for (const sp of s.specs) {
    const same = specs.find((x) => x.key.trim() === sp.key.trim());
    if (same) {
      if (!same.value.trim()) same.value = sp.value;
    } else if (specs.length < 50) specs.push({ key: sp.key, value: sp.value });
  }
  return {
    ...f,
    manufacturer: pick(f.manufacturer, s.manufacturer),
    model_number: pick(f.model_number, s.model_number),
    // The form starts at "new"; a stated condition (used / refurbished) wins over that default.
    condition: s.condition && f.condition === NEW ? s.condition : f.condition,
    title: pick(f.title, s.title),
    title_en: pick(f.title_en, s.title_en),
    description: pick(f.description, s.description),
    description_en: pick(f.description_en, s.description_en),
    specs,
  };
}

const NEW = "new";
const VOICE_TYPES = ".wav,.mp3,.m4a,.ogg,.opus,.webm,.flac";
const FILE_TYPES = ".pdf,.xlsx,.xls,.csv,.txt";
const LANGS = ["ja", "en", "ta", "hi"] as const;
const MODES = ["text", "voice", "file"] as const;
const SHOWN_FIELDS = ["manufacturer", "model_number", "condition", "title", "title_en"] as const;
type Mode = (typeof MODES)[number];

/** Microphone recording, where the webview supports it. */
function canRecord(): boolean {
  return typeof window !== "undefined" && "MediaRecorder" in window && !!navigator.mediaDevices?.getUserMedia;
}

function blobToBase64(blob: Blob): Promise<string> {
  return readAsBase64(new File([blob], "voice"));
}

/**
 * The listing assistant: describe the product by typing, a voice note or a catalogue file;
 * the server (and its AI model) proposes fields, flags what it could not find in the source,
 * and the supplier decides what to apply.
 */
export function AiAssist({
  orgBase,
  listingId,
  onApply,
}: {
  orgBase: string;
  listingId: string | null;
  onApply: (s: ListingAiOutput) => void;
}) {
  const { t } = useTranslation();
  const { run, busy, error, setError } = useAction();
  const [mode, setMode] = useState<Mode>("text");
  const [text, setText] = useState("");
  const [lang, setLang] = useState<string>("ja");
  const [result, setResult] = useState<SuggestionResponse | null>(null);
  const [recording, setRecording] = useState<MediaRecorder | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const voiceInput = useRef<HTMLInputElement>(null);
  const base = `${orgBase}/ai`;

  async function send(path: string, body: object) {
    setResult(null);
    const res = await run<SuggestionResponse>("POST", `${base}/${path}`, { ...body, listing_id: listingId });
    if (res?.body) {
      setResult(res.body);
      if (res.body.error_code) setError(res.body.error_code);
    }
  }

  async function sendFile(path: string, file: File | Blob, name: string, extra: object = {}) {
    if (file.size > MAX_UPLOAD_BYTES) return setError("payload_too_large");
    const data = file instanceof File ? await readAsBase64(file) : await blobToBase64(file);
    await send(path, { file_name: name, data_base64: data, ...extra });
  }

  async function record() {
    if (recording) {
      recording.stop();
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const rec = new MediaRecorder(stream);
      const chunks: Blob[] = [];
      rec.ondataavailable = (e) => chunks.push(e.data);
      rec.onstop = () => {
        stream.getTracks().forEach((tr) => tr.stop());
        setRecording(null);
        const blob = new Blob(chunks, { type: rec.mimeType });
        void sendFile("from-voice", blob, "voice", { language: lang });
      };
      rec.start();
      setRecording(rec);
    } catch {
      setError("microphone_unavailable");
    }
  }

  async function decide(accepted: boolean) {
    if (!result) return;
    const res = await run("POST", `${base}/suggestions/${result.id}/decision`, { accepted });
    if (res && accepted) onApply(result.output as ListingAiOutput);
    setResult(null);
  }

  const out = result && !result.error_code ? (result.output as ListingAiOutput) : null;

  return (
    <div className="card stack ai-assist" data-testid="ai-assist">
      <h2>{t("catalogue:ai.title")}</h2>
      <p className="muted small">{t("catalogue:ai.hint")}</p>
      <div className="tabs" role="tablist">
        {MODES.map((m) => (
          <button
            key={m}
            type="button"
            role="tab"
            aria-selected={mode === m}
            className={mode === m ? "tab active" : "tab"}
            onClick={() => setMode(m)}
          >
            {t(`catalogue:ai.mode.${m}`)}
          </button>
        ))}
      </div>
      {mode === "text" && (
        <>
          <textarea
            rows={5}
            maxLength={20000}
            aria-label={t("catalogue:ai.textLabel")}
            placeholder={t("catalogue:ai.textPlaceholder")}
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
          <div className="row">
            <button
              type="button"
              disabled={busy || !text.trim()}
              onClick={() => void send("from-text", { text })}
            >
              {t("catalogue:ai.suggest")}
            </button>
          </div>
        </>
      )}
      {mode === "voice" && (
        <div className="row">
          <label>
            {t("catalogue:ai.language")}
            <select value={lang} onChange={(e) => setLang(e.target.value)}>
              {LANGS.map((l) => (
                <option key={l} value={l}>
                  {t(`catalogue:ai.lang.${l}`)}
                </option>
              ))}
            </select>
          </label>
          {canRecord() && (
            <button type="button" disabled={busy} onClick={() => void record()}>
              {recording ? t("catalogue:ai.stop") : t("catalogue:ai.record")}
            </button>
          )}
          <input
            ref={voiceInput}
            type="file"
            accept={VOICE_TYPES}
            hidden
            data-testid="voice-input"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void sendFile("from-voice", f, f.name, { language: lang });
            }}
          />
          <button type="button" disabled={busy} onClick={() => voiceInput.current?.click()}>
            {t("catalogue:ai.uploadVoice")}
          </button>
          <span className="muted small">{t("catalogue:ai.voiceHint")}</span>
        </div>
      )}
      {mode === "file" && (
        <div className="row">
          <input
            ref={fileInput}
            type="file"
            accept={FILE_TYPES}
            hidden
            data-testid="catalogue-file-input"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void sendFile("from-file", f, f.name);
            }}
          />
          <button type="button" disabled={busy} onClick={() => fileInput.current?.click()}>
            {t("catalogue:ai.uploadFile")}
          </button>
          <span className="muted small">{t("catalogue:ai.fileHint")}</span>
        </div>
      )}
      {busy && (
        <p className="muted" role="status">
          {t("catalogue:ai.working")}
        </p>
      )}
      <ErrorText code={error} />
      {result && (
        <details open={result.input_source !== "typed"}>
          <summary>{t(`catalogue:ai.source.${result.input_source}`)}</summary>
          <pre className="small wrap" data-testid="ai-input-text">
            {result.input_text}
          </pre>
        </details>
      )}
      {out && (
        <div className="stack" data-testid="ai-suggestion">
          {out.needs_review && <p className="warn-text">{t("catalogue:ai.checkCarefully")}</p>}
          {out.flags.length > 0 && (
            <ul className="small">
              {out.flags.map((f) => (
                <li key={f}>{t(`catalogue:ai.flag.${f}`, { defaultValue: f })}</li>
              ))}
            </ul>
          )}
          <dl className="facts">
            {SHOWN_FIELDS.map((k) =>
              out[k] ? (
                <div key={k} className="contents">
                  <dt>{t(`catalogue:ai.field.${k}`)}</dt>
                  <dd>{out[k]}</dd>
                </div>
              ) : null,
            )}
          </dl>
          {out.specs.length > 0 && (
            <table className="table narrow-table">
              <tbody>
                {out.specs.map((sp, i) => (
                  <tr key={i} className={sp.in_source ? "" : "warn-text"}>
                    <th>{sp.key}</th>
                    <td>
                      {sp.value}
                      {!sp.in_source && ` — ${t("catalogue:ai.notInSource")}`}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <div className="row">
            <button type="button" className="primary" disabled={busy} onClick={() => void decide(true)}>
              {t("catalogue:ai.apply")}
            </button>
            <button type="button" disabled={busy} onClick={() => void decide(false)}>
              {t("catalogue:ai.discard")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Puts a translation draft into the other-language fields, but only where they are empty:
 * text the supplier already wrote there is kept.
 */
export function applyTranslation<T extends ListingFields>(f: T, d: TranslationOutput): T {
  const fill = (current: string, draft: string) => (current.trim() ? current : draft);
  return d.target === "en"
    ? { ...f, title_en: fill(f.title_en, d.title), description_en: fill(f.description_en, d.description) }
    : { ...f, title: fill(f.title, d.title), description: fill(f.description, d.description) };
}

/** "Draft in the other language" for a saved listing; the result fills the editor only. */
export function TranslateButton({
  orgBase,
  listingId,
  onDraft,
}: {
  orgBase: string;
  listingId: string;
  onDraft: (d: TranslationOutput) => void;
}) {
  const { t } = useTranslation();
  const { run, busy, error } = useAction();
  return (
    <span className="stack">
      <button
        type="button"
        disabled={busy}
        onClick={async () => {
          const res = await run<SuggestionResponse>("POST", `${orgBase}/${listingId}/ai/translate`);
          if (!res?.body || res.body.error_code) return;
          // Recorded as applied only once the server has accepted the decision.
          const decided = await run("POST", `${orgBase}/ai/suggestions/${res.body.id}/decision`, {
            accepted: true,
          });
          if (decided) onDraft(res.body.output as TranslationOutput);
        }}
      >
        {t("catalogue:ai.translate")}
      </button>
      <ErrorText code={error} />
    </span>
  );
}
