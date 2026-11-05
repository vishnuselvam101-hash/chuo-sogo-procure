# L0 — Gap audit: procurement agent layer (2026-11-05)

Goal: an agent layer that does routine procurement work (match, chase, handle exceptions, prepare cost and negotiation work,
watch suppliers) with a human approving every consequential action. Clean-room: built from public descriptions of
agentic-procurement products, no third-party code or text.

| Capability | Exists today | Gap → phase |
|---|---|---|
| Agent runtime | ad-hoc `tokio::spawn` loops (`ops::spawn_exception_checks`, `supplier_mail::spawn`); `ai_suggestions` with provenance; no job table | queue table (SKIP LOCKED), agent registry, per-org settings + budget, exception records, worker → **L1** |
| 3-way match | `domain::matching::three_way` (exact, no tolerance), `discrepancies`, `orders::record_invoice`, `resolve_discrepancy` | per-org tolerances, matching agent that classifies each discrepancy and proposes a resolution, invoice text extraction (`extract_invoice`) → **L2** |
| Chasing | RFQ reminders, `outbox` (test/email/fax, reply tokens), IMAP poll → `mail_inbox`, `check_delivery`/`no_reply` tasks, `confirm_delivery_date` | order-acknowledgement, delivery-date, overdue-delivery and missing-invoice chases; ja/en templates; reply parsing (`parse_supplier_reply`) → proposed updates → **L3** |
| Exception inbox | `tasks` (kinds incl. exception), `ops/queue`, OpsQueue.tsx | one inbox of agent exceptions with proposed action, approve/edit/reject → **L1 backend, L4/L8 UI** |
| Should-cost | `history` (price history, summary, alerts), `opportunity::assess`, `savings`, `comparison`, `decision` | cost build-up model (material, process, overhead, logistics, margin), quote-gap analysis → **L5** |
| Negotiation prep | pieces above | negotiation brief (deterministic facts + AI narrative in ja) → **L5** |
| Supplier lifecycle | `suppliers.status`, `customer_supplier_approvals`, `shortlist`, `supplier_documents` (quotes only) | compliance documents with expiry (incl. qualified-invoice registration number), QCD scorecards, expiry/risk agent → **L6** |
| Integrations | `request_import` (CSV/TSV/xlsx requests), `exports/orders.csv`, `savings.csv` | ERP-neutral connector interface; CSV/xlsx import of POs, receipts, invoices with saved column-mapping profiles; agent-results export → **L7** |
| Agent console | — | dashboard: runs, exceptions, time and savings, settings (ja first) → **L8** |
| Hardening | gate, ZAP script, injection corpus for extraction | injection corpus for supplier replies, load test of the worker, E2E for agents → **L9** |
