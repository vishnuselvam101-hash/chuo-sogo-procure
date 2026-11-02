# Factory-side model (CEO direction, 2026-11-02)

## What the CEO said

- We stand on the **factory's side** and do the factory's buying work with AI. We are not a
  place that connects sellers and buyers. Closest to Model B (Procurement-as-a-Service), but
  paid differently.
- Customer: a factory of a manufacturer with sales of ¥10 billion or more. Start with **one
  factory**.
- First scope: **consumables, tools, electrical materials**: bought by quotation every time,
  not from a catalogue.
- Flow:
  1. The factory says what it wants to buy.
  2. The AI asks about 10 suppliers for quotes.
  3. The AI reads the replies (email, PDF, Excel), compares them and shows the best one.
  4. When the factory approves, the AI arranges the order and checks the delivery date.
  - A person on our side checks **before the AI sends a request** and **after it compares**.
- The factory decides the supplier and the price and **pays the supplier directly**. We are
  never the seller: no stock, no catalogue of our own.
- **No monthly fee.** We receive **half of the amount the price went down** from the price
  paid before, for **36 months**. No price drop → **¥0**.

## How the product now does this

| CEO step | In the product | Where |
|---|---|---|
| 1. Factory says what it wants | Request with lines; AI reads free text/files into lines (Phase 3) | `api/requests.rs`, `api/ai.rs` |
| Check before sending | **RFQ check step**: an assigned staff member marks the RFQ checked (recipients, items, no factory name in the message); dispatch refuses without it (`rfq_not_checked`) | `api/rfqs.rs::check_rfq`, migration 0017 |
| 2. Ask ~10 suppliers | One RFQ to up to 20 verified suppliers the factory has not rejected; messages recorded in the test outbox (no real email until approved) | `api/rfqs.rs` |
| 3. Read replies (email, PDF, Excel) | **Reply files**: a saved email (`.eml`, ISO-2022-JP / Shift_JIS / UTF-8, attachments read too), PDF (text layer, else OCR), Excel/CSV, FAX scan (OCR) → text → AI extraction → staff confirm and save the quotation | `intake.rs::quote_reply_text`, `POST …/rfqs/{id}/reply-text` |
| Compare, show the best | Deterministic comparison (compliance, total, lead time) + price vs the factory's last price + decision memo from its buying criteria | `domain/comparison.rs`, `domain/decision.rs` |
| Check after comparing | **Comparison check step**: staff mark the recommendation checked (prices/quantities/lead times against the replies); sending to the factory refuses without it (`recommendation_not_checked`) | `api/approvals.rs::check_recommendation` |
| 4. Factory approves | Factory approvers only; AI and our staff can never approve | `domain/workflow.rs` |
| Arrange the order | Order recorded from the approved quotation with the factory's own PO number; the factory pays the supplier | `api/orders.rs` |
| Check the delivery date | **Promised date** from the quoted lead time; staff record the **supplier's confirmed date**; an automatic `check_delivery` task is due that day, closes when everything arrived, reopens if a receipt is reversed; orders past the date show **Late** | migration 0018, `api/orders.rs` |
| Not a marketplace | Customer catalogue hidden unless a build sets `VITE_MARKETPLACE=on`; shop and supplier listings stay in the code, unused | `apps/desktop/src/lib/nav.ts` |
| Paid from savings | **Savings share** (below) | migration 0016, `domain/savings.rs`, `api/savings.rs` |

## Savings share (how we are paid)

- **Terms** per factory (admin console → organisation → *Savings share*): share (default
  50 %), term (default 36 months), start date. The end date is computed. Terms are fixed
  once any saving was recorded.
- **Price paid before (baseline)**, fixed once per item (alias groups count as one item):
  - from the factory's own purchase records (spreadsheet/PDF/OCR imports), the **last
    purchase dated before the start date**. Records of orders placed through us never
    count, so our first order does not reset the baseline.
  - or, for an item with no record, a price our staff enter with its evidence.
  - A baseline counts automatically only when it comes from a record **the factory's own
    users imported** and that **states its unit**. A record our staff imported for them, a
    record without a unit, and an agreed price all wait until a **factory approver confirms**
    them; lines reconciled meanwhile are priced at the moment of confirmation.
  - Once confirmed or used, a baseline cannot be edited or deleted (database trigger).
- **Terms** become fixed only once a line was counted under them (lines recorded before any
  terms existed stay at ¥0 and do not block saving the terms).
- **When it counts:** when the order **reconciles** (goods received and the supplier's
  invoice matched), so only prices really paid count.
- **Per order line:** saving = (baseline − price) × quantity, floored at 0, rounded down to
  the yen; fee = saving × share, rounded down (always in the factory's favour). Each line is
  recorded with its reason: `counted`, `no_saving`, `no_baseline`,
  `unconfirmed_baseline`, `mismatch` (unit/currency), `outside_term`, `no_agreement`. Rows
  are immutable in the database.
- **Billing:** admin creates a draft Chuo-Sogo invoice "through" a date: one line per order,
  10 % consumption tax on the fee. Deleting the draft releases the lines. The factory sees a
  fee as unbilled until the invoice is issued.
- **Existing data:** RFQs and recommendations sent before the check step existed are
  recorded as checked by the person who sent them (migration 0017), so open approvals on
  the live and demo servers keep working. Issuing, PDF and
  credit notes are the existing Phase B invoicing.
- **The factory sees** every line, its baseline, our fee, and the totals (Savings page).

## Questions for the CEO (defaults in force until answered)

1. **Price paid before**: we fix it once per item, at the factory's last price before the
   contract starts, for the whole 36 months. (The alternative, "the previous order's price"
   each time, would make the fee ¥0 from the second order on.) Correct?
2. **36 months**: counted from the contract start for all items (default), or 36 months per
   item from its first saving?
3. **New items** with no earlier price: we agree a price with the factory and they confirm it
   (default). Or no fee for such items?
4. **What is compared**: unit price before tax; shipping is not included (default). Should a
   cheaper delivery charge or a bigger pack size count?
5. **When we invoice**: monthly, for orders whose goods arrived and whose supplier invoice
   matched (default). Or at the time of the order?
6. **Suppliers see**: today our quote requests do not name the factory (Chuo-Sogo's RFQ number
   only). Should suppliers be told which factory it is for?
7. **The order**: the factory issues its own PO number and pays the supplier (default). Do we
   also send the order to the supplier on the factory's behalf, by email?

## Not done yet (needs a founder decision)

- **Real email** to suppliers (currently test mode: messages are recorded and exported by
  hand). Needs a sending address/provider and the founder's approval (CLAUDE.md).
- **Reading replies straight from a mailbox** (IMAP): today staff save the email and upload
  it. Same decision as above.
- **AI supplier shortlist** of ~10 for each request: staff pick suppliers from the directory.
- The **AI model** for reading replies (D5 in RELEASE.md); the mock model is the default.
- Live server and demo still run the older build until `scripts/install-server.sh` and
  `scripts/demo/install-demo.sh` are run again (migrations 0016–0018 apply then).
