import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useApi } from "../lib/useApi";
import type { ListingDetail, ListingSummary, MembershipView } from "../lib/types";
import { Loading, PageHeader } from "../components/ui";
import { ListingPhoto, ListingView, Price } from "../catalogue/ListingView";

function Item({ base, id, onBack }: { base: string; id: string; onBack: () => void }) {
  const { t } = useTranslation();
  const { data, error } = useApi<ListingDetail>(`${base}/${id}`);
  return (
    <section>
      <PageHeader title={data?.title ?? t("nav.catalogue")}>
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </PageHeader>
      <Loading data={data} error={error} />
      {data && <ListingView l={data} mediaPath={(m) => `${base}/${id}/media/${m}/content`} />}
    </section>
  );
}

/** Published listings of all suppliers, reviewed by Chuo-Sogo. */
export function Catalogue({ active }: { active: MembershipView }) {
  const { t, i18n } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/catalogue`;
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const { data, error } = useApi<ListingSummary[]>(search ? `${base}?q=${encodeURIComponent(search)}` : base);
  const [open, setOpen] = useState<string | null>(null);
  if (open) return <Item base={base} id={open} onBack={() => setOpen(null)} />;
  return (
    <section>
      <PageHeader title={t("nav.catalogue")}>
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            setSearch(query.trim());
          }}
        >
          <input
            type="search"
            maxLength={100}
            aria-label={t("catalogue:search")}
            placeholder={t("catalogue:search")}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <button type="submit">{t("catalogue:find")}</button>
        </form>
      </PageHeader>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("catalogue:nothingFound")}</p>}
      <div className="card-grid" data-testid="catalogue-grid">
        {data?.map((l) => (
          <button type="button" key={l.id} className="card product-card" onClick={() => setOpen(l.id)}>
            {l.cover_media_id && (
              <ListingPhoto
                path={`${base}/${l.id}/media/${l.cover_media_id}/content`}
                alt=""
                className="photo"
              />
            )}
            <strong>{l.title}</strong>
            <span className="muted small">
              {l.manufacturer} · <code>{l.model_number}</code> · {l.supplier_name}
            </span>
            <Price l={l} />
            {l.stock !== null && l.stock !== undefined && (
              <span className="small">
                {t("catalogue:stock")}: {new Intl.NumberFormat(i18n.language).format(l.stock)}
              </span>
            )}
          </button>
        ))}
      </div>
    </section>
  );
}
