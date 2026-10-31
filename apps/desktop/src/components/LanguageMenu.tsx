import { useState } from "react";
import { useTranslation } from "react-i18next";
import { LANGUAGES, type LanguageCode } from "../i18n";

interface Props {
  current: LanguageCode;
  onChange: (code: LanguageCode) => void;
}

/** Main-menu language selector: native names, current language always visible. */
export function LanguageMenu({ current, onChange }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const currentName = LANGUAGES.find((l) => l.code === current)?.nativeName ?? current;
  return (
    <div className="language-menu">
      <button
        type="button"
        className="nav-item"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <span>{t("nav.language")}</span>
        <span className="nav-value" data-testid="current-language">
          {currentName}
        </span>
      </button>
      {open && (
        <ul role="listbox" aria-label={t("nav.language")} className="language-list">
          {LANGUAGES.map((l) => (
            <li key={l.code}>
              <button
                type="button"
                role="option"
                aria-selected={l.code === current}
                lang={l.code}
                onClick={() => {
                  onChange(l.code);
                  setOpen(false);
                }}
              >
                {l.nativeName}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
