//! Contract drift guard: the committed api/openapi.json must match the code.

#[test]
fn committed_openapi_document_is_current() {
    let generated = procure_server::openapi::json().unwrap();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../api/openapi.json");
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        generated == committed,
        "api/openapi.json is stale: run `cargo run -p procure-server -- openapi > api/openapi.json` \
         and `pnpm -C apps/desktop gen:api`"
    );
}

#[test]
fn every_routed_endpoint_is_documented() {
    let doc = procure_server::openapi::json().unwrap();
    for path in [
        "/api/v1/auth/login",
        "/api/v1/auth/logout",
        "/api/v1/auth/password-reset",
        "/api/v1/auth/set-password",
        "/api/v1/me",
        "/api/v1/invitations/{org_id}/{answer}",
        "/api/v1/platform/organisations",
        "/api/v1/orgs/{org_id}/members",
        "/api/v1/orgs/{org_id}/members/{user_id}",
        "/api/v1/orgs/{org_id}/assignments",
        "/api/v1/orgs/{org_id}/assignments/{user_id}",
        "/api/v1/orgs/{org_id}/audit",
    ] {
        assert!(
            doc.contains(&format!("\"{path}\"")),
            "{path} missing from OpenAPI"
        );
    }
}
