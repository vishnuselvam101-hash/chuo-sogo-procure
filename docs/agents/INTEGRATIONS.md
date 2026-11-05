# Integrations (agent lane L7)

How the factory's own systems exchange data with Chuo-Sogo Procure. Today that is files
(CSV / TSV / xlsx exported from the factory's ERP or accounting system); the same pipeline is
meant for ERP API adapters later. Contract: `docs/agents/DESIGN.md` (L7 row).

## What comes in

| Kind | One record is | Applied through (same code as manual entry) |
|---|---|---|
| `po` | the factory's PO number for one of our approved requests (`依頼番号` = `PR-…`) | `orders::record_order_tx` (the order is the approved quotation; file lines are only compared) |
| `receipt` | goods received against a PO on one date | `orders::record_delivery_tx` |
| `invoice` | one supplier invoice for a PO, with lines, tax and shipping | `orders::record_invoice_tx` (the same three-way match, discrepancies, reconciliation, savings and purchase history) |

Record types are ERP-neutral (`procure_domain::connector`): `PurchaseOrderRecord`,
`ReceiptRecord`, `InvoiceRecord`, each with `RecordLine`s. Money is integer minor units of the
record's currency; quantities are whole units; dates are calendar days. A line names its order
line by line number (`行番号`), else part number (`品番`, any of ordered / offered / verified),
else item name (exactly one match).

## Column mapping

A **profile** (`procure_domain::connector::Profile`) is named and saved per organisation
(`integration_profiles`): explicit `source column → field` pairs, date format
(`auto` / `ymd` / `dmy` / `mdy`), decimal and thousands separators, encoding
(`auto` = UTF-8 else Shift_JIS, `utf-8`, `shift_jis`), and a default tax rate. Columns a
profile does not name are recognised by Japanese / English header synonyms (`発注番号`,
`依頼番号`, `納品日`, `請求番号`, `品番`, `数量`, `単価`, `金額`, `消費税`, `送料`, `PO Number`,
`Qty`, …); the header row may follow a title block (first 15 rows are searched).

Built-in presets are generic and neutral: `generic_ja_po`, `generic_ja_receipt`,
`generic_ja_invoice`, `generic_en_po`, `generic_en_receipt`, `generic_en_invoice`. They do not
claim compatibility with any vendor's product; a vendor export with other headers gets a saved
profile.

Reading is exact: a number with more decimals than the currency has, a line amount that is not
unit price × quantity, an unreadable date or a header value (PO, date, tax, …) that differs
between rows of one record is a **row error** (`row` 1-based as in a spreadsheet, `field`,
`code`). A record with any bad row is held back whole; nothing is guessed or rounded.

## Flow

1. **Upload → preview** `POST /api/v1/orgs/{org}/integrations/imports`
   `{kind, file_name, data_base64, profile?, config?, currency?}` — needs the permission manual
   entry of that kind needs (`record_orders` / `receive_goods` / `manage_invoices`). Creates an
   `import_batches` row (file SHA-256, profile used, mapping, counts, errors) and one
   `import_rows` row per record. **No business table changes.** A file already imported (same
   SHA-256, batch not failed) is refused (`409 duplicate_file`). Audit `import.previewed`.
2. **Agent `import`** (queued by the upload when agents are on) cross-checks the batch against
   the organisation's data: unknown request / PO numbers, POs or invoices already recorded, the
   same PO or invoice twice in the file, orders not at the right step, receipt lines not on the
   order or more than outstanding (all blocking), and amounts that differ from the order (a
   warning — for invoices the three-way match records them as discrepancies). It raises one
   exception per batch (`import:batch:<id>`):
   - `import_ready` (`info`, or `warning` with findings) with
     `proposed_action = {"type": "apply_import", "batch_id": …}`;
   - `import_blocked` (`critical`, no action) when applying would be refused.
3. **Apply** — a person approves the proposal (`agents/exceptions/{id}/decide`; the action's own
   permission is checked again for the decider) or applies directly
   (`POST …/integrations/imports/{id}/apply`). Every record goes through the manual-entry
   function with its checks, workflow transitions and audit events, plus
   `import.record_applied` (rows, entity) per record and `import.applied` for the batch. The
   first refusal rolls back the whole batch.
4. **Discard** `POST …/imports/{id}/discard` marks a previewed batch `failed` (`discarded`); the
   same file may then be uploaded again.

Other routes: `GET …/integrations/imports[?kind=]`, `GET …/imports/{id}` (with staged
records), `GET/PUT/DELETE …/integrations/profiles[/{name}]`.

## What goes out

`GET …/integrations/exports/agent-results.csv?agent=&status=` (`view_agents`): every agent
exception with its decision (created, agent, kind, severity, status, title key, subject,
params, proposed and decided action, decider, decision time, note). Existing exports:
`exports/orders.csv`, `exports/savings.csv`. All CSV is UTF-8 with a BOM; a text cell starting
with `=`, `+`, `-`, `@`, tab or CR is prefixed with `'` so a spreadsheet never runs it.

## Connector trait (for future ERP adapters)

`procure_server::integrations::Connector`:

```rust
pub trait Connector: Send + Sync {
    fn name(&self) -> &'static str;
    /// Reads the records of `profile.kind`. Must not write anything anywhere.
    fn pull<'a>(&'a self, req: &'a PullRequest, profile: &'a Profile)
        -> BoxFuture<'a, Result<Pulled, ConnectorError>>;
    /// Delivers an export; returns what was delivered.
    fn push<'a>(&'a self, export: &'a Export) -> BoxFuture<'a, Result<Vec<u8>, ConnectorError>>;
}
```

Only `FileConnector` exists (CSV / TSV / xlsx in; CSV out). An adapter for an ERP's API
(e.g. SAP or Oracle) would:

- implement `pull` by fetching documents and mapping them to the same `Record` types (its own
  field mapping lives in the adapter; the domain parsing rules — integer money, exact dates, row
  errors — still apply);
- hand the result to the **same** staging (`import_batches` / `import_rows`), agent cross-check
  and person-approved apply — never write orders, receipts or invoices directly;
- keep credentials out of the database rows and logs, be test-mode unless configured and
  approved by the founder, and treat everything pulled as untrusted input;
- implement `push` for exports (agent results, orders) if the ERP accepts them.

## Limits and safety

- Files: `.csv` / `.tsv` / `.txt` (UTF-8 or Shift_JIS) or `.xlsx`, at most 10 MB; PDFs, images,
  executables and non-zip `.xlsx` are refused by content, not only by name. Workbooks go through
  the zip-bomb / sheet-size checks of `intake::table_rows`.
- At most 5,000 data rows, 500 records and 200 lines per record per file.
- Tables are tenant-scoped with RLS (`ENABLE` + `FORCE`, `org_id = app_org_id()`); the runtime
  role may only change a batch's status and decision columns, never what was uploaded.
