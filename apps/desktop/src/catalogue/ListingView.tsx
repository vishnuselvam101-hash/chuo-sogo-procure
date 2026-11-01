import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useBridge } from "../lib/BridgeContext";
import { ok } from "../lib/bridge";
import type { ListingDetail, ListingSummary } from "../lib/types";
import { Money } from "../components/ui";

const PHOTO = "photo";

/** A listing photo, fetched through the native layer (the webview never fetches itself). */
export function ListingPhoto({ path, alt, className }: { path: string; alt: string; className?: string }) {
  const { t } = useTranslation();
  const bridge = useBridge();
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let current = true;
    bridge
      .image(path)
      .then((res) => {
        if (!current) return;
        if (ok(res)) setSrc(res.body.data_url);
        else setFailed(true);
      })
      .catch(() => current && setFailed(true));
    return () => {
      current = false;
    };
  }, [bridge, path]);
  if (failed) return <div className={`photo-placeholder ${className ?? ""}`}>{t("catalogue:noPhoto")}</div>;
  if (!src) return <div className={`photo-placeholder ${className ?? ""}`} aria-busy="true" />;
  return <img className={className} src={src} alt={alt} />;
}

export function ListingStatusBadge({ status }: { status: string }) {
  const { t } = useTranslation();
  return <span className={`badge badge-listing-${status}`}>{t(`catalogue:status.${status}`)}</span>;
}

export function Price({ l }: { l: ListingSummary }) {
  const { t } = useTranslation();
  return (
    <span className="stack small">
      {l.business_price_minor !== null && l.business_price_minor !== undefined && (
        <span>
          {t("catalogue:businessPrice")}: <Money minor={l.business_price_minor} currency={l.currency} />
        </span>
      )}
      {l.consumer_price_minor !== null && l.consumer_price_minor !== undefined && (
        <span>
          {t("catalogue:consumerPrice")}: <Money minor={l.consumer_price_minor} currency={l.currency} />
        </span>
      )}
    </span>
  );
}

/** Read-only listing: photos, identity, prices, specs, documents. `mediaPath` builds content paths. */
export function ListingView({ l, mediaPath }: { l: ListingDetail; mediaPath: (mediaId: string) => string }) {
  const { t, i18n } = useTranslation();
  const photos = l.media.filter((m) => m.kind === PHOTO);
  const documents = l.media.filter((m) => m.kind !== PHOTO);
  const title = i18n.language !== "ja" && l.title_en ? l.title_en : l.title;
  const description = i18n.language !== "ja" && l.description_en ? l.description_en : l.description;
  return (
    <div className="stack" data-testid="listing-view">
      <div className="photo-strip">
        {photos.map((m, i) => (
          <ListingPhoto
            key={m.id}
            path={mediaPath(m.id)}
            alt={t("catalogue:photoAlt", { n: i + 1, title })}
            className="photo"
          />
        ))}
      </div>
      <dl className="facts">
        <dt>{t("catalogue:titleLabel")}</dt>
        <dd>{title || "—"}</dd>
        <dt>{t("catalogue:supplier")}</dt>
        <dd>{l.supplier_name}</dd>
        <dt>{t("catalogue:manufacturer")}</dt>
        <dd>{l.manufacturer || "—"}</dd>
        <dt>{t("catalogue:model")}</dt>
        <dd>
          <code>{l.model_number || "—"}</code>
        </dd>
        {l.serial_number && (
          <>
            <dt>{t("catalogue:serial")}</dt>
            <dd>
              <code>{l.serial_number}</code>
            </dd>
          </>
        )}
        <dt>{t("catalogue:condition")}</dt>
        <dd>{t(`catalogue:conditions.${l.condition}`)}</dd>
        <dt>{t("catalogue:price")}</dt>
        <dd>
          <Price l={l} />
        </dd>
        <dt>{t("catalogue:stock")}</dt>
        <dd>{l.stock === null || l.stock === undefined ? t("catalogue:onRequest") : l.stock}</dd>
      </dl>
      {description && <p className="pre">{description}</p>}
      {l.specs.length > 0 && (
        <table className="table narrow-table" data-testid="listing-specs">
          <tbody>
            {l.specs.map((s, i) => (
              <tr key={i}>
                <th>{s.key}</th>
                <td>{s.value}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {documents.length > 0 && (
        <p className="muted small">
          {t("catalogue:documents")}: {documents.map((d) => d.file_name).join(", ")}
        </p>
      )}
    </div>
  );
}
