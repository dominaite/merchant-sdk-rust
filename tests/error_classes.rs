//! One error class per answer, whatever the route: a coded 5xx keeps its
//! code, a coded charge answer is a charge error, and a storefront code is a
//! storefront error even when it arrives as an HTTP 200 refusal.

#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    charge_error_code, recurring_error_code, session_error_code, transient_error_code,
    ChargeRequest, CheckoutSessionRequest, Client, Error, IdempotencyKey, PlanRequest,
    RefundRequest,
};
use support::{MockServer, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TRANSACTION_ID: &str = "11111111-1111-4111-8111-111111111111";
const PAYMENT_METHOD_ID: &str = "pm_0123456789abcdef0123456789abcdef";

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(server.base_url())
        .timeout(Duration::from_secs(5))
        // Retries stay on; the waits between them cost no time here.
        .retry_sleep(|_| {})
        .build()
        .expect("valid credentials")
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).expect("a valid idempotency key")
}

fn assert_coded_transport(error: &Error, status: u16, code: &str, label: &str) {
    assert!(
        matches!(error, Error::Transport { .. }),
        "{label}: {error:?}"
    );
    assert_eq!(error.http_status(), Some(status), "{label}");
    assert_eq!(error.code(), Some(code), "{label}");
    assert!(error.is_retryable(), "{label}");
    if let Error::Transport { raw, .. } = error {
        assert_eq!(raw["error"]["code"], code, "{label}");
    }
}

#[test]
fn a_coded_5xx_keeps_its_code_on_every_route() {
    let unavailable = transient_error_code::MERCHANT_API_UNAVAILABLE;

    let server = MockServer::start(vec![Reply::error_envelope(503, unavailable, "Try later.")]);
    let error = client_for(&server)
        .create_refund(TRANSACTION_ID, &RefundRequest::new(key("refund-1")))
        .expect_err("unavailable");
    assert_coded_transport(&error, 503, unavailable, "refund");
    assert!(error.to_string().contains("Try later"), "{error}");

    let server = MockServer::start(vec![Reply::error_envelope(503, unavailable, "Try later.")]);
    let error = client_for(&server)
        .create_plan(&PlanRequest::new(
            "Monthly",
            1990,
            "EUR",
            "month",
            key("plan-1"),
        ))
        .expect_err("unavailable");
    assert_coded_transport(&error, 503, unavailable, "plan");

    let server = MockServer::start(vec![Reply::error_envelope(503, unavailable, "Try later.")]);
    let error = client_for(&server)
        .get_plan("plan_0123456789abcdef0123456789abcdef")
        .expect_err("unavailable");
    assert_coded_transport(&error, 503, unavailable, "plan read");
}

#[test]
fn a_coded_500_on_the_charge_and_revoke_routes_is_an_outage_not_an_outcome() {
    let server = MockServer::start(vec![Reply::error_envelope(
        500,
        "INTERNAL_ERROR",
        "An unexpected error occurred",
    )]);
    let error = client_for(&server)
        .charge_payment_method(
            PAYMENT_METHOD_ID,
            &ChargeRequest::new(2500, "EUR", "order-1", key("charge-1")),
        )
        .expect_err("outage");
    assert_coded_transport(&error, 500, "INTERNAL_ERROR", "charge");

    let server = MockServer::start(vec![Reply::error_envelope(
        500,
        "INTERNAL_ERROR",
        "An unexpected error occurred",
    )]);
    let error = client_for(&server)
        .revoke_payment_method(PAYMENT_METHOD_ID)
        .expect_err("outage");
    assert_coded_transport(&error, 500, "INTERNAL_ERROR", "revoke");
}

#[test]
fn a_coded_charge_404_is_a_charge_error() {
    for code in [
        charge_error_code::PAYMENT_METHOD_NOT_FOUND,
        charge_error_code::AGREEMENT_NOT_FOUND,
    ] {
        let server = MockServer::start(vec![Reply::error_envelope(404, code, "Not found.")]);
        let error = client_for(&server)
            .charge_payment_method(
                PAYMENT_METHOD_ID,
                &ChargeRequest::new(2500, "EUR", "order-1", key("charge-1")),
            )
            .expect_err("not found");
        match &error {
            Error::Charge {
                status,
                code: got,
                charge,
                ..
            } => {
                assert_eq!(*status, 404, "{code}");
                assert_eq!(got, code);
                assert!(charge.is_none(), "{code}");
            }
            other => panic!("{code}: expected a charge error, got {other:?}"),
        }
        assert!(!error.is_retryable(), "{code}");
    }
}

#[test]
fn a_coded_404_elsewhere_stays_an_api_error() {
    let server = MockServer::start(vec![Reply::error_envelope(
        404,
        recurring_error_code::PLAN_NOT_FOUND,
        "No plan.",
    )]);
    let error = client_for(&server)
        .get_plan("plan_0123456789abcdef0123456789abcdef")
        .expect_err("not found");
    assert!(matches!(error, Error::Api { status: 404, .. }), "{error:?}");
    assert_eq!(error.code(), Some(recurring_error_code::PLAN_NOT_FOUND));
}

#[test]
fn a_storefront_mismatch_on_a_replay_is_a_storefront_error() {
    let mismatch = session_error_code::STOREFRONT_MISMATCH;
    let payload = format!(
        r#"{{"success":false,"errorCode":"{mismatch}","errorMessage":"Bound to another storefront."}}"#
    );
    let server = MockServer::start(vec![Reply::enveloped(&payload)]);
    let error = client_for(&server)
        .create_checkout_session(&CheckoutSessionRequest::new(
            2500,
            "EUR",
            "order-1",
            key("order-1"),
        ))
        .expect_err("mismatch");

    // The same class as the 400 the API answers without a replay.
    match &error {
        Error::Api {
            status,
            code,
            message,
            raw,
            ..
        } => {
            assert_eq!(*status, 200);
            assert_eq!(code.as_deref(), Some(mismatch));
            assert_eq!(message, "Bound to another storefront.");
            assert_eq!(raw["data"]["errorCode"], mismatch);
        }
        other => panic!("expected the storefront error, got {other:?}"),
    }
    assert!(!error.is_retryable());

    let server = MockServer::start(vec![Reply::error_envelope(400, mismatch, "Bound.")]);
    let error = client_for(&server)
        .create_checkout_session(&CheckoutSessionRequest::new(
            2500,
            "EUR",
            "order-1",
            key("order-1"),
        ))
        .expect_err("mismatch");
    assert!(matches!(error, Error::Api { status: 400, .. }), "{error:?}");
    assert_eq!(error.code(), Some(mismatch));
}
