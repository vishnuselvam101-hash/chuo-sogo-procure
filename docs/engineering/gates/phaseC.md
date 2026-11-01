# Quality gate — Phase C (supplier workspace and catalogue), 2026-11-01

## Scope
- **Supplier organisations** (third organisation kind) with roles `supplier_admin` and
  `supplier_staff`; created from the admin console (type: customer / supplier). Supplier
  members are external accounts and get no procurement, finance or catalogue permissions.
- **Listings** (`procure_domain::listing`): manufacturer, model number (normalised key for
  duplicate checks), serial number only for single units (stock ≤ 1), condition, Japanese and
  English title/description, specifications, business price (tax excl.) and consumer price
  (tax incl.), tax rate, stock. Workflow draft → submitted → approved / rejected, retire;
  any supplier change takes a published listing back to draft.
- **Photos and documents**: 2–5 photos (JPEG/PNG/WebP, ≥ 1200 px long side, ≤ 50 MP, ≤ 4:1),
  up to 5 PDFs; content-sniffed; photo metadata removed before storage (`media.rs`: JPEG
  APPn/COM except ICC and Adobe, data after EOI; PNG allowlist with CRC check; WebP EXIF/XMP).
- **Moderation** in the admin console (`moderate_listings`): queue by state, photos, possible
  duplicates (same manufacturer + model), approve / reject with note / retire.
- **Catalogue** in the business app (`view_catalogue`, customer roles): search by name, maker
  or model number (loose match), product page with photos and specifications. Suspended
  suppliers disappear from the catalogue.
- **Native** `api_image`: listing photos reach the webview only as checked `data:` URLs.
- Migration `0010`: RLS (owner / platform scope / published), workflow trigger (suppliers
  cannot publish or review, moderators cannot change content), supplier name visible only
  for suppliers with published listings.

## Automated
`scripts/gate.sh full` — all 17 steps **PASS**.

| Suite | Count | Result |
|---|---|---|
| Domain listing + RBAC | 5 + 11 | pass |
| Media metadata removal (JPEG, PNG, WebP) | 3 | pass |
| Server listings integration (real PostgreSQL; workflow trigger tested directly) | 4 | pass |
| Native layer (path/type checks for images) | 14 | pass |
| Desktop UI (Vitest) | 37 | pass |
| Catalogue E2E across both apps: supplier lists with 2 real photos → admin approves (photos shown) → customer finds by model number | 9 checks | **9/9 pass** |
| Admin E2E / business workflow E2E (regression) | 20 / 30 checks | **pass** |

## Review
Local independent review: no tenant-isolation leaks. Fixed with regression tests:
SF-047 (photo slots exhausted after delete/add cycles → permanent 500), SF-048 (suspended
supplier stayed in the catalogue), SF-049 (JPEG data after end-of-image kept; Adobe colour
segment dropped; PNG private chunks kept, CRC unchecked; no pixel limit), SF-050 (workflow
enforced only in the API → database trigger).

## Not done
- AI help for listings (voice, photos, catalogues) is Phase D.
- No ordering from the catalogue yet (consumer/business checkout is Phase E); customers can
  reference a listing when they raise a procurement request.
- Photo order can only be changed by removing and re-adding (first photo = cover).
