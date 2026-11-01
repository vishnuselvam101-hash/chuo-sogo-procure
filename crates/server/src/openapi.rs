//! OpenAPI document generated from the handlers' annotations.
//! `procure-server openapi > api/openapi.json`; a test fails if the committed file is stale,
//! and the desktop client's TypeScript types are generated from that file.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::api::{approvals, attachments, auth, orgs, products, requests, rfqs, suppliers};
use crate::error::{ErrorBody, ErrorInner};

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, doc: &mut utoipa::openapi::OpenApi) {
        let components = doc.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some("Opaque session token from /api/v1/auth/login"))
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Chuo-Sogo Procure API", description = "Backend API for the Chuo-Sogo Procure desktop client."),
    paths(
        auth::login,
        auth::logout,
        auth::me,
        auth::respond_invitation,
        auth::request_password_reset,
        auth::set_password,
        orgs::list_members,
        orgs::add_member,
        orgs::update_member,
        orgs::assign_operator,
        orgs::revoke_operator,
        orgs::list_audit,
        orgs::create_organisation,
        orgs::list_organisations,
        requests::create_request,
        requests::list_requests,
        requests::get_request,
        requests::update_request,
        requests::add_item,
        requests::replace_item,
        requests::delete_item,
        requests::submit,
        requests::cancel,
        requests::pick_up,
        requests::request_clarification,
        requests::verify_item,
        requests::start_sourcing,
        suppliers::create_supplier,
        suppliers::list_suppliers,
        suppliers::set_supplier_status,
        suppliers::list_org_suppliers,
        suppliers::propose_supplier,
        suppliers::decide_supplier,
        rfqs::create_rfq,
        rfqs::dispatch_rfq,
        rfqs::rfq_outbox,
        rfqs::get_rfq,
        rfqs::record_quotation,
        rfqs::run_comparison,
        rfqs::get_comparison,
        approvals::list_policies,
        approvals::put_policy,
        approvals::recommend,
        approvals::send_recommendation,
        approvals::get_recommendation,
        approvals::decide,
        approvals::withdraw_recommendation,
        products::create_product,
        products::list_products,
        products::get_product,
        products::get_org_product,
        products::add_evidence,
        products::set_product_status,
        attachments::upload,
        attachments::list,
        attachments::download,
        attachments::remove,
    ),
    components(schemas(ErrorBody, ErrorInner)),
    modifiers(&BearerAuth)
)]
pub struct ApiDoc;

pub fn json() -> Result<String, serde_json::Error> {
    ApiDoc::openapi().to_pretty_json().map(|mut s| {
        s.push('\n');
        s
    })
}
