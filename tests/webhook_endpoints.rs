//! Webhook endpoints against the loopback mock: `create_webhook_endpoint` and
//! `list_webhook_endpoints` on `/merchant-api/webhooks`, and
//! `delete_webhook_endpoint` on `/merchant-api/webhooks/{id}`. No network, no
//! credentials.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    event_type, sign_request, webhook_endpoint_status, Client, Error, IdempotencyKey, SignRequest,
    WebhookEndpointRequest, WEBHOOKS_PATH,
};
use serde_json::json;
use support::{MockServer, Recorded, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const WEBHOOK_ID: &str = "5d0c6a52-8f1e-4b7a-9c3d-2e1f0a9b8c7d";
const WEBHOOK_KEY: &str = "webhook-orders-2026-10";
const SIGNING_SECRET_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const URL: &str = "https://shop.example/webhooks/dominaite";

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(format!("{}/api", server.base_url()))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

fn key() -> IdempotencyKey {
    IdempotencyKey::new(WEBHOOK_KEY).expect("a valid idempotency key")
}

fn request() -> WebhookEndpointRequest {
    WebhookEndpointRequest::new(
        URL,
        [event_type::PAYMENT_SUCCEEDED, event_type::CHARGE_FAILED],
        key(),
    )
}

/// The endpoint as the API answers it, with the secret on the first 201 only.
fn endpoint(with_secret: bool) -> String {
    let secret = if with_secret {
        format!(r#","secret":"whsec_{SIGNING_SECRET_HEX}""#)
    } else {
        String::new()
    };
    format!(
        r#"{{"id":"{WEBHOOK_ID}","url":"{URL}","events":["payment.succeeded","charge.failed"],"status":"active","createdAt":"2026-10-11T09:00:00Z"{secret}}}"#
    )
}

fn enveloped(status: u16, data: &str) -> Reply {
    Reply::Json(
        status,
        format!(r#"{{"success":true,"data":{data},"metadata":{{"requestId":"r1"}}}}"#),
    )
}

fn assert_signature_matches(recorded: &Recorded, expected_path: &str, idempotency_key: &str) {
    let expected = sign_request(SignRequest {
        secret: SECRET,
        timestamp: recorded.header("X-Timestamp").expect("X-Timestamp sent"),
        method: &recorded.method,
        path: expected_path,
        idempotency_key,
        body: &recorded.body,
    });
    assert_eq!(
        recorded.header("X-Signature"),
        Some(expected.as_str()),
        "the signature does not cover what was sent"
    );
}

#[test]
fn create_sends_url_and_events_signs_the_key_and_returns_the_secret() {
    let server = MockServer::start(vec![enveloped(201, &endpoint(true))]);

    let created = client_for(&server)
        .create_webhook_endpoint(&request().extra("futureField", json!(1)))
        .expect("an endpoint");

    assert_eq!(created.id, WEBHOOK_ID);
    assert_eq!(created.url, URL);
    assert_eq!(created.events, ["payment.succeeded", "charge.failed"]);
    assert_eq!(created.status, webhook_endpoint_status::ACTIVE);
    assert_eq!(created.created_at, "2026-10-11T09:00:00Z");
    assert_eq!(
        created.secret.as_deref(),
        Some(format!("whsec_{SIGNING_SECRET_HEX}").as_str())
    );

    let recorded = server.only_request();
    assert_eq!(recorded.method, "POST");
    assert_eq!(recorded.path, format!("/api{WEBHOOKS_PATH}"));
    assert_eq!(
        recorded.body,
        r#"{"url":"https://shop.example/webhooks/dominaite","events":["payment.succeeded","charge.failed"],"futureField":1}"#
    );
    assert_eq!(recorded.header("Idempotency-Key"), Some(WEBHOOK_KEY));
    assert_signature_matches(&recorded, WEBHOOKS_PATH, WEBHOOK_KEY);
}

/// A replay of the same key answers the same endpoint without the secret.
#[test]
fn a_replay_has_no_secret() {
    let server = MockServer::start(vec![enveloped(201, &endpoint(false))]);
    let replayed = client_for(&server)
        .create_webhook_endpoint(&request())
        .expect("the same endpoint");
    assert_eq!(replayed.id, WEBHOOK_ID);
    assert_eq!(replayed.secret, None);
}

#[test]
fn debug_output_never_shows_the_signing_secret() {
    let server = MockServer::start(vec![enveloped(201, &endpoint(true))]);
    let created = client_for(&server)
        .create_webhook_endpoint(&request())
        .expect("an endpoint");
    assert_eq!(
        created.raw["secret"],
        format!("whsec_{SIGNING_SECRET_HEX}"),
        "raw keeps the answer as sent"
    );

    for printed in [format!("{created:?}"), format!("{created:#?}")] {
        assert!(!printed.contains(SIGNING_SECRET_HEX), "{printed}");
        assert!(printed.contains("whsec_***redacted***"), "{printed}");
        assert!(printed.contains(WEBHOOK_ID), "{printed}");
    }
}

#[test]
fn create_refuses_an_empty_url_or_no_events_before_sending() {
    let server = MockServer::start(vec![enveloped(201, &endpoint(true))]);
    let no_events: [&str; 0] = [];
    for request in [
        WebhookEndpointRequest::new(" ", [event_type::PAYMENT_SUCCEEDED], key()),
        WebhookEndpointRequest::new(URL, no_events, key()),
    ] {
        let error = client_for(&server)
            .create_webhook_endpoint(&request)
            .expect_err("refused");
        assert!(matches!(error, Error::Validation { .. }), "{error:?}");
    }
    assert!(server.requests().is_empty());
}

/// The id of the endpoint this key registered sits in the error envelope,
/// which `Error::Api` keeps in `raw`.
#[test]
fn already_registered_keeps_the_existing_endpoint_id() {
    let server = MockServer::start(vec![Reply::Json(
        409,
        format!(
            r#"{{"success":false,"data":null,"error":{{"code":"WEBHOOK_ALREADY_REGISTERED","message":"This Idempotency-Key already registered an endpoint.","params":{{"webhookId":"{WEBHOOK_ID}"}}}}}}"#
        ),
    )]);
    let error = client_for(&server)
        .create_webhook_endpoint(&request())
        .expect_err("already registered");
    assert_eq!(error.code(), Some("WEBHOOK_ALREADY_REGISTERED"));
    let Error::Api { status, raw, .. } = &error else {
        panic!("expected Error::Api, got {error:?}");
    };
    assert_eq!(*status, 409);
    assert_eq!(raw["error"]["params"]["webhookId"], WEBHOOK_ID);
}

#[test]
fn a_key_without_the_webhooks_scope_is_forbidden() {
    let server = MockServer::start(vec![Reply::error_envelope(
        403,
        "INSUFFICIENT_SCOPE",
        "This API key does not have the webhooks:manage scope.",
    )]);
    let error = client_for(&server)
        .list_webhook_endpoints()
        .expect_err("forbidden");
    assert!(matches!(error, Error::Forbidden { .. }), "{error:?}");
    assert_eq!(error.code(), Some("INSUFFICIENT_SCOPE"));
}

#[test]
fn list_reads_every_endpoint_and_signs_like_a_get() {
    let inactive = r#"{"id":"0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d","url":"https://old.example/hook","events":["payment.refunded"],"status":"inactive","createdAt":"2026-09-01T09:00:00Z"}"#;
    let server = MockServer::start(vec![enveloped(
        200,
        &format!("[{},{inactive}]", endpoint(false)),
    )]);

    let endpoints = client_for(&server)
        .list_webhook_endpoints()
        .expect("the endpoints");

    assert_eq!(endpoints.len(), 2);
    assert_eq!(endpoints[0].id, WEBHOOK_ID);
    assert_eq!(endpoints[1].status, webhook_endpoint_status::INACTIVE);
    assert_eq!(endpoints[1].events, [event_type::PAYMENT_REFUNDED]);
    assert!(endpoints.iter().all(|endpoint| endpoint.secret.is_none()));
    assert_eq!(endpoints[1].raw["url"], "https://old.example/hook");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "GET");
    assert_eq!(recorded.path, format!("/api{WEBHOOKS_PATH}"));
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_eq!(recorded.body, "");
    assert_signature_matches(&recorded, WEBHOOKS_PATH, "");
}

#[test]
fn an_empty_list_is_an_empty_vec() {
    let server = MockServer::start(vec![enveloped(200, "[]")]);
    let endpoints = client_for(&server)
        .list_webhook_endpoints()
        .expect("the endpoints");
    assert!(endpoints.is_empty());
}

/// The API signs the id in lowercase hyphenated form, so an id in capitals
/// goes out in lowercase.
#[test]
fn delete_sends_the_id_in_lowercase_and_resolves_on_a_204() {
    let server = MockServer::start(vec![Reply::Json(204, String::new())]);
    client_for(&server)
        .delete_webhook_endpoint(&WEBHOOK_ID.to_uppercase())
        .expect("deleted");

    let expected_path = format!("{WEBHOOKS_PATH}/{WEBHOOK_ID}");
    let recorded = server.only_request();
    assert_eq!(recorded.method, "DELETE");
    assert_eq!(recorded.path, format!("/api{expected_path}"));
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_eq!(recorded.body, "");
    assert_signature_matches(&recorded, &expected_path, "");
}

#[test]
fn deleting_an_unknown_endpoint_is_webhook_not_found() {
    let server = MockServer::start(vec![Reply::error_envelope(
        404,
        "WEBHOOK_NOT_FOUND",
        "No webhook endpoint with this id under your account.",
    )]);
    let error = client_for(&server)
        .delete_webhook_endpoint(WEBHOOK_ID)
        .expect_err("not found");
    assert!(matches!(error, Error::Api { status: 404, .. }), "{error:?}");
    assert_eq!(error.code(), Some("WEBHOOK_NOT_FOUND"));
}

#[test]
fn a_webhook_id_that_is_not_a_uuid_is_refused_before_sending() {
    let server = MockServer::start(vec![Reply::Json(204, String::new())]);
    let client = client_for(&server);
    for id in ["", "wh_123", "5d0c6a52-8f1e-4b7a-9c3d", "../5d0c6a52"] {
        let error = client.delete_webhook_endpoint(id).expect_err("refused");
        assert!(
            matches!(error, Error::Validation { .. }),
            "{id:?}: {error:?}"
        );
    }
    assert!(server.requests().is_empty());
}
