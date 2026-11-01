# Quality gate — Phase E (consumer shop), 2026-11-01

## Scope
- **Chuo-Sogo Shop** app (Tauri, `tauri.shop.conf.json`, Vite `--mode shop`; phone layout):
  public seller notice, sign-up with an emailed code, product list and pages (consumer price,
  tax included), cart (kept per account on the device), checkout with a Japanese address,
  orders with status timeline, bank-transfer instructions, invoice / credit note PDFs,
  account page (my data, close account).
- **Server** (`api/shop.rs`, migration `0012`): personal `consumer` organisation per person
  (existing tenant isolation applies), role `consumer` (`shop`), staff permission
  `manage_shop`; orders priced by the server (refused if the expected total differs);
  confirmation issues a **tax-inclusive qualified invoice** (`totals_inclusive`, rounded once
  per rate) in the same transaction and reserves stock; payment, shipping (carrier +
  tracking), completion; cancellation returns stock and issues a credit note for invoiced
  orders; notifications in the customer's language (test-mode outbox). Shop rules and the
  特定商取引法 notice; the shop cannot open until the notice and policies are complete (API
  and database). Order lifecycle enforced by a database trigger.
- **Admin console**: Shop section (order queue by state, actions, rules and notice).

## Automated
| Suite | Count | Result |
|---|---|---|
| Domain: inclusive tax, order lifecycle, shipping rule | 3 + 2 | pass |
| Server shop integration (sign-up, orders, invoice/credit note, stock, privacy, isolation, DB guard) | 4 | pass |
| Desktop UI (Vitest) | 43 | pass |
| Shop E2E (release binary): seller notice → sign-up with emailed code → product → cart (¥2,200 + ¥880) → order → staff confirm → transfer instructions → invoice PDF saved → paid → shipped with tracking | 12 checks | **12/12 pass** |
| Catalogue / admin / workflow E2E (regression) | 13 / 20 / 30 | pass |

## Review
Fixed with regression tests: SF-055 (consumers saw business prices and supplier names),
SF-056 (no stock reservation → oversell), SF-057 (sign-up timing, address blocking, global
limit, platform scope), SF-058 (mail language).

## Not done / decisions
Card / convenience-store payment (D7, a paid provider), public hosting for consumers (D8),
store distribution (D9). The legal texts must be written/approved by Chuo-Sogo.
