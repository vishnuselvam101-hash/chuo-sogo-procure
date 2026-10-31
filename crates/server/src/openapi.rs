//! OpenAPI document generated from the handlers' annotations.
//! `procure-server openapi > api/openapi.json`; a test fails if the committed file is stale,
//! and the desktop client's TypeScript types are generated from that file.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::api::{auth, orgs};
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
