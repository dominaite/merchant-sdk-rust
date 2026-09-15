//! Stored payment methods against the loopback mock: `save_card` on a session,
//! `payment_method` on its status, then off-session charges and revocation on
//! `/merchant-api/payment-methods/{id}`. No network, no credentials.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    charge_status, decline_class, payment_method_status, sign_request, ChargeRequest,
    CheckoutSessionRequest, Client, Error, SignRequest, PAYMENT_METHODS_PATH, SESSIONS_PATH,
};
use support::{MockServer, Recorded, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TRANSACTION_ID: &str = "11111111-1111-4111-8111-111111111111";
const PAYMENT_METHOD_ID: &str = "pm_0123456789abcdef0123456789abcdef";

// The charge vector from tests/vectors.rs: with the vector timestamp the header
// the client sends is the vector signature.
const CHARGE_KEY: &str = "00000000-0000-4000-8000-000000000003";
const CHARGE_BODY: &str = r#"{"amount":2500,"currency":"EUR","orderReference":"order-1043"}"#;
const CHARGE_SIGNATURE: &str = "9ce9f54efa2533a46aa4493b97b56aeb657f41d6a18f1c008c7fd412029aebf9";
const REVOKE_SIGNATURE: &str = "9330100343c4b820504890a09829a193d5815ca39e92160fdfc13d320a802a02";

const CHECKOUT: &str = r#"{"transactionId":"11111111-1111-4111-8111-111111111111","orderId":"dom_42","cashierKey":"ck_live","cashierToken":"ct_live","amount":2500,"currency":"EUR","expiresAt":"2026-08-20T12:00:00Z"}"#;
const CHARGE: &str = r#"{"chargeId":"chg_1","status":"succeeded","declineClass":null,"declineCode":null,"transactionId":"33333333-3333-4333-8333-333333333333"}"#;

fn create_ok() -> Reply {
    Reply::enveloped(&format!(r#"{{"success":true,"checkout":{CHECKOUT}}}"#))
}

fn charge_ok() -> Reply {
    Reply::Json(201, CHARGE.to_string())
}

fn revoke_ok() -> Reply {
    Reply::Json(204, String::new())
}

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(format!("{}/api", server.base_url()))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

fn charge_request() -> ChargeRequest {
    ChargeRequest::new(2500, "EUR", "order-1043").idempotency_key(CHARGE_KEY)
}

fn charge_path() -> String {
    format!("{PAYMENT_METHODS_PATH}/{PAYMENT_METHOD_ID}/charges")
}

fn revoke_path() -> String {
    format!("{PAYMENT_METHODS_PATH}/{PAYMENT_METHOD_ID}")
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
fn save_card_is_sent_in_the_session_body_and_nowhere_else() {
    let server = MockServer::start(vec![create_ok()]);
    client_for(&server)
        .create_checkout_session(
            &CheckoutSessionRequest::new(2500, "EUR", "order-1042")
                .save_card(true)
                .idempotency_key("00000000-0000-4000-8000-000000000001"),
        )
        .expect("session created");

    let recorded = server.only_request();
    let body: serde_json::Value = serde_json::from_str(&recorded.body).expect("JSON body");
    assert_eq!(body["saveCard"], serde_json::Value::Bool(true));
    assert!(
        body.get("idempotencyKey").is_none(),
        "the key is a header, not a body field"
    );
    assert_signature_matches(
        &recorded,
        SESSIONS_PATH,
        "00000000-0000-4000-8000-000000000001",
    );
}

/// A session without `save_card` keeps the exact vector body: the flag is
/// omitted, not sent as false, so the signed bytes of every existing integration
/// do not move.
#[test]
fn a_session_without_save_card_keeps_the_vector_body() {
    let server = MockServer::start(vec![create_ok()]);
    client_for(&server)
        .create_checkout_session(&CheckoutSessionRequest::new(2500, "EUR", "order-1042"))
        .expect("session created");

    assert_eq!(
        server.only_request().body,
        r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042"}"#
    );
}

#[test]
fn get_status_carries_the_stored_payment_method() {
    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"transactionId":"{TRANSACTION_ID}","status":"succeeded","amount":2500,"currency":"EUR","paymentMethod":{{"id":"{PAYMENT_METHOD_ID}","brand":"visa","last4":"4242","expiryMonth":12,"expiryYear":2029,"status":"active"}}}}"#
    ))]);

    let status = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect("status read");

    let method = status.payment_method.expect("a stored payment method");
    assert_eq!(method.id, PAYMENT_METHOD_ID);
    assert_eq!(method.brand, "visa");
    assert_eq!(method.last4, "4242");
    assert_eq!((method.expiry_month, method.expiry_year), (12, 2029));
    assert_eq!(method.status, payment_method_status::ACTIVE);
    assert!(method.is_chargeable());
}

#[test]
fn get_status_without_a_saved_card_leaves_payment_method_none() {
    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"transactionId":"{TRANSACTION_ID}","status":"succeeded","amount":2500,"currency":"EUR","paymentMethod":null}}"#
    ))]);

    let status = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect("status read");

    assert_eq!(status.payment_method, None);
}

#[test]
fn a_revoked_or_unknown_payment_method_status_is_not_chargeable() {
    for value in ["revoked", "expired", "frozen"] {
        let server = MockServer::start(vec![Reply::enveloped(&format!(
            r#"{{"transactionId":"{TRANSACTION_ID}","status":"succeeded","amount":2500,"currency":"EUR","paymentMethod":{{"id":"{PAYMENT_METHOD_ID}","brand":"visa","last4":"4242","expiryMonth":12,"expiryYear":2029,"status":"{value}"}}}}"#
        ))]);
        let status = client_for(&server)
            .get_status(TRANSACTION_ID)
            .expect("status read");
        assert!(
            !status.payment_method.expect("present").is_chargeable(),
            "{value} must not read as chargeable"
        );
    }
}

#[test]
fn charge_payment_method_signs_the_charge_vector_byte_for_byte() {
    let server = MockServer::start(vec![charge_ok()]);
    let charge = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect("charged");

    assert_eq!(charge.charge_id, "chg_1");
    assert_eq!(charge.status, charge_status::SUCCEEDED);
    assert!(charge.is_paid());
    assert!(charge.is_terminal());
    assert_eq!(charge.decline_class, None);
    assert_eq!(charge.decline_code, None);
    assert_eq!(
        charge.transaction_id,
        "33333333-3333-4333-8333-333333333333"
    );
    assert_eq!(charge.raw["chargeId"], "chg_1");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "POST");
    // The signed path is the canonical path; the request goes to the /api prefix.
    assert_eq!(recorded.path, format!("/api{}", charge_path()));
    assert_eq!(recorded.body, CHARGE_BODY);
    assert_eq!(recorded.header("Idempotency-Key"), Some(CHARGE_KEY));
    assert_eq!(recorded.header("X-Api-Key-Id"), Some(KEY_ID));
    assert_signature_matches(&recorded, &charge_path(), CHARGE_KEY);

    // With the vector timestamp, the same recipe over the same bytes IS the
    // published vector.
    let pinned = sign_request(SignRequest {
        secret: SECRET,
        timestamp: "1755302400",
        method: &recorded.method,
        path: &charge_path(),
        idempotency_key: CHARGE_KEY,
        body: &recorded.body,
    });
    assert_eq!(pinned, CHARGE_SIGNATURE);
}

#[test]
fn charge_payment_method_generates_a_key_and_sends_the_description_last() {
    let server = MockServer::start(vec![charge_ok()]);
    client_for(&server)
        .charge_payment_method(
            PAYMENT_METHOD_ID,
            &ChargeRequest::new(2500, "EUR", "order-1043").description("Monthly plan"),
        )
        .expect("charged");

    let recorded = server.only_request();
    let key = recorded.header("Idempotency-Key").expect("a generated key");
    assert_eq!(key.len(), 36, "a v4 UUID: {key}");
    assert_eq!(
        recorded.body,
        r#"{"amount":2500,"currency":"EUR","orderReference":"order-1043","description":"Monthly plan"}"#
    );
    assert_signature_matches(&recorded, &charge_path(), key);
}

#[test]
fn a_declined_charge_is_a_result_with_a_decline_class_not_an_error() {
    // Through the envelope too: the unwrap path must not eat the decline.
    let server = MockServer::start(vec![Reply::Json(
        201,
        r#"{"success":true,"data":{"chargeId":"chg_2","status":"failed","declineClass":"soft_funds","declineCode":"51","transactionId":"33333333-3333-4333-8333-333333333334"}}"#.to_string(),
    )]);

    let charge = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect("a decline is a result");

    assert_eq!(charge.status, charge_status::FAILED);
    assert!(!charge.is_paid());
    assert!(charge.is_terminal());
    assert_eq!(
        charge.decline_class.as_deref(),
        Some(decline_class::SOFT_FUNDS)
    );
    assert_eq!(charge.decline_code.as_deref(), Some("51"));
}

#[test]
fn a_pending_charge_is_not_terminal_and_neither_is_an_unknown_status() {
    for value in ["pending", "reviewing"] {
        let server = MockServer::start(vec![Reply::Json(
            201,
            format!(r#"{{"chargeId":"chg_3","status":"{value}","transactionId":"t"}}"#),
        )]);
        let charge = client_for(&server)
            .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
            .expect("a charge");
        assert!(!charge.is_paid(), "{value}");
        assert!(
            !charge.is_terminal(),
            "{value} must keep the caller polling"
        );
    }
}

#[test]
fn a_charge_the_gateway_refuses_to_attempt_is_a_refusal_with_its_code() {
    let server = MockServer::start(vec![Reply::enveloped(
        r#"{"success":false,"errorCode":"ALREADY_PROCESSED","errorMessage":"Already charged","transactionId":"33333333-3333-4333-8333-333333333333"}"#,
    )]);

    let error = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("a refusal is not a charge");

    assert!(!error.is_retryable());
    match error {
        Error::Refusal {
            code,
            transaction_id,
            ..
        } => {
            assert_eq!(code, "ALREADY_PROCESSED");
            assert_eq!(
                transaction_id.as_deref(),
                Some("33333333-3333-4333-8333-333333333333")
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_payload_without_a_charge_id_is_a_refusal_whatever_success_says() {
    let server = MockServer::start(vec![Reply::enveloped(r#"{"success":true}"#)]);
    let error = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("no charge id, no charge");
    assert!(matches!(error, Error::Refusal { .. }), "{error:?}");
    assert_eq!(error.code(), Some("UNKNOWN"));
}

#[test]
fn a_charge_against_a_method_that_is_not_yours_is_a_404_api_error() {
    let server = MockServer::start(vec![Reply::error_envelope(
        404,
        "NOT_FOUND",
        "No such payment method",
    )]);
    let error = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("not found");

    assert!(matches!(error, Error::Api { .. }), "{error:?}");
    assert_eq!(error.http_status(), Some(404));
    assert_eq!(error.code(), Some("NOT_FOUND"));
}

#[test]
fn a_5xx_on_a_charge_is_a_retryable_transport_error() {
    let server = MockServer::start(vec![Reply::Html(503, "<h1>down</h1>".to_string())]);
    let error = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("down");
    assert!(matches!(error, Error::Transport { .. }), "{error:?}");
    assert!(error.is_retryable());
}

#[test]
fn charge_payment_method_validates_money_params_like_a_session() {
    let server = MockServer::start(vec![charge_ok()]);
    let client = client_for(&server);

    for (label, bad) in [
        ("zero amount", ChargeRequest::new(0, "EUR", "order-1")),
        (
            "negative amount",
            ChargeRequest::new(-500, "EUR", "order-1"),
        ),
        ("missing currency", ChargeRequest::new(2500, "", "order-1")),
        (
            "missing order reference",
            ChargeRequest::new(2500, "EUR", ""),
        ),
        (
            "over-long order reference",
            ChargeRequest::new(2500, "EUR", "x".repeat(101)),
        ),
        (
            "empty idempotency key",
            ChargeRequest::new(2500, "EUR", "order-1").idempotency_key(""),
        ),
        (
            "over-long idempotency key",
            ChargeRequest::new(2500, "EUR", "order-1").idempotency_key("k".repeat(101)),
        ),
    ] {
        let error = client
            .charge_payment_method(PAYMENT_METHOD_ID, &bad)
            .expect_err(&format!("{label} must be rejected"));
        assert!(
            matches!(error, Error::Validation { .. }),
            "{label}: {error}"
        );
    }

    assert!(
        server.requests().is_empty(),
        "nothing may reach the network on a validation failure"
    );
}

#[test]
fn a_payment_method_id_that_would_not_stay_one_path_segment_is_refused_before_signing() {
    let server = MockServer::start(vec![charge_ok()]);
    let client = client_for(&server);

    for bad in [
        "",
        " ",
        "pm_1/charges",
        "pm_1?x=1",
        "pm_1#f",
        "pm 1",
        "pm_1%2F",
        &"p".repeat(101),
    ] {
        let error = client
            .charge_payment_method(bad, &charge_request())
            .expect_err(&format!("charge accepted {bad:?}"));
        assert!(
            matches!(error, Error::Validation { .. }),
            "{bad:?}: {error}"
        );

        let error = client
            .revoke_payment_method(bad)
            .expect_err(&format!("revoke accepted {bad:?}"));
        assert!(
            matches!(error, Error::Validation { .. }),
            "{bad:?}: {error}"
        );
    }

    assert!(
        server.requests().is_empty(),
        "no bad id may reach the network"
    );
}

#[test]
fn a_padded_payment_method_id_is_trimmed() {
    let server = MockServer::start(vec![revoke_ok()]);
    client_for(&server)
        .revoke_payment_method(&format!("  {PAYMENT_METHOD_ID} "))
        .expect("revoked");
    assert_eq!(server.only_request().path, format!("/api{}", revoke_path()));
}

#[test]
fn revoke_payment_method_signs_the_revoke_vector_and_resolves_on_204() {
    let server = MockServer::start(vec![revoke_ok()]);
    client_for(&server)
        .revoke_payment_method(PAYMENT_METHOD_ID)
        .expect("revoked");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "DELETE");
    assert_eq!(recorded.path, format!("/api{}", revoke_path()));
    assert_eq!(recorded.body, "");
    assert_eq!(
        recorded.header("Idempotency-Key"),
        None,
        "a DELETE must not send an Idempotency-Key header"
    );
    assert_signature_matches(&recorded, &revoke_path(), "");

    let pinned = sign_request(SignRequest {
        secret: SECRET,
        timestamp: "1755302400",
        method: "DELETE",
        path: &revoke_path(),
        idempotency_key: "",
        body: "",
    });
    assert_eq!(pinned, REVOKE_SIGNATURE);
}

#[test]
fn revoke_payment_method_maps_a_404_and_a_5xx() {
    let server = MockServer::start(vec![Reply::error_envelope(
        404,
        "NOT_FOUND",
        "No such payment method",
    )]);
    let error = client_for(&server)
        .revoke_payment_method(PAYMENT_METHOD_ID)
        .expect_err("not found");
    assert!(matches!(error, Error::Api { .. }), "{error:?}");
    assert_eq!(error.http_status(), Some(404));

    let server = MockServer::start(vec![Reply::Json(503, r#"{"success":false}"#.to_string())]);
    let error = client_for(&server)
        .revoke_payment_method(PAYMENT_METHOD_ID)
        .expect_err("down");
    assert!(matches!(error, Error::Transport { .. }), "{error:?}");
}
