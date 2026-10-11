//! `get_integration_contract`: the machine-readable contract the API serves.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{sign_request, Client, Error, SignRequest, INTEGRATION_CONTRACT_PATH};
use serde_json::Value;
use support::{MockServer, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(format!("{}/api", server.base_url()))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

/// The wire fixture is a projection of the production contract, so it is a
/// real answer with fewer sections.
fn production_projection() -> Value {
    serde_json::from_str(include_str!("merchant-api-wire-contract.json")).expect("valid JSON")
}

#[test]
fn the_contract_is_a_get_that_reads_the_version_and_the_vocabularies() {
    let projection = production_projection();
    let server = MockServer::start(vec![Reply::enveloped(&projection.to_string())]);

    let contract = client_for(&server)
        .get_integration_contract()
        .expect("the contract");

    assert_eq!(contract.version, projection["version"]);
    assert_eq!(contract.version, "2026-09-16");
    assert_eq!(
        serde_json::to_value(&contract.statuses).expect("serializes"),
        projection["statuses"]
    );
    assert_eq!(
        serde_json::to_value(&contract.webhook_event_catalog).expect("serializes"),
        projection["webhookEventCatalog"]
    );
    // Every section the struct does not model is in raw, as sent.
    assert_eq!(contract.raw, projection);

    let recorded = server.only_request();
    assert_eq!(recorded.method, "GET");
    assert_eq!(recorded.path, "/api/merchant-api/integration/contract");
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_eq!(recorded.body, "");
    let expected = sign_request(SignRequest {
        secret: SECRET,
        timestamp: recorded.header("X-Timestamp").expect("X-Timestamp sent"),
        method: "GET",
        path: INTEGRATION_CONTRACT_PATH,
        idempotency_key: "",
        body: "",
    });
    assert_eq!(recorded.header("X-Signature"), Some(expected.as_str()));
}

#[test]
fn a_contract_with_only_a_version_still_parses() {
    let server = MockServer::start(vec![Reply::enveloped(r#"{"version":"2027-01-01"}"#)]);
    let contract = client_for(&server)
        .get_integration_contract()
        .expect("the contract");
    assert_eq!(contract.version, "2027-01-01");
    assert!(contract.statuses.is_empty());
    assert!(contract.webhook_event_catalog.is_empty());
}

#[test]
fn a_contract_of_the_wrong_shape_is_an_api_error() {
    let server = MockServer::start(vec![Reply::enveloped(r#"{"version":7}"#)]);
    let error = client_for(&server)
        .get_integration_contract()
        .expect_err("not a contract");
    assert!(matches!(error, Error::Api { status: 200, .. }), "{error:?}");
}
