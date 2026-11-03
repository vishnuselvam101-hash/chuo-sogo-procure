import { useTranslation } from "react-i18next";
import { LANGUAGES, type LanguageCode } from "../i18n";
import { formatDateTime, formatMoney } from "../lib/format";
import { PageHeader } from "../components/ui";
import { TEXT_SIZES } from "../lib/display";
import type { TextSize } from "../lib/types";
import { PackManager } from "./LocalMode";

interface Props {
  language: LanguageCode;
  explicit: boolean;
  orgDefault: LanguageCode;
  timezone: string;
  onLanguage: (code: LanguageCode | null) => void;
  textSize: TextSize;
  highContrast: boolean;
  onDisplay: (textSize: TextSize, highContrast: boolean) => void;
  /** Standalone mode: show the pack manager. */
  localMode?: boolean;
}

// Fixed sample values for the preview; display only.
const SAMPLE_INSTANT = "2026-04-01T01:30:00Z";
const SAMPLE_MINOR = 1_280_000;

export function Settings({
  language,
  explicit,
  orgDefault,
  timezone,
  onLanguage,
  textSize,
  highContrast,
  onDisplay,
  localMode = false,
}: Props) {
  const { t } = useTranslation();
  const orgDefaultName = LANGUAGES.find((l) => l.code === orgDefault)?.nativeName ?? orgDefault;
  return (
    <section>
      <PageHeader title={t("nav.settings")} />
      <fieldset className="card">
        <legend>{t("settings:language.title")}</legend>
        <p className="muted">{t("settings:language.help")}</p>
        <label className="radio">
          <input type="radio" name="lang" checked={!explicit} onChange={() => onLanguage(null)} />
          {t("settings:language.orgDefault", { lang: orgDefaultName })}
        </label>
        {LANGUAGES.map((l) => (
          <label key={l.code} className="radio" lang={l.code}>
            <input
              type="radio"
              name="lang"
              checked={explicit && language === l.code}
              onChange={() => onLanguage(l.code)}
            />
            {l.nativeName}
          </label>
        ))}
        <div className="preview">
          <span className="muted">{t("settings:language.preview")}</span>
          <span data-testid="preview-date">{formatDateTime(SAMPLE_INSTANT, language, timezone)}</span>
          <span data-testid="preview-money">{formatMoney(SAMPLE_MINOR, "JPY", language)}</span>
        </div>
      </fieldset>
      <fieldset className="card">
        <legend>{t("settings:display.title")}</legend>
        <p className="muted">{t("settings:display.help")}</p>
        <div className="choice-row" role="radiogroup" aria-label={t("settings:display.textSize")}>
          {TEXT_SIZES.map((size) => (
            <label key={size} className={`choice choice-${size}${textSize === size ? " selected" : ""}`}>
              <input
                type="radio"
                name="text-size"
                checked={textSize === size}
                onChange={() => onDisplay(size, highContrast)}
              />
              <span className="choice-sample" aria-hidden="true">
                {t("settings:display.sample")}
              </span>
              <span>{t(`settings:display.size.${size}`)}</span>
            </label>
          ))}
        </div>
        <label className="radio switch">
          <input
            type="checkbox"
            checked={highContrast}
            onChange={(e) => onDisplay(textSize, e.target.checked)}
          />
          <span>
            <span className="strong">{t("settings:display.contrast")}</span>
            <span className="muted block">{t("settings:display.contrastHelp")}</span>
          </span>
        </label>
      </fieldset>
      {localMode && (
        <fieldset className="card">
          <legend>{t("local:settings.title")}</legend>
          <p className="muted">{t("local:settings.lead")}</p>
          <PackManager />
        </fieldset>
      )}
    </section>
  );
}
