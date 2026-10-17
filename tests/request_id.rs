//! Every result and every error carries the id of the call that produced it:
//! the `Dominaite-Request-Id` header when the API sends one, else the body's
//! `metadata.requestId`. Errors the client raises before sending carry none.

#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    ChargeRequest, CheckoutSessionRequest, Client, Error, IdempotencyKey, RefundRequest,
};
use support::{MockServer, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TRANSACTION_ID: &str = "11111111-1111-4111-8111-111111111111";
const PAYMENT_METHOD_ID: &str = "pm_0123456789abcdef0123456789abcdef";
const CHECKOUT: &str = r#"{"transactionId":"11111111-1111-4111-8111-111111111111","orderId":"dom_42","cashierKey":"ck_live","cashierToken":"ct_live","amount":2500,"currency":"EUR","expiresAt":"2026-08-20T12:00:00Z"}"#;
const REFUND: &str = r#"{"refundId":"re_7c1e9a2b4d6f48a0b3c5d7e9f1a2b3c4","transactionId":"11111111-1111-4111-8111-111111111111","status":"pending","amount":500,"currency":"EUR"}"#;
const CHARGE: &str = r#"{"chargeId":"ch_33333333333343338333333333333333","status":"failed","declineClass":"soft_funds","declineCode":"51","transactionId":"33333333-3333-4333-8333-333333333333"}"#;
const HEADER_ID: &str = "req_from_header";
const BODY_ID: &str = "req_from_body";

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(server.base_url())
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).expect("a valid idempotency key")
}

fn session_request() -> CheckoutSessionRequest {
    CheckoutSessionRequest::new(2500, "EUR", "order-1042", key("order-1042"))
}

/// A success envelope around `data`, with `metadata.requestId`.
fn ok_body(data: &str) -> String {
    format!(r#"{{"success":true,"data":{data},"metadata":{{"requestId":"{BODY_ID}"}}}}"#)
}

/// An error envelope, with `metadata.requestId`.
fn error_body(code: &str, data: &str) -> String {
    format!(
        r#"{{"success":false,"data":{data},"error":{{"code":"{code}","message":"No."}},"metadata":{{"requestId":"{BODY_ID}"}}}}"#
    )
}

fn with_header(status: u16, body: String) -> Reply {
    Reply::JsonWithHeaders(
        status,
        body,
        vec![("Dominaite-Request-Id".to_string(), HEADER_ID.to_string())],
    )
}

#[test]
fn the_header_wins_over_the_body_request_id() {
    let checkout = format!(r#"{{"success":true,"checkout":{CHECKOUT}}}"#);
    let server = MockServer::start(vec![with_header(200, ok_body(&checkout))]);
    let session = client_for(&server)
        .create_checkout_session(&session_request())
        .expect("session");
    assert_eq!(session.request_id.as_deref(), Some(HEADER_ID));

    let server = MockServer::start(vec![with_header(400, error_body("INVALID_AMOUNT", "null"))]);
    let error = client_for(&server)
        .create_checkout_session(&session_request())
        .expect_err("rejected");
    assert_eq!(error.request_id(), Some(HEADER_ID));
}

#[test]
fn the_body_request_id_fills_in_without_the_header() {
    let checkout = format!(r#"{{"success":true,"checkout":{CHECKOUT}}}"#);
    let server = MockServer::start(vec![Reply::Json(200, ok_body(&checkout))]);
    let session = client_for(&server)
        .create_checkout_session(&session_request())
        .expect("session");
    assert_eq!(session.request_id.as_deref(), Some(BODY_ID));

    let server = MockServer::start(vec![Reply::Json(202, ok_body(REFUND))]);
    let refund = client_for(&server)
        .create_refund(TRANSACTION_ID, &RefundRequest::new(key("refund-1")))
        .expect("refund");
    assert_eq!(refund.request_id.as_deref(), Some(BODY_ID));

    let list = format!(r#"{{"transactionId":"{TRANSACTION_ID}","refunds":[{REFUND}]}}"#);
    let server = MockServer::start(vec![Reply::Json(200, ok_body(&list))]);
    let refunds = client_for(&server)
        .list_refunds(TRANSACTION_ID)
        .expect("refund list");
    assert_eq!(refunds.request_id.as_deref(), Some(BODY_ID));
    assert_eq!(refunds.refunds[0].request_id.as_deref(), Some(BODY_ID));
}

#[test]
fn a_reply_without_any_request_id_leaves_it_empty() {
    let checkout = format!(r#"{{"success":true,"checkout":{CHECKOUT}}}"#);
    let server = MockServer::start(vec![Reply::enveloped(&checkout)]);
    let session = client_for(&server)
        .create_checkout_session(&session_request())
        .expect("session");
    assert_eq!(session.request_id, None);
}

#[test]
fn every_api_error_carries_the_request_id() {
    let refusal = r#"{"success":false,"errorCode":"DUPLICATE_REQUEST","errorMessage":"No."}"#;
    let server = MockServer::start(vec![Reply::Json(200, ok_body(refusal))]);
    let error = client_for(&server)
        .create_checkout_session(&session_request())
        .expect_err("refused");
    assert!(matches!(error, Error::Refusal { .. }), "{error:?}");
    assert_eq!(error.request_id(), Some(BODY_ID));

    let charge_request = ChargeRequest::new(2500, "EUR", "order-1043", key("charge-1"));
    let server = MockServer::start(vec![Reply::Json(
        402,
        error_body("CHARGE_DECLINED", CHARGE),
    )]);
    let declined = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request)
        .expect("a decline is a charge result");
    assert_eq!(declined.request_id.as_deref(), Some(BODY_ID));

    let pending = CHARGE.replace(r#""status":"failed""#, r#""status":"pending""#);
    let server = MockServer::start(vec![Reply::Json(
        502,
        error_body("CHARGE_OUTCOME_UNKNOWN", &pending),
    )]);
    let error = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request)
        .expect_err("outcome unknown");
    assert_eq!(error.request_id(), Some(BODY_ID));
    match &error {
        Error::Charge { charge, .. } => {
            let charge = charge.as_ref().expect("the charge body");
            assert_eq!(charge.request_id.as_deref(), Some(BODY_ID));
        }
        other => panic!("expected a charge error, got {other:?}"),
    }

    let server = MockServer::start(vec![Reply::RateLimitedWith(
        Some("30".to_string()),
        error_body("RATE_LIMITED", "null"),
    )]);
    let error = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect_err("limited");
    assert!(matches!(error, Error::RateLimited { .. }), "{error:?}");
    assert_eq!(error.request_id(), Some(BODY_ID));

    let server = MockServer::start(vec![Reply::Json(
        401,
        error_body("INVALID_SIGNATURE", "null"),
    )]);
    let error = client_for(&server).ping().expect_err("unauthorized");
    assert!(matches!(error, Error::Auth { .. }), "{error:?}");
    assert_eq!(error.request_id(), Some(BODY_ID));
}

#[test]
fn a_validation_error_has_no_request_id() {
    let server = MockServer::start(vec![Reply::enveloped("{}")]);
    let error = client_for(&server)
        .charge_payment_method(
            "pm_../../refunds",
            &ChargeRequest::new(2500, "EUR", "order-1043", key("charge-1")),
        )
        .expect_err("invalid id");
    assert!(matches!(error, Error::Validation { .. }), "{error:?}");
    assert_eq!(error.request_id(), None);
    assert!(server.requests().is_empty());
}
