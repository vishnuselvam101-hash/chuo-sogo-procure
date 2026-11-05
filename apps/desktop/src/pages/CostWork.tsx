import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  bpsLabel,
  type BriefView,
  type CostModelInput,
  type CostModelView,
  type QuotationFacts,
  type SuggestionView,
} from "../lib/agents";
import {
  formatDateTime,
  formatMilliPrice,
  minorToInput,
  parseMinor,
  parseScaled,
  percentToBps,
} from "../lib/format";
import { P } from "../lib/perm";
import type { MembershipView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { EmptyState, ErrorText, Loading, Money, PageHeader } from "../components/ui";

const TABS = ["briefs", "models"] as const;
type Tab = (typeof TABS)[number];
const DRAFT = "draft";
const FINAL = "final";
const UNKNOWN = "unknown";

/** L5: should-cost models and negotiation briefs (reference material for people). */
export function CostWork({ active }: { active: MembershipView }) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("briefs");
  const canWrite = active.permissions.includes(P.managePurchaseHistory);
  return (
    <section>
      <PageHeader title={t("nav.cost")} />
      <p className="lead">{t("agents:ui.cost.lead")}</p>
      <div className="tabs" role="tablist">
        {TABS.map((id) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            className={`tab${tab === id ? " active" : ""}`}
            onClick={() => setTab(id)}
          >
            {t(`agents:ui.cost.tab.${id}`)}
          </button>
        ))}
      </div>
      {tab === "briefs" ? (
        <Briefs active={active} canWrite={canWrite} />
      ) : (
        <Models active={active} canWrite={canWrite} />
      )}
    </section>
  );
}

/** Deterministic facts about one quotation, line by line. */
export function FactsTable({ facts }: { facts: QuotationFacts }) {
  const { t, i18n } = useTranslation();
  const cur = facts.currency;
  const milli = (v: number | null | undefined) =>
    v === null || v === undefined ? "—" : formatMilliPrice(v, cur, i18n.language);
  return (
    <div className="stack" data-testid="facts">
      <dl className="stats">
        <div>
          <dt>{t("agents:cost.facts.quotedTotal")}</dt>
          <dd>
            <Money minor={facts.quoted_total_minor} currency={cur} />
          </dd>
        </div>
        <div>
          <dt>{t("agents:cost.facts.savings")}</dt>
          <dd>
            <Money minor={facts.savings_opportunity_minor} currency={cur} />
          </dd>
        </div>
      </dl>
      <p className="small">
        {facts.supplier_name} · {facts.rfq_number}
        {facts.superseded && <span className="badge badge-warn">{t("agents:ui.cost.superseded")}</span>}
      </p>
      <div className="table-scroll">
        <table className="table">
          <thead>
            <tr>
              <th className="num">{t("procurement:item.line")}</th>
              <th>{t("procurement:item.description")}</th>
              <th className="num">{t("agents:ui.cost.quoted")}</th>
              <th className="num">{t("agents:cost.facts.shouldCost")}</th>
              <th className="num">{t("agents:cost.facts.shouldCostGap")}</th>
              <th className="num">{t("agents:cost.facts.historicalLow")}</th>
              <th className="num">{t("agents:cost.facts.historyGap")}</th>
              <th className="num">{t("agents:cost.facts.lastPaid")}</th>
              <th className="num">{t("agents:cost.facts.competing")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {facts.lines.map((l) => (
              <tr key={l.request_item_id}>
                <td className="num">{l.line_no}</td>
                <td>
                  {l.description}
                  {l.alternative && (
                    <span className="badge badge-warn">{t("procurement:quote.alternative")}</span>
                  )}
                </td>
                <td className="num">
                  <Money minor={l.quoted_unit_minor} currency={cur} />
                </td>
                <td className="num">{milli(l.should_cost_milli)}</td>
                <td className="num">{bpsLabel(l.should_cost_gap_bps) ?? "—"}</td>
                <td className="num">{milli(l.historical_low_milli)}</td>
                <td className="num">{bpsLabel(l.history_gap_bps) ?? "—"}</td>
                <td className="num">{milli(l.last_paid_milli)}</td>
                <td className="num">{l.competing_quotes}</td>
                <td>
                  {l.signals.map((s) => (
                    <span key={s} className="badge badge-warn">
                      {t(`agents:cost.signal.${s}`, { defaultValue: s })}
                    </span>
                  ))}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function Briefs({ active, canWrite }: { active: MembershipView; canWrite: boolean }) {
  const { t, i18n } = useTranslation();
  const [status, setStatus] = useState("");
  const base = `/api/v1/orgs/${active.org_id}/cost/briefs`;
  const { data, error, reload } = useApi<BriefView[]>(status ? `${base}?status=${status}` : base);
  const [open, setOpen] = useState<string | null>(null);
  if (open)
    return (
      <BriefDetail
        key={open}
        active={active}
        id={open}
        canWrite={canWrite}
        onBack={() => {
          setOpen(null);
          reload();
        }}
      />
    );
  return (
    <div className="stack">
      <label className="compact">
        {t("agents:ui.filter.status")}
        <select value={status} onChange={(e) => setStatus(e.target.value)}>
          <option value="">{t("agents:ui.filter.all")}</option>
          <option value={DRAFT}>{t("agents:cost.briefStatus.draft")}</option>
          <option value={FINAL}>{t("agents:cost.briefStatus.final")}</option>
        </select>
      </label>
      <Loading data={data} error={error} />
      {data && data.length === 0 && <EmptyState hint={t("agents:ui.cost.briefsEmpty")} />}
      {data && data.length > 0 && (
        <table className="table" data-testid="briefs">
          <thead>
            <tr>
              <th>{t("agents:ui.cost.quotation")}</th>
              <th className="num">{t("agents:cost.facts.quotedTotal")}</th>
              <th className="num">{t("agents:cost.facts.savings")}</th>
              <th>{t("agents:ui.col.status")}</th>
              <th>{t("agents:ui.col.updated")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {data.map((b) => (
              <tr key={b.id}>
                <td>
                  {b.facts.supplier_name} · {b.facts.rfq_number}
                </td>
                <td className="num">
                  <Money minor={b.quoted_total_minor} currency={b.currency} />
                </td>
                <td className="num">
                  <Money minor={b.facts.savings_opportunity_minor} currency={b.currency} />
                </td>
                <td>
                  <span className={`badge badge-brief-${b.status}`}>
                    {t(`agents:cost.briefStatus.${b.status}`)}
                  </span>
                </td>
                <td className="small">{formatDateTime(b.updated_at, i18n.language, active.timezone)}</td>
                <td className="row-actions">
                  <button type="button" onClick={() => setOpen(b.id)}>
                    {t("agents:ui.inbox.open")}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function BriefDetail({
  active,
  id,
  canWrite,
  onBack,
}: {
  active: MembershipView;
  id: string;
  canWrite: boolean;
  onBack: () => void;
}) {
  const { t } = useTranslation();
  const path = `/api/v1/orgs/${active.org_id}/cost/briefs/${id}`;
  const { data, error, reload } = useApi<BriefView>(path);
  const action = useAction();
  const [form, setForm] = useState<{ summary: string; args: string; target: string; used: boolean } | null>(
    null,
  );
  const [ai, setAi] = useState<SuggestionView | null>(null);
  if (!data) return <Loading data={data} error={error} />;
  const cur = data.currency;
  const editable = canWrite && data.status === DRAFT;
  const f = form ?? {
    summary: data.summary,
    args: data.arguments.join("\n"),
    target:
      data.target_price_minor === null || data.target_price_minor === undefined
        ? ""
        : minorToInput(data.target_price_minor, cur),
    used: false,
  };

  async function draftNarrative() {
    const res = await action.run<{ brief: BriefView; suggestion: SuggestionView }>(
      "POST",
      `${path}/narrative`,
    );
    if (res) setAi(res.body.suggestion);
  }
  function takeSuggestion(s: SuggestionView) {
    const out = s.output as { summary?: string; arguments?: string[]; target_price_minor?: number | null };
    setForm({
      summary: out.summary ?? "",
      args: (out.arguments ?? []).join("\n"),
      target: typeof out.target_price_minor === "number" ? minorToInput(out.target_price_minor, cur) : "",
      used: true,
    });
  }
  async function finalise(e: FormEvent) {
    e.preventDefault();
    const target = f.target.trim() === "" ? null : parseMinor(f.target, cur);
    if (f.target.trim() !== "" && target === null)
      return action.setError("validation_failed/target_price_minor");
    const res = await action.run("POST", `${path}/finalise`, {
      summary: f.summary,
      arguments: f.args
        .split("\n")
        .map((a) => a.trim())
        .filter(Boolean),
      target_price_minor: target,
      used_suggestion: f.used,
    });
    if (res) {
      setForm(null);
      reload();
    }
  }
  const out = (ai?.output ?? {}) as { summary?: string; arguments?: string[]; flags?: string[] };

  return (
    <div className="stack" data-testid="brief">
      <div className="row">
        <h2 className="grow">
          {data.facts.supplier_name} · {data.facts.rfq_number}
        </h2>
        <span className={`badge badge-brief-${data.status}`}>
          {t(`agents:cost.briefStatus.${data.status}`)}
        </span>
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </div>
      <FactsTable facts={data.facts} />
      {editable && (
        <div className="sub-card ai-assist stack">
          <p className="muted small">{t("agents:cost.narrative.aiNote")}</p>
          <button type="button" disabled={action.busy} onClick={() => void draftNarrative()}>
            {t("agents:cost.narrative.draft")}
          </button>
          {ai && ai.status === "failed" && <ErrorText code={ai.error_code ?? UNKNOWN} />}
          {ai && ai.status !== "failed" && (
            <div className="stack" data-testid="narrative">
              <p className="pre">{out.summary}</p>
              <ul>
                {(out.arguments ?? []).map((a, i) => (
                  <li key={i}>{a}</li>
                ))}
              </ul>
              {(out.flags ?? []).map((fl) => (
                <span key={fl} className="badge badge-warn">
                  {t(`agents:cost.narrative.flag.${fl}`, { defaultValue: fl })}
                </span>
              ))}
              <button type="button" onClick={() => takeSuggestion(ai)}>
                {t("agents:ui.cost.useText")}
              </button>
            </div>
          )}
        </div>
      )}
      {editable ? (
        <form className="stack" onSubmit={(e) => void finalise(e)}>
          <label>
            {t("agents:ui.cost.summary")}
            <textarea
              rows={4}
              maxLength={4000}
              value={f.summary}
              onChange={(e) => setForm({ ...f, summary: e.target.value })}
            />
          </label>
          <label>
            {t("agents:ui.cost.arguments")}
            <textarea rows={5} value={f.args} onChange={(e) => setForm({ ...f, args: e.target.value })} />
          </label>
          <label>
            {t("agents:cost.facts.targetPrice")}
            <input
              inputMode="decimal"
              value={f.target}
              onChange={(e) => setForm({ ...f, target: e.target.value })}
            />
          </label>
          <button type="submit" className="primary" disabled={action.busy || !f.summary.trim()}>
            {t("agents:ui.cost.finalise")}
          </button>
        </form>
      ) : (
        <div className="stack">
          <p className="pre">{data.summary}</p>
          <ul>
            {data.arguments.map((a, i) => (
              <li key={i}>{a}</li>
            ))}
          </ul>
          {data.target_price_minor !== null && data.target_price_minor !== undefined && (
            <p>
              {t("agents:cost.facts.targetPrice")}: <Money minor={data.target_price_minor} currency={cur} />
            </p>
          )}
        </div>
      )}
      <ErrorText code={action.error} />
    </div>
  );
}

interface MaterialRow {
  name: string;
  unit: string;
  qty: string;
  price: string;
  scrap: string;
}
interface ProcessRow {
  name: string;
  cycle: string;
  rate: string;
  setup: string;
}
const EMPTY_MATERIAL: MaterialRow = { name: "", unit: "kg", qty: "", price: "", scrap: "0" };
const EMPTY_PROCESS: ProcessRow = { name: "", cycle: "", rate: "", setup: "0" };
const EMPTY_MODEL = {
  name: "",
  mpn: "",
  description: "",
  currency: "JPY",
  lot: "100",
  overhead: "0",
  margin: "0",
  logistics: "0",
  notes: "",
  materials: [EMPTY_MATERIAL],
  processes: [EMPTY_PROCESS],
};

/** Form values → the API body; null when a value cannot be read exactly. */
export function toModelInput(f: typeof EMPTY_MODEL): CostModelInput | null {
  const lot = /^\d{1,9}$/.test(f.lot.trim()) ? Number(f.lot.trim()) : null;
  const overhead = percentToBps(f.overhead);
  const margin = percentToBps(f.margin);
  const logistics = parseMinor(f.logistics, f.currency);
  if (lot === null || overhead === null || margin === null || logistics === null) return null;
  const materials = f.materials
    .filter((m) => m.name.trim())
    .map((m) => ({
      name: m.name.trim(),
      unit: m.unit.trim(),
      quantity_milli: parseScaled(m.qty, 3),
      unit_price_minor: parseMinor(m.price, f.currency),
      scrap_bps: percentToBps(m.scrap),
    }));
  const processes = f.processes
    .filter((p) => p.name.trim())
    .map((p) => ({
      name: p.name.trim(),
      cycle_ms: parseScaled(p.cycle, 3),
      rate_minor_per_hour: parseMinor(p.rate, f.currency),
      setup_minutes: /^\d{1,6}$/.test(p.setup.trim()) ? Number(p.setup.trim()) : null,
    }));
  if (materials.some((m) => m.quantity_milli === null || m.unit_price_minor === null || m.scrap_bps === null))
    return null;
  if (
    processes.some((p) => p.cycle_ms === null || p.rate_minor_per_hour === null || p.setup_minutes === null)
  )
    return null;
  return {
    name: f.name.trim(),
    mpn: f.mpn.trim(),
    description: f.description.trim(),
    currency: f.currency,
    notes: f.notes,
    inputs: {
      lot_size: lot,
      overhead_bps: overhead,
      margin_bps: margin,
      logistics_minor_per_lot: logistics,
      materials: materials as NonNullable<CostModelInput["inputs"]["materials"]>,
      processes: processes as NonNullable<CostModelInput["inputs"]["processes"]>,
    },
  };
}

function Models({ active, canWrite }: { active: MembershipView; canWrite: boolean }) {
  const { t, i18n } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/cost/models`;
  const { data, error, reload } = useApi<CostModelView[]>(base);
  const action = useAction();
  const [form, setForm] = useState<typeof EMPTY_MODEL | null>(null);
  const [open, setOpen] = useState<string | null>(null);

  async function save(e: FormEvent) {
    e.preventDefault();
    if (!form) return;
    const body = toModelInput(form);
    if (!body) return action.setError("validation_failed/inputs");
    if (await action.run("POST", base, body)) {
      setForm(null);
      reload();
    }
  }
  const setMat = (i: number, patch: Partial<MaterialRow>) =>
    form && setForm({ ...form, materials: form.materials.map((m, j) => (j === i ? { ...m, ...patch } : m)) });
  const setProc = (i: number, patch: Partial<ProcessRow>) =>
    form && setForm({ ...form, processes: form.processes.map((p, j) => (j === i ? { ...p, ...patch } : p)) });
  const opened = data?.find((m) => m.id === open);

  return (
    <div className="stack">
      {canWrite && !form && (
        <div className="row">
          <button type="button" className="primary" onClick={() => setForm(EMPTY_MODEL)}>
            {t("agents:ui.cost.newModel")}
          </button>
        </div>
      )}
      {form && (
        <form className="card stack" onSubmit={(e) => void save(e)} aria-label={t("agents:ui.cost.newModel")}>
          <div className="grid-2">
            <label>
              {t("agents:ui.cost.modelName")}
              <input
                maxLength={200}
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
              />
            </label>
            <label>
              {t("agents:ui.cost.mpn")}
              <input
                maxLength={200}
                value={form.mpn}
                onChange={(e) => setForm({ ...form, mpn: e.target.value })}
              />
            </label>
            <label>
              {t("agents:ui.cost.description")}
              <input
                maxLength={500}
                value={form.description}
                onChange={(e) => setForm({ ...form, description: e.target.value })}
              />
            </label>
            <label>
              {t("agents:ui.cost.currency")}
              <select value={form.currency} onChange={(e) => setForm({ ...form, currency: e.target.value })}>
                {["JPY", "USD", "EUR", "INR", "CNY", "KRW"].map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("agents:ui.cost.lotSize")}
              <input
                inputMode="numeric"
                value={form.lot}
                onChange={(e) => setForm({ ...form, lot: e.target.value })}
              />
            </label>
            <label>
              {t("agents:ui.cost.logistics")}
              <input
                inputMode="decimal"
                value={form.logistics}
                onChange={(e) => setForm({ ...form, logistics: e.target.value })}
              />
            </label>
            <label>
              {t("agents:ui.cost.overhead")}
              <input
                inputMode="decimal"
                value={form.overhead}
                onChange={(e) => setForm({ ...form, overhead: e.target.value })}
              />
            </label>
            <label>
              {t("agents:ui.cost.margin")}
              <input
                inputMode="decimal"
                value={form.margin}
                onChange={(e) => setForm({ ...form, margin: e.target.value })}
              />
            </label>
          </div>
          <h3>{t("agents:cost.model.material")}</h3>
          {form.materials.map((m, i) => (
            <div key={i} className="row">
              <label className="grow">
                {t("agents:ui.cost.materialName")}
                <input maxLength={200} value={m.name} onChange={(e) => setMat(i, { name: e.target.value })} />
              </label>
              <label>
                {t("agents:ui.cost.unit")}
                <input maxLength={20} value={m.unit} onChange={(e) => setMat(i, { unit: e.target.value })} />
              </label>
              <label>
                {t("agents:ui.cost.quantityPerPiece")}
                <input
                  inputMode="decimal"
                  value={m.qty}
                  onChange={(e) => setMat(i, { qty: e.target.value })}
                />
              </label>
              <label>
                {t("agents:ui.cost.unitPrice")}
                <input
                  inputMode="decimal"
                  value={m.price}
                  onChange={(e) => setMat(i, { price: e.target.value })}
                />
              </label>
              <label>
                {t("agents:ui.cost.scrap")}
                <input
                  inputMode="decimal"
                  value={m.scrap}
                  onChange={(e) => setMat(i, { scrap: e.target.value })}
                />
              </label>
            </div>
          ))}
          <div className="row">
            <button
              type="button"
              onClick={() => setForm({ ...form, materials: [...form.materials, EMPTY_MATERIAL] })}
            >
              {t("agents:ui.cost.addMaterial")}
            </button>
          </div>
          <h3>{t("agents:cost.model.process")}</h3>
          {form.processes.map((p, i) => (
            <div key={i} className="row">
              <label className="grow">
                {t("agents:ui.cost.processName")}
                <input
                  maxLength={200}
                  value={p.name}
                  onChange={(e) => setProc(i, { name: e.target.value })}
                />
              </label>
              <label>
                {t("agents:ui.cost.cycleSeconds")}
                <input
                  inputMode="decimal"
                  value={p.cycle}
                  onChange={(e) => setProc(i, { cycle: e.target.value })}
                />
              </label>
              <label>
                {t("agents:ui.cost.ratePerHour")}
                <input
                  inputMode="decimal"
                  value={p.rate}
                  onChange={(e) => setProc(i, { rate: e.target.value })}
                />
              </label>
              <label>
                {t("agents:ui.cost.setupMinutes")}
                <input
                  inputMode="numeric"
                  value={p.setup}
                  onChange={(e) => setProc(i, { setup: e.target.value })}
                />
              </label>
            </div>
          ))}
          <div className="row">
            <button
              type="button"
              onClick={() => setForm({ ...form, processes: [...form.processes, EMPTY_PROCESS] })}
            >
              {t("agents:ui.cost.addProcess")}
            </button>
          </div>
          <label>
            {t("agents:ui.cost.notes")}
            <textarea
              maxLength={4000}
              rows={2}
              value={form.notes}
              onChange={(e) => setForm({ ...form, notes: e.target.value })}
            />
          </label>
          <div className="row">
            <button type="submit" className="primary" disabled={action.busy || !form.name.trim()}>
              {t("action.save")}
            </button>
            <button type="button" onClick={() => setForm(null)}>
              {t("action.cancel")}
            </button>
          </div>
        </form>
      )}
      <ErrorText code={action.error} />
      <Loading data={data} error={error} />
      {data && data.length === 0 && <EmptyState hint={t("agents:ui.cost.modelsEmpty")} />}
      {data && data.length > 0 && (
        <table className="table" data-testid="models">
          <thead>
            <tr>
              <th>{t("agents:ui.cost.modelName")}</th>
              <th>{t("agents:cost.model.version")}</th>
              <th className="num">{t("agents:cost.model.unitCost")}</th>
              <th>{t("agents:ui.col.updated")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {data.map((m) => (
              <tr key={m.id}>
                <td>
                  {m.name}
                  <div className="muted small">{m.item_key}</div>
                </td>
                <td>{m.version}</td>
                <td className="num">{formatMilliPrice(m.unit_cost_milli, m.currency, i18n.language)}</td>
                <td className="small">{formatDateTime(m.created_at, i18n.language, active.timezone)}</td>
                <td className="row-actions">
                  <button type="button" onClick={() => setOpen(open === m.id ? null : m.id)}>
                    {open === m.id ? t("agents:ui.inbox.close") : t("agents:ui.inbox.open")}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {opened && <ModelBreakdown model={opened} />}
    </div>
  );
}

function ModelBreakdown({ model }: { model: CostModelView }) {
  const { t, i18n } = useTranslation();
  const b = model.breakdown;
  const price = (v: number) => formatMilliPrice(v, model.currency, i18n.language);
  const parts = [
    ["material", b.material_milli],
    ["process", b.process_milli],
    ["overhead", b.overhead_milli],
    ["logistics", b.logistics_milli],
    ["margin", b.margin_milli],
  ] as const;
  return (
    <div className="card stack" data-testid="breakdown">
      <h3>{model.name}</h3>
      <dl className="stats">
        {parts.map(([k, v]) => (
          <div key={k}>
            <dt>{t(`agents:cost.model.${k}`)}</dt>
            <dd>{price(v)}</dd>
          </div>
        ))}
        <div>
          <dt>{t("agents:cost.model.unitCost")}</dt>
          <dd>{price(model.unit_cost_milli)}</dd>
        </div>
      </dl>
      {model.drivers.length > 0 && (
        <>
          <h4>{t("agents:cost.model.drivers")}</h4>
          <ul>
            {model.drivers.map((d) => (
              <li key={d.input}>
                {t("agents:ui.cost.driver", {
                  name: d.name,
                  delta: price(d.delta_milli),
                  pct: bpsLabel(d.delta_bps),
                })}
              </li>
            ))}
          </ul>
        </>
      )}
      {model.notes && <p className="pre small">{model.notes}</p>}
    </div>
  );
}
