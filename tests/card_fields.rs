//! Card fields on a checkout session: the `integration` request field, and the
//! echoed `integration` and `clientSecret` on the response, pinned against the
//! canonical fixture.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use serde_json::Value;

use dominaite::{
    sign_request, CheckoutSessionRequest, Client, Error, IdempotencyKey, Integration, SignRequest,
    SESSIONS_PATH,
};
use support::{MockServer, Reply};

const FIXTURE: &str = include_str!("merchant-api-contract.json");

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const KEY: &str = "checkout-order-1042-2500-EUR";

fn create_endpoint() -> Value {
    let contract: Value = serde_json::from_str(FIXTURE).expect("the fixture is valid JSON");
    contract["endpoints"]["createCheckoutSession"].clone()
}

fn example(name: &str) -> Reply {
    Reply::enveloped(&create_endpoint()[name].to_string())
}

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(server.base_url())
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

fn request() -> CheckoutSessionRequest {
    CheckoutSessionRequest::new(
        2500,
        "EUR",
        "order-1042",
        IdempotencyKey::new(KEY).expect("a valid idempotency key"),
    )
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("a JSON array")
        .iter()
        .map(|item| item.as_str().expect("a JSON string").to_string())
        .collect()
}

fn sorted_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("a JSON object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// Asking for card fields must reach the gateway in the signed body, or the
/// merchant gets a widget session and a page that cannot mount card fields.
#[test]
fn fields_is_sent_in_the_signed_body() {
    let server = MockServer::start(vec![example("fieldsSuccessExample")]);
    client_for(&server)
        .create_checkout_session(&request().integration(Integration::Fields))
        .expect("session created");

    let recorded = server.only_request();
    assert_eq!(
        recorded.body,
        r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042","integration":"fields"}"#
    );
    let expected = sign_request(SignRequest {
        secret: SECRET,
        timestamp: recorded.header("X-Timestamp").expect("X-Timestamp sent"),
        method: &recorded.method,
        path: SESSIONS_PATH,
        idempotency_key: KEY,
        body: &recorded.body,
    });
    assert_eq!(recorded.header("X-Signature"), Some(expected.as_str()));
}

#[test]
fn widget_is_sent_when_set_explicitly() {
    let server = MockServer::start(vec![example("successExample")]);
    client_for(&server)
        .create_checkout_session(&request().integration(Integration::Widget))
        .expect("session created");

    let body: Value = serde_json::from_str(&server.only_request().body).expect("JSON body");
    assert_eq!(body["integration"], "widget");
}

/// An unset integration is omitted, not sent as null, so the signed bytes of
/// every existing widget integration do not move.
#[test]
fn an_unset_integration_is_omitted_from_the_body() {
    let server = MockServer::start(vec![example("successExample")]);
    client_for(&server)
        .create_checkout_session(&request())
        .expect("session created");

    assert_eq!(
        server.only_request().body,
        r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042"}"#
    );
}

#[test]
fn the_integration_vocabulary_is_exactly_the_contracts() {
    let contract: Value = serde_json::from_str(FIXTURE).expect("the fixture is valid JSON");
    let ours: Vec<String> = Integration::ALL
        .iter()
        .map(|value| value.as_str().to_string())
        .collect();
    assert_eq!(ours, strings(&contract["integrationVocabulary"]));

    for value in Integration::ALL {
        assert_eq!(
            serde_json::to_value(value).expect("serializes"),
            Value::String(value.as_str().to_string()),
            "{value:?} serializes to its wire value"
        );
    }
}

/// The fields session is what the drop-in is mounted with: every one of these
/// values goes to the payer's page, so a dropped one is a dead checkout.
#[test]
fn the_fields_example_comes_back_with_integration_and_client_secret() {
    let server = MockServer::start(vec![example("fieldsSuccessExample")]);
    let session = client_for(&server)
        .create_checkout_session(&request().integration(Integration::Fields))
        .expect("the contract's fields example is a session");

    assert_eq!(session.integration, Integration::Fields.as_str());
    assert_eq!(
        session.client_secret.as_deref(),
        Some("cs_4f3e2d1c0b9a8f7e6d5c4b3a2f1e0d9c")
    );
    assert_eq!(
        session.transaction_id,
        "7a6b5c4d-3e2f-4a1b-9c8d-7e6f5a4b3c2d"
    );
    assert_eq!(session.cashier_key, "ck_live_blox_8c7d6e5f4a3b2c1d");
    assert_eq!(session.cashier_token, "ctok_blox_0a1b2c3d4e5f6a7b");
}

#[test]
fn the_widget_example_echoes_widget_without_a_secret() {
    let server = MockServer::start(vec![example("successExample")]);
    let session = client_for(&server)
        .create_checkout_session(&request())
        .expect("the contract's success example is a session");

    assert_eq!(session.integration, Integration::Widget.as_str());
    assert_eq!(session.client_secret, None);
}

/// The fields example carries every checkout field; the widget example carries
/// all of them except clientSecret, which exists only for fields.
#[test]
fn the_examples_carry_the_checkout_fields() {
    let create = create_endpoint();
    let mut fields = strings(&create["checkoutFields"]);
    fields.sort();
    assert_eq!(
        sorted_keys(&create["fieldsSuccessExample"]["checkout"]),
        fields
    );

    let widget: Vec<String> = fields
        .into_iter()
        .filter(|name| name != "clientSecret")
        .collect();
    assert_eq!(sorted_keys(&create["successExample"]["checkout"]), widget);
}

/// Card fields on an account without them is a 400 INVALID_SELECTION. It has
/// to arrive as a machine-readable code, not as a refusal: nothing was created.
#[test]
fn fields_not_enabled_is_an_api_error_with_its_code() {
    let server = MockServer::start(vec![Reply::error_envelope(
        400,
        "INVALID_SELECTION",
        "Card fields are not enabled for this account. Ask Dominaite support to enable them.",
    )]);

    let error = client_for(&server)
        .create_checkout_session(&request().integration(Integration::Fields))
        .expect_err("a rejected request is not a session");

    assert_eq!(error.code(), Some("INVALID_SELECTION"));
    assert_eq!(error.http_status(), Some(400));
    assert!(matches!(error, Error::Api { .. }), "got {error:?}");
}
