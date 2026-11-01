import { useRef, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useApi } from "../lib/useApi";
import { useAction } from "../lib/useAction";
import { minorToInput, parseMinor } from "../lib/format";
import { MAX_UPLOAD_BYTES, readAsBase64 } from "../lib/files";
import type { ListingDetail, ListingInput, ListingSummary, MembershipView } from "../lib/types";
import { ErrorText, Loading, PageHeader } from "../components/ui";
import { ListingPhoto, ListingStatusBadge, ListingView, Price } from "../catalogue/ListingView";

const CONDITIONS = ["new", "used", "refurbished"] as const;
const RATES = [1000, 800, 0] as const;
const DEFAULT_CURRENCY = "JPY";
const PHOTO = "photo";
const DOCUMENT = "document";
const PHOTO_TYPES = ".jpg,.jpeg,.png,.webp";
const DOC_TYPES = ".pdf";
const EDITABLE = ["draft", "submitted", "rejected", "approved"];

interface Form {
  manufacturer: string;
  model_number: string;
  serial_number: string;
  condition: string;
  title: string;
  title_en: string;
  description: string;
  description_en: string;
  specs: { key: string; value: string }[];
  business: string;
  consumer: string;
  tax_rate_bps: number;
  stock: string;
}

function formFrom(l: ListingDetail | null): Form {
  const c = l?.currency ?? DEFAULT_CURRENCY;
  const money = (v: number | null | undefined) => (v === null || v === undefined ? "" : minorToInput(v, c));
  return {
    manufacturer: l?.manufacturer ?? "",
    model_number: l?.model_number ?? "",
    serial_number: l?.serial_number ?? "",
    condition: l?.condition ?? "new",
    title: l?.title ?? "",
    title_en: l?.title_en ?? "",
    description: l?.description ?? "",
    description_en: l?.description_en ?? "",
    specs: l?.specs.map((s) => ({ ...s })) ?? [],
    business: money(l?.business_price_minor),
    consumer: money(l?.consumer_price_minor),
    tax_rate_bps: l?.tax_rate_bps ?? 1000,
    stock: l?.stock === null || l?.stock === undefined ? "" : String(l.stock),
  };
}

/** Form → API body; `null` when a price or the stock cannot be read exactly. */
export function toInput(f: Form): ListingInput | null {
  const price = (s: string) => (s.trim() === "" ? null : parseMinor(s, DEFAULT_CURRENCY));
  const business = price(f.business);
  const consumer = price(f.consumer);
  if ((f.business.trim() && business === null) || (f.consumer.trim() && consumer === null)) return null;
  const stock = f.stock.trim() === "" ? null : Number(f.stock);
  if (stock !== null && (!Number.isInteger(stock) || stock < 0)) return null;
  return {
    manufacturer: f.manufacturer,
    model_number: f.model_number,
    serial_number: f.serial_number.trim() || null,
    condition: f.condition,
    title: f.title,
    title_en: f.title_en,
    description: f.description,
    description_en: f.description_en,
    specs: f.specs.filter((s) => s.key.trim() || s.value.trim()),
    currency: DEFAULT_CURRENCY,
    business_price_minor: business,
    consumer_price_minor: consumer,
    tax_rate_bps: f.tax_rate_bps,
    stock,
  };
}

function Editor({
  base,
  initial,
  onSaved,
  onCancel,
}: {
  base: string;
  initial: ListingDetail | null;
  onSaved: (id: string) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const { run, busy, error, setError } = useAction();
  const [f, setF] = useState<Form>(() => formFrom(initial));
  const set = (k: keyof Form) => (e: { target: { value: string } }) =>
    setF((x) => ({ ...x, [k]: e.target.value }));

  async function submit(e: FormEvent) {
    e.preventDefault();
    const body = toInput(f);
    if (!body) return setError("validation_failed/price");
    if (initial) {
      if (await run("PUT", `${base}/${initial.id}`, body)) onSaved(initial.id);
    } else {
      const res = await run<{ id: string }>("POST", base, body);
      if (res?.body) onSaved(res.body.id);
    }
  }

  return (
    <form className="card stack" onSubmit={submit} data-testid="listing-editor">
      <h2>{initial ? t("catalogue:editListing") : t("catalogue:newListing")}</h2>
      {initial?.status === "approved" && <p className="warn-text">{t("catalogue:editUnpublishes")}</p>}
      <div className="form-grid">
        <label>
          {t("catalogue:manufacturer")}
          <input maxLength={200} value={f.manufacturer} onChange={set("manufacturer")} />
        </label>
        <label>
          {t("catalogue:model")}
          <input maxLength={100} value={f.model_number} onChange={set("model_number")} />
        </label>
        <label>
          {t("catalogue:condition")}
          <select value={f.condition} onChange={set("condition")}>
            {CONDITIONS.map((c) => (
              <option key={c} value={c}>
                {t(`catalogue:conditions.${c}`)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("catalogue:serial")}
          <input maxLength={100} value={f.serial_number} onChange={set("serial_number")} />
        </label>
        <p className="muted small">{t("catalogue:serialHint")}</p>
        <label>
          {t("catalogue:titleJa")}
          <input maxLength={200} value={f.title} onChange={set("title")} />
        </label>
        <label>
          {t("catalogue:titleEn")}
          <input maxLength={200} value={f.title_en} onChange={set("title_en")} />
        </label>
      </div>
      <label>
        {t("catalogue:descriptionJa")}
        <textarea maxLength={5000} rows={4} value={f.description} onChange={set("description")} />
      </label>
      <label>
        {t("catalogue:descriptionEn")}
        <textarea maxLength={5000} rows={3} value={f.description_en} onChange={set("description_en")} />
      </label>
      <fieldset className="stack">
        <legend>{t("catalogue:specs")}</legend>
        {f.specs.map((s, i) => (
          <div className="row" key={i}>
            <input
              aria-label={t("catalogue:specKey")}
              placeholder={t("catalogue:specKey")}
              maxLength={100}
              value={s.key}
              onChange={(e) =>
                setF((x) => ({
                  ...x,
                  specs: x.specs.map((y, j) => (j === i ? { ...y, key: e.target.value } : y)),
                }))
              }
            />
            <input
              aria-label={t("catalogue:specValue")}
              placeholder={t("catalogue:specValue")}
              maxLength={500}
              value={s.value}
              onChange={(e) =>
                setF((x) => ({
                  ...x,
                  specs: x.specs.map((y, j) => (j === i ? { ...y, value: e.target.value } : y)),
                }))
              }
            />
            <button
              type="button"
              className="link"
              onClick={() => setF((x) => ({ ...x, specs: x.specs.filter((_, j) => j !== i) }))}
            >
              {t("catalogue:remove")}
            </button>
          </div>
        ))}
        <div className="row">
          <button
            type="button"
            disabled={f.specs.length >= 50}
            onClick={() => setF((x) => ({ ...x, specs: [...x.specs, { key: "", value: "" }] }))}
          >
            {t("catalogue:addSpec")}
          </button>
        </div>
      </fieldset>
      <div className="form-grid">
        <label>
          {t("catalogue:businessPriceExcl")}
          <input inputMode="decimal" value={f.business} onChange={set("business")} />
        </label>
        <label>
          {t("catalogue:consumerPriceIncl")}
          <input inputMode="decimal" value={f.consumer} onChange={set("consumer")} />
        </label>
        <label>
          {t("catalogue:taxRate")}
          <select
            value={f.tax_rate_bps}
            onChange={(e) => setF((x) => ({ ...x, tax_rate_bps: Number(e.target.value) }))}
          >
            {RATES.map((r) => (
              <option key={r} value={r}>
                {t(`catalogue:rates.r${r}`)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("catalogue:stock")}
          <input
            inputMode="numeric"
            placeholder={t("catalogue:onRequest")}
            value={f.stock}
            onChange={set("stock")}
          />
        </label>
      </div>
      <ErrorText code={error} />
      <div className="row">
        <button type="submit" className="primary" disabled={busy}>
          {t("action.save")}
        </button>
        <button type="button" onClick={onCancel}>
          {t("action.cancel")}
        </button>
      </div>
    </form>
  );
}

/** Photos and documents of one listing: upload, remove (any change returns it to draft). */
function MediaManager({ base, l, onChanged }: { base: string; l: ListingDetail; onChanged: () => void }) {
  const { t } = useTranslation();
  const { run, busy, error, setError } = useAction();
  const photoInput = useRef<HTMLInputElement>(null);
  const docInput = useRef<HTMLInputElement>(null);
  const editable = EDITABLE.includes(l.status);

  async function upload(kind: string, files: FileList | null) {
    for (const file of Array.from(files ?? [])) {
      if (file.size > MAX_UPLOAD_BYTES) {
        setError("payload_too_large");
        return;
      }
      const data = await readAsBase64(file);
      if (!(await run("POST", `${base}/${l.id}/media`, { kind, file_name: file.name, data_base64: data })))
        return;
    }
    onChanged();
  }

  const photos = l.media.filter((m) => m.kind === PHOTO);
  const documents = l.media.filter((m) => m.kind === DOCUMENT);
  return (
    <div className="card stack" data-testid="listing-media">
      <h2>{t("catalogue:photos")}</h2>
      <p className="muted small">{t("catalogue:photoRules")}</p>
      <div className="photo-strip">
        {photos.map((m, i) => (
          <figure key={m.id} className="photo-tile">
            <ListingPhoto
              path={`${base}/${l.id}/media/${m.id}/content`}
              alt={m.file_name}
              className="photo"
            />
            <figcaption className="small">
              {i === 0 ? t("catalogue:cover") : m.file_name} · {m.width}×{m.height}
              {editable && (
                <button
                  type="button"
                  className="link"
                  disabled={busy}
                  onClick={async () => {
                    if (await run("DELETE", `${base}/${l.id}/media/${m.id}`)) onChanged();
                  }}
                >
                  {t("catalogue:remove")}
                </button>
              )}
            </figcaption>
          </figure>
        ))}
      </div>
      {editable && (
        <div className="row">
          <input
            ref={photoInput}
            type="file"
            accept={PHOTO_TYPES}
            multiple
            hidden
            data-testid="photo-input"
            onChange={(e) => void upload(PHOTO, e.target.files)}
          />
          <button
            type="button"
            disabled={busy || photos.length >= 5}
            onClick={() => photoInput.current?.click()}
          >
            {t("catalogue:addPhotos")}
          </button>
          <input
            ref={docInput}
            type="file"
            accept={DOC_TYPES}
            hidden
            onChange={(e) => void upload(DOCUMENT, e.target.files)}
          />
          <button
            type="button"
            disabled={busy || documents.length >= 5}
            onClick={() => docInput.current?.click()}
          >
            {t("catalogue:addDocument")}
          </button>
        </div>
      )}
      {documents.length > 0 && (
        <ul className="plain">
          {documents.map((d) => (
            <li key={d.id}>
              {d.file_name}
              {editable && (
                <button
                  type="button"
                  className="link"
                  onClick={async () => {
                    if (await run("DELETE", `${base}/${l.id}/media/${d.id}`)) onChanged();
                  }}
                >
                  {t("catalogue:remove")}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      <ErrorText code={error} />
    </div>
  );
}

function Detail({ base, id, onBack }: { base: string; id: string; onBack: () => void }) {
  const { t } = useTranslation();
  const { data, error, reload } = useApi<ListingDetail>(`${base}/${id}`);
  const { run, busy, error: actionError } = useAction();
  const [editing, setEditing] = useState(false);
  if (!data) return <Loading data={data} error={error} />;
  if (editing)
    return (
      <Editor
        base={base}
        initial={data}
        onCancel={() => setEditing(false)}
        onSaved={() => {
          setEditing(false);
          reload();
        }}
      />
    );
  return (
    <section>
      <PageHeader title={data.title || t("catalogue:untitled")}>
        <ListingStatusBadge status={data.status} />
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </PageHeader>
      {data.status === "rejected" && data.review_note && (
        <p className="notice" role="status">
          {t("catalogue:rejectedBecause", { note: data.review_note })}
        </p>
      )}
      {data.status === "submitted" && <p className="muted">{t("catalogue:inReview")}</p>}
      {data.status === "approved" && <p className="ok">{t("catalogue:published")}</p>}
      <div className="card stack">
        {data.status === "draft" && data.missing.length > 0 && (
          <div data-testid="listing-missing">
            <p>{t("catalogue:missingTitle")}</p>
            <ul>
              {data.missing.map((m) => (
                <li key={m}>{t(`catalogue:missing.${m}`)}</li>
              ))}
            </ul>
          </div>
        )}
        <div className="row">
          {EDITABLE.includes(data.status) && (
            <button type="button" onClick={() => setEditing(true)}>
              {t("catalogue:edit")}
            </button>
          )}
          {data.status === "draft" && (
            <button
              type="button"
              className="primary"
              disabled={busy || data.missing.length > 0}
              onClick={async () => {
                if (await run("POST", `${base}/${id}/submit`)) reload();
              }}
            >
              {t("catalogue:submit")}
            </button>
          )}
          {data.status !== "retired" && (
            <button
              type="button"
              disabled={busy}
              onClick={async () => {
                if (await run("POST", `${base}/${id}/retire`)) reload();
              }}
            >
              {t("catalogue:retire")}
            </button>
          )}
        </div>
        <ErrorText code={actionError} />
      </div>
      <MediaManager base={base} l={data} onChanged={reload} />
      <ListingView l={data} mediaPath={(m) => `${base}/${id}/media/${m}/content`} />
    </section>
  );
}

/** Supplier workspace: the company's own product listings. */
export function Listings({ active }: { active: MembershipView }) {
  const { t } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/listings`;
  const { data, error, reload } = useApi<ListingSummary[]>(base);
  const [open, setOpen] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  if (open)
    return (
      <Detail
        key={open}
        base={base}
        id={open}
        onBack={() => {
          setOpen(null);
          reload();
        }}
      />
    );
  if (creating)
    return (
      <Editor
        base={base}
        initial={null}
        onCancel={() => setCreating(false)}
        onSaved={(id) => {
          setCreating(false);
          setOpen(id);
        }}
      />
    );
  return (
    <section>
      <PageHeader title={t("nav.listings")}>
        <button type="button" className="primary" onClick={() => setCreating(true)}>
          {t("catalogue:newListing")}
        </button>
      </PageHeader>
      <p className="muted small">{t("catalogue:supplierHint")}</p>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <p className="muted">{t("catalogue:noListings")}</p>}
      {data && data.length > 0 && (
        <table className="table" data-testid="listing-list">
          <thead>
            <tr>
              <th />
              <th>{t("catalogue:titleLabel")}</th>
              <th>{t("catalogue:model")}</th>
              <th>{t("catalogue:price")}</th>
              <th>{t("catalogue:statusLabel")}</th>
            </tr>
          </thead>
          <tbody>
            {data.map((l) => (
              <tr key={l.id}>
                <td>
                  {l.cover_media_id && (
                    <ListingPhoto
                      path={`${base}/${l.id}/media/${l.cover_media_id}/content`}
                      alt=""
                      className="thumb"
                    />
                  )}
                </td>
                <td>
                  <button type="button" className="link" onClick={() => setOpen(l.id)}>
                    {l.title || t("catalogue:untitled")}
                  </button>
                </td>
                <td>
                  <code>{l.model_number}</code>
                </td>
                <td>
                  <Price l={l} />
                </td>
                <td>
                  <ListingStatusBadge status={l.status} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
