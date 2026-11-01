import { useState } from "react";
import { useTranslation } from "react-i18next";
import { formatDate, formatNumber } from "../lib/format";
import type { ItemView, MembershipView, RequestDetail as Detail, SupplierView } from "../lib/types";
import { useAction } from "../lib/useAction";
import { useApi } from "../lib/useApi";
import { ErrorText, Loading, PageHeader, StatusBadge } from "../components/ui";
import { ItemFields, emptyItem, toItemInput } from "./ItemFields";
import { RecommendationPanel } from "./RecommendationPanel";
import { RfqPanel } from "./RfqPanel";
import { P } from "../lib/perm";

const EDITABLE = ["draft", "needs_clarification"];
const RFQ_STATES = ["sourcing", "rfq_sent", "quotes_received", "under_comparison"];

interface Props {
  active: MembershipView;
  selfId: string;
  requestId: string;
  onBack: () => void;
}

export function RequestDetail({ active, selfId, requestId, onBack }: Props) {
  const { t, i18n } = useTranslation();
  const base = `/api/v1/orgs/${active.org_id}/requests/${requestId}`;
  const { data, error, reload } = useApi<Detail>(base);
  const action = useAction();
  const [answer, setAnswer] = useState("");
  const [question, setQuestion] = useState("");
  const [newItem, setNewItem] = useState(emptyItem());

  if (!data) return <Loading data={data} error={error} />;
  const r = data.request;
  const has = (p: string) => active.permissions.includes(p);
  const operator = active.access === "assigned";
  const owner = r.requested_by === selfId && has(P.submitRequest);
  const editable = owner && EDITABLE.includes(r.status);
  const canReadQuotes = has(P.manageRfqs) || has(P.recordQuotations) || has(P.reviewQuotations);

  async function post(path: string, body: unknown = {}) {
    if (await action.run("POST", `${base}${path}`, body)) reload();
  }

  async function addItem() {
    const item = toItemInput(newItem);
    if (!item) return action.setError("validation_failed/quantity");
    if (await action.run("POST", `${base}/items`, item)) {
      setNewItem(emptyItem());
      reload();
    }
  }

  return (
    <section>
      <PageHeader title={`${r.number} — ${r.title}`}>
        <StatusBadge status={r.status} />
        <button type="button" onClick={reload}>
          {t("action.refresh")}
        </button>
        <button type="button" onClick={onBack}>
          {t("action.back")}
        </button>
      </PageHeader>

      <div className="card grid-2">
        <div>
          <div className="muted">{t("procurement:request.currency")}</div>
          <div>{r.currency}</div>
        </div>
        <div>
          <div className="muted">{t("procurement:request.neededBy")}</div>
          <div>{r.needed_by ? formatDate(r.needed_by, i18n.language) : "—"}</div>
        </div>
        <div>
          <div className="muted">{t("procurement:request.requester")}</div>
          <div>{r.requester_name ?? ""}</div>
        </div>
        <div>
          <div className="muted">{t("procurement:request.deliveryLocation")}</div>
          <div>{r.delivery_location || "—"}</div>
        </div>
        {r.description && <p className="span-2 pre">{r.description}</p>}
      </div>

      {r.clarification_question && (
        <div className="card notice">
          <div className="muted">{t("procurement:clarification.question")}</div>
          <p className="pre">{r.clarification_question}</p>
          {r.clarification_answer && (
            <>
              <div className="muted">{t("procurement:clarification.answer")}</div>
              <p className="pre">{r.clarification_answer}</p>
            </>
          )}
        </div>
      )}

      <h2>{t("procurement:item.heading")}</h2>
      <table className="table">
        <thead>
          <tr>
            <th className="num">{t("procurement:item.line")}</th>
            <th>{t("procurement:item.description")}</th>
            <th>{t("procurement:item.manufacturer")}</th>
            <th>{t("procurement:item.mpn")}</th>
            <th className="num">{t("procurement:item.quantity")}</th>
            <th>{t("procurement:item.verification")}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {data.items.map((item) => (
            <ItemRow
              key={item.id}
              item={item}
              canVerify={operator && has(P.verifyProducts) && r.status === "identifying"}
              canDelete={editable}
              onVerify={(body) => post(`/items/${item.id}/verification`, body)}
              onDelete={async () => {
                if (await action.run("DELETE", `${base}/items/${item.id}`)) reload();
              }}
            />
          ))}
        </tbody>
      </table>

      {editable && (
        <div className="card item-row">
          <ItemFields value={newItem} onChange={setNewItem} />
          <button type="button" onClick={() => void addItem()} disabled={action.busy}>
            {t("procurement:item.add")}
          </button>
        </div>
      )}

      {(owner || operator || action.error) && (
        <div className="card stack">
          {owner && r.status === "needs_clarification" && (
            <label>
              {t("procurement:clarification.answer")}
              <textarea
                rows={3}
                maxLength={2000}
                value={answer}
                onChange={(e) => setAnswer(e.target.value)}
              />
            </label>
          )}
          <div className="row">
            {editable && (
              <button
                type="button"
                className="primary"
                disabled={action.busy}
                onClick={() => void post("/submit", { answer: answer || null })}
              >
                {t("procurement:request.submit")}
              </button>
            )}
            {owner && ["draft", "submitted", "needs_clarification"].includes(r.status) && (
              <button
                type="button"
                disabled={action.busy}
                onClick={() => void post("/cancel", { reason: "" })}
              >
                {t("procurement:request.cancel")}
              </button>
            )}
            {operator && r.status === "submitted" && (
              <button type="button" className="primary" onClick={() => void post("/pick-up")}>
                {t("procurement:ops.pickUp")}
              </button>
            )}
            {operator && r.status === "identifying" && (
              <button type="button" className="primary" onClick={() => void post("/start-sourcing")}>
                {t("procurement:ops.startSourcing")}
              </button>
            )}
          </div>
          {operator && ["submitted", "identifying"].includes(r.status) && (
            <div className="row">
              <label className="grow">
                {t("procurement:clarification.ask")}
                <input maxLength={2000} value={question} onChange={(e) => setQuestion(e.target.value)} />
              </label>
              <button
                type="button"
                disabled={!question.trim() || action.busy}
                onClick={() => void post("/clarification", { question }).then(() => setQuestion(""))}
              >
                {t("procurement:clarification.send")}
              </button>
            </div>
          )}
          <ErrorText code={action.error} />
        </div>
      )}

      {(data.rfqs.length > 0 || (operator && RFQ_STATES.includes(r.status))) && (
        <h2>{t("procurement:rfq.heading")}</h2>
      )}
      {operator && has(P.manageRfqs) && RFQ_STATES.includes(r.status) && (
        <NewRfq base={base} onCreated={reload} />
      )}
      {data.rfqs.map((rfq) =>
        canReadQuotes ? (
          <RfqPanel
            key={rfq.id}
            active={active}
            rfqId={rfq.id}
            requestId={r.id}
            requestStatus={r.status}
            currency={r.currency}
            items={data.items}
            onChanged={reload}
          />
        ) : (
          <div key={rfq.id} className="card row">
            <strong>{rfq.number}</strong>
            <StatusBadge status={rfq.status} />
            <span className="muted">
              {t("procurement:rfq.counts", {
                recipients: formatNumber(rfq.recipients, i18n.language),
                quotations: formatNumber(rfq.quotations, i18n.language),
              })}
            </span>
          </div>
        ),
      )}

      {data.recommendation && (
        <RecommendationPanel
          active={active}
          selfId={selfId}
          recommendationId={data.recommendation.id}
          onChanged={reload}
        />
      )}
    </section>
  );
}

interface ItemRowProps {
  item: ItemView;
  canVerify: boolean;
  canDelete: boolean;
  onVerify: (body: { manufacturer: string; mpn: string; evidence: string }) => Promise<void>;
  onDelete: () => Promise<void>;
}

function ItemRow({ item, canVerify, canDelete, onVerify, onDelete }: ItemRowProps) {
  const { t, i18n } = useTranslation();
  const [verifying, setVerifying] = useState(false);
  const [form, setForm] = useState({ manufacturer: item.manufacturer, mpn: item.mpn, evidence: "" });
  const verified = item.verification_status === "verified";
  return (
    <>
      <tr>
        <td className="num">{item.line_no}</td>
        <td>{item.description}</td>
        <td>{verified ? item.verified_manufacturer : item.manufacturer}</td>
        <td>{verified ? item.verified_mpn : item.mpn}</td>
        <td className="num">
          {formatNumber(item.quantity, i18n.language)} {item.unit}
        </td>
        <td>
          <StatusBadge status={item.verification_status} />
          {verified && item.verification_evidence && (
            <div className="muted small">{item.verification_evidence}</div>
          )}
        </td>
        <td className="row-actions">
          {canVerify && (
            <button type="button" onClick={() => setVerifying(!verifying)}>
              {t("procurement:item.verify")}
            </button>
          )}
          {canDelete && (
            <button type="button" onClick={() => void onDelete()}>
              {t("procurement:item.remove")}
            </button>
          )}
        </td>
      </tr>
      {verifying && (
        <tr>
          <td colSpan={7}>
            <div className="item-fields">
              <label>
                {t("procurement:item.manufacturer")}
                <input
                  value={form.manufacturer}
                  onChange={(e) => setForm({ ...form, manufacturer: e.target.value })}
                />
              </label>
              <label>
                {t("procurement:item.mpn")}
                <input
                  required
                  value={form.mpn}
                  onChange={(e) => setForm({ ...form, mpn: e.target.value })}
                />
              </label>
              <label className="grow">
                {t("procurement:item.evidence")}
                <input
                  required
                  placeholder={t("procurement:item.evidenceHint")}
                  value={form.evidence}
                  onChange={(e) => setForm({ ...form, evidence: e.target.value })}
                />
              </label>
              <button
                type="button"
                className="primary"
                disabled={!form.mpn.trim() || !form.evidence.trim()}
                onClick={() => void onVerify(form).then(() => setVerifying(false))}
              >
                {t("procurement:item.confirmVerified")}
              </button>
            </div>
          </td>
        </tr>
      )}
    </>
  );
}

function NewRfq({ base, onCreated }: { base: string; onCreated: () => void }) {
  const { t } = useTranslation();
  const { data } = useApi<SupplierView[]>("/api/v1/suppliers?status=verified");
  const action = useAction();
  const [chosen, setChosen] = useState<string[]>([]);
  const [due, setDue] = useState("");
  const [message, setMessage] = useState("");

  async function create() {
    const res = await action.run("POST", `${base}/rfqs`, {
      supplier_ids: chosen,
      response_due: due || null,
      message,
    });
    if (res) {
      setChosen([]);
      setMessage("");
      onCreated();
    }
  }

  return (
    <div className="card stack">
      <strong>{t("procurement:rfq.new")}</strong>
      {data === null ? (
        <p className="muted">{t("state.loading")}</p>
      ) : data.length === 0 ? (
        <p className="muted">{t("procurement:rfq.noVerifiedSuppliers")}</p>
      ) : (
        <div className="checks">
          {data.map((s) => (
            <label key={s.id} className="radio">
              <input
                type="checkbox"
                checked={chosen.includes(s.id)}
                onChange={(e) =>
                  setChosen(e.target.checked ? [...chosen, s.id] : chosen.filter((c) => c !== s.id))
                }
              />
              {s.name} ({s.country})
            </label>
          ))}
        </div>
      )}
      <div className="row">
        <label>
          {t("procurement:rfq.responseDue")}
          <input type="date" value={due} onChange={(e) => setDue(e.target.value)} />
        </label>
        <label className="grow">
          {t("procurement:rfq.message")}
          <input maxLength={4000} value={message} onChange={(e) => setMessage(e.target.value)} />
        </label>
      </div>
      <p className="muted small">{t("procurement:rfq.anonymityNote")}</p>
      <div className="row">
        <button
          type="button"
          className="primary"
          disabled={chosen.length === 0 || action.busy}
          onClick={() => void create()}
        >
          {t("procurement:rfq.create")}
        </button>
      </div>
      <ErrorText code={action.error} />
    </div>
  );
}
