-- Phase D: AI help for supplier listings. Suggestions for listings live with the others in
-- ai_suggestions (the supplier organisation is the tenant); where the text came from is
-- recorded. Raw audio is never stored, only its transcript (the suggestion's input text).

ALTER TABLE ai_suggestions DROP CONSTRAINT ai_suggestions_capability_check;
ALTER TABLE ai_suggestions ADD CONSTRAINT ai_suggestions_capability_check
    CHECK (capability IN ('extract_items', 'extract_quotation', 'extract_listing', 'translate_listing'));

ALTER TABLE ai_suggestions DROP CONSTRAINT ai_suggestions_subject_type_check;
ALTER TABLE ai_suggestions ADD CONSTRAINT ai_suggestions_subject_type_check
    CHECK (subject_type IN ('procurement_request', 'rfq', 'listing', 'supplier_org'));

-- typed text, a transcribed voice note, or text read from a file (PDF, spreadsheet, CSV).
ALTER TABLE ai_suggestions ADD COLUMN input_source text NOT NULL DEFAULT 'typed'
    CHECK (input_source IN ('typed', 'voice', 'file'));
