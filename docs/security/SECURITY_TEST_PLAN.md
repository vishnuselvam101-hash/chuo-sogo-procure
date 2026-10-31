# Security test plan

Critical scenarios (must pass at every milestone from the phase that introduces them):
1. Customer A cannot read/list/modify Customer B requests, quotes, files, POs, invoices, audit (API + direct repo + RLS-only checks).
2. Requester without `approve` permission cannot approve; nobody can self-approve unless org policy explicitly allows; limits enforced per step.
3. AI output naming a different manufacturer/model never changes a verified identity; alternatives need a customer approval record.
4. Supplier not `verified` + `customer_approved` is never shown as approved.
5. Quotation missing required fields → `incomplete`, never `compliant`.
6. No code path lets AI or the job worker create a PO record; PO requires a satisfied approval.
7. Invoice price/qty/tax/shipping mismatches create discrepancies; reconcile blocked while open.
8. Every state transition writes exactly one audit event in the same transaction.
9. AI timeout/malformed output leaves records unchanged (transaction rolled back, suggestion marked failed).
10. Two orgs' jobs/AI calls never share context (assert prompt contents).

Plus: OWASP ASVS 5.0.0 Level 2 checklist per release; ZAP baseline + API scan against local server only; Tauri capability review; gitleaks on built binaries.
