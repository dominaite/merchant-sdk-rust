//! Stored payment methods against the loopback mock: `save_card` on a session,
//! `stored_payment_method` on its status, then off-session charges and
//! revocation on `/merchant-api/payment-methods/{id}`. No network, no
//! credentials.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    charge_error_code, charge_status, decline_class, revoke_error_code, sign_request,
    stored_payment_method_status, ChargeRequest, CheckoutSessionRequest, Client, Error,
    IdempotencyKey, SignRequest, PAYMENT_METHODS_PATH, SESSIONS_PATH,
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
const CHARGE_ID: &str = "ch_33333333333343338333333333333333";
const CHARGE_TRANSACTION_ID: &str = "33333333-3333-4333-8333-333333333333";
const CHARGE: &str = r#"{"chargeId":"ch_33333333333343338333333333333333","status":"succeeded","declineClass":null,"declineCode":null,"transactionId":"33333333-3333-4333-8333-333333333333"}"#;

/// The gateway envelope around a charge answer: `data` when there is a charge
/// row, `error` when there is a code.
fn charge_envelope(status: u16, code: Option<(&str, &str)>, data: Option<&str>) -> Reply {
    let mut parts = vec![format!(r#""success":{}"#, code.is_none())];
    if let Some(data) = data {
        parts.push(format!(r#""data":{data}"#));
    }
    if let Some((code, message)) = code {
        parts.push(format!(
            r#""error":{{"message":"{message}","code":"{code}","statusCode":{status},"timestamp":"2026-09-15T18:02:11.4183920Z"}}"#
        ));
    }
    parts.push(r#""metadata":{"requestId":"c2a1e6d4-3b5f-4c7e-9a8d-1f2e3d4c5b6a"}"#.to_string());
    Reply::Json(status, format!("{{{}}}", parts.join(",")))
}

fn create_ok() -> Reply {
    Reply::enveloped(&format!(r#"{{"success":true,"checkout":{CHECKOUT}}}"#))
}

fn charge_ok() -> Reply {
    charge_envelope(201, None, Some(CHARGE))
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

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).expect("a valid idempotency key")
}

fn charge_request() -> ChargeRequest {
    ChargeRequest::new(2500, "EUR", "order-1043", key(CHARGE_KEY))
}

fn charge_for(amount: i64, currency: &str, order_reference: &str) -> ChargeRequest {
    ChargeRequest::new(amount, currency, order_reference, key(CHARGE_KEY))
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
            &CheckoutSessionRequest::new(
                2500,
                "EUR",
                "order-1042",
                key("00000000-0000-4000-8000-000000000001"),
            )
            .save_card(true),
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
        .create_checkout_session(&CheckoutSessionRequest::new(
            2500,
            "EUR",
            "order-1042",
            key("00000000-0000-4000-8000-000000000001"),
        ))
        .expect("session created");

    assert_eq!(
        server.only_request().body,
        r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042"}"#
    );
}

#[test]
fn get_status_carries_the_stored_payment_method() {
    // paymentMethod is the gateway's string category of how the payer paid; it
    // is not the stored card and stays on `raw` untyped.
    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"transactionId":"{TRANSACTION_ID}","status":"succeeded","amount":2500,"currency":"EUR","paymentMethod":"card","storedPaymentMethod":{{"id":"{PAYMENT_METHOD_ID}","brand":"visa","last4":"4242","expiryMonth":12,"expiryYear":2029,"status":"active"}}}}"#
    ))]);

    let status = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect("status read");

    let method = status
        .stored_payment_method
        .expect("a stored payment method");
    assert_eq!(method.id, PAYMENT_METHOD_ID);
    assert_eq!(method.brand.as_deref(), Some("visa"));
    assert_eq!(method.last4.as_deref(), Some("4242"));
    assert_eq!(
        (method.expiry_month, method.expiry_year),
        (Some(12), Some(2029))
    );
    assert_eq!(method.status, stored_payment_method_status::ACTIVE);
    assert!(method.is_chargeable());
    assert_eq!(status.raw["paymentMethod"], "card");
}

#[test]
fn get_status_without_a_saved_card_leaves_stored_payment_method_none() {
    // The gateway omits null fields on the wire, so both spellings must read
    // the same.
    for tail in ["", r#","storedPaymentMethod":null"#] {
        let server = MockServer::start(vec![Reply::enveloped(&format!(
            r#"{{"transactionId":"{TRANSACTION_ID}","status":"succeeded","amount":2500,"currency":"EUR","paymentMethod":"card"{tail}}}"#
        ))]);

        let status = client_for(&server)
            .get_status(TRANSACTION_ID)
            .expect("status read");

        assert_eq!(status.stored_payment_method, None, "{tail:?}");
    }
}

#[test]
fn a_stored_payment_method_the_provider_did_not_describe_reads_as_none_fields() {
    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"transactionId":"{TRANSACTION_ID}","status":"succeeded","amount":2500,"currency":"EUR","storedPaymentMethod":{{"id":"{PAYMENT_METHOD_ID}","status":"active"}}}}"#
    ))]);

    let status = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect("status read");

    let method = status.stored_payment_method.expect("present");
    assert_eq!(method.brand, None);
    assert_eq!(method.last4, None);
    assert_eq!(method.expiry_month, None);
    assert_eq!(method.expiry_year, None);
    assert!(method.is_chargeable());
}

#[test]
fn a_revoked_or_unknown_payment_method_status_is_not_chargeable() {
    for value in ["revoked", "expired", "frozen"] {
        let server = MockServer::start(vec![Reply::enveloped(&format!(
            r#"{{"transactionId":"{TRANSACTION_ID}","status":"succeeded","amount":2500,"currency":"EUR","storedPaymentMethod":{{"id":"{PAYMENT_METHOD_ID}","brand":"visa","last4":"4242","expiryMonth":12,"expiryYear":2029,"status":"{value}"}}}}"#
        ))]);
        let status = client_for(&server)
            .get_status(TRANSACTION_ID)
            .expect("status read");
        assert!(
            !status
                .stored_payment_method
                .expect("present")
                .is_chargeable(),
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

    assert_eq!(charge.charge_id, CHARGE_ID);
    assert_eq!(charge.status, charge_status::SUCCEEDED);
    assert!(charge.is_paid());
    assert!(charge.is_terminal());
    assert_eq!(charge.decline_class, None);
    assert_eq!(charge.decline_code, None);
    assert_eq!(charge.transaction_id, CHARGE_TRANSACTION_ID);
    // raw is the unwrapped charge object, not the envelope.
    assert_eq!(charge.raw["chargeId"], CHARGE_ID);
    assert!(charge.raw.get("success").is_none());

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
fn charge_payment_method_sends_the_callers_key_and_the_description_last() {
    let server = MockServer::start(vec![charge_ok()]);
    client_for(&server)
        .charge_payment_method(
            PAYMENT_METHOD_ID,
            &ChargeRequest::new(2500, "EUR", "order-1043", key("sub-8817-2026-10"))
                .description("Monthly plan"),
        )
        .expect("charged");

    let recorded = server.only_request();
    assert_eq!(recorded.header("Idempotency-Key"), Some("sub-8817-2026-10"));
    assert_eq!(
        recorded.body,
        r#"{"amount":2500,"currency":"EUR","orderReference":"order-1043","description":"Monthly plan"}"#
    );
    assert_signature_matches(&recorded, &charge_path(), "sub-8817-2026-10");
}

#[test]
fn a_402_decline_is_a_result_with_a_decline_class_not_an_error() {
    // The envelope says success=false and names CHARGE_DECLINED, but the charge
    // row is right there: a decline is a result, not an error.
    let server = MockServer::start(vec![charge_envelope(
        402,
        Some((
            "CHARGE_DECLINED",
            "The payment provider declined the charge.",
        )),
        Some(
            r#"{"chargeId":"ch_33333333333343338333333333333334","status":"failed","declineClass":"soft_funds","declineCode":"51","transactionId":"33333333-3333-4333-8333-333333333334"}"#,
        ),
    )]);

    let charge = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect("a decline is a result");

    assert_eq!(charge.charge_id, "ch_33333333333343338333333333333334");
    assert_eq!(charge.status, charge_status::FAILED);
    assert!(!charge.is_paid());
    assert!(charge.is_terminal());
    assert_eq!(
        charge.decline_class.as_deref(),
        Some(decline_class::SOFT_FUNDS)
    );
    assert_eq!(charge.decline_code.as_deref(), Some("51"));
    assert_eq!(
        charge.transaction_id,
        "33333333-3333-4333-8333-333333333334"
    );
}

#[test]
fn a_200_replay_is_returned_as_the_charge() {
    // A durable replay of the same key answers 200 with the original charge.
    let server = MockServer::start(vec![charge_envelope(200, None, Some(CHARGE))]);

    let charge = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect("a replay is the charge");

    assert_eq!(charge.charge_id, CHARGE_ID);
    assert!(charge.is_paid());
}

#[test]
fn a_pending_charge_is_not_terminal_and_neither_is_an_unknown_status() {
    for value in ["pending", "reviewing"] {
        let server = MockServer::start(vec![charge_envelope(
            201,
            None,
            Some(&format!(
                r#"{{"chargeId":"{CHARGE_ID}","status":"{value}","transactionId":"{CHARGE_TRANSACTION_ID}"}}"#
            )),
        )]);
        let charge = client_for(&server)
            .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
            .expect("a charge");
        assert!(!charge.is_paid(), "{value}");
        assert!(
            !charge.is_terminal(),
            "{value} must keep the caller polling"
        );
        // Absent on the wire reads as None, like null.
        assert_eq!(charge.decline_class, None);
        assert_eq!(charge.decline_code, None);
    }
}

#[test]
fn a_cancelled_charge_is_terminal_and_not_paid() {
    let server = MockServer::start(vec![charge_envelope(
        201,
        None,
        Some(&format!(
            r#"{{"chargeId":"{CHARGE_ID}","status":"cancelled","transactionId":"{CHARGE_TRANSACTION_ID}"}}"#
        )),
    )]);
    let charge = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect("a charge");
    assert_eq!(charge.status, charge_status::CANCELLED);
    assert!(!charge.is_paid());
    assert!(charge.is_terminal());
}

#[test]
fn charge_outcome_unknown_carries_the_transaction_to_poll() {
    let server = MockServer::start(vec![charge_envelope(
        502,
        Some((
            "CHARGE_OUTCOME_UNKNOWN",
            "The payment provider gave no verdict.",
        )),
        Some(&format!(
            r#"{{"chargeId":"{CHARGE_ID}","status":"pending","declineClass":null,"declineCode":null,"transactionId":"{CHARGE_TRANSACTION_ID}"}}"#
        )),
    )]);

    let error = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("no verdict is not a charge");

    // Never blind-retried: the money may have moved.
    assert!(!error.is_retryable());
    assert_eq!(error.http_status(), Some(502));
    assert_eq!(
        error.code(),
        Some(charge_error_code::CHARGE_OUTCOME_UNKNOWN)
    );
    assert_eq!(
        error.to_string(),
        "charge error (HTTP 502, CHARGE_OUTCOME_UNKNOWN): The payment provider gave no verdict."
    );
    match error {
        Error::Charge {
            status,
            code,
            message,
            charge,
            transaction_id,
            raw,
        } => {
            assert_eq!(status, 502);
            assert_eq!(code, charge_error_code::CHARGE_OUTCOME_UNKNOWN);
            assert_eq!(message, "The payment provider gave no verdict.");
            let charge = charge.expect("the charge row is attached");
            assert_eq!(charge.charge_id, CHARGE_ID);
            assert_eq!(charge.status, charge_status::PENDING);
            assert_eq!(charge.decline_class, None);
            assert_eq!(transaction_id.as_deref(), Some(CHARGE_TRANSACTION_ID));
            assert_eq!(raw["success"], false);
            assert_eq!(raw["error"]["code"], "CHARGE_OUTCOME_UNKNOWN");
            assert_eq!(raw["data"]["chargeId"], CHARGE_ID);
        }
        other => panic!("expected a charge error, got {other:?}"),
    }
}

#[test]
fn charge_errors_without_data_have_no_charge() {
    for (status, code) in [
        (409, charge_error_code::PAYMENT_METHOD_NOT_ACTIVE),
        (409, charge_error_code::DUPLICATE_REQUEST),
        (422, charge_error_code::IDEMPOTENCY_KEY_REUSED),
        (502, charge_error_code::CHARGE_FAILED),
        (503, charge_error_code::PAYMENT_METHOD_CHARGES_DISABLED),
        (503, charge_error_code::PAYMENT_PROCESSING_UNAVAILABLE),
        // The gateway can add a code; it must still arrive typed.
        (409, "A_NEW_CODE"),
    ] {
        let server =
            MockServer::start(vec![charge_envelope(status, Some((code, "refused")), None)]);
        let error = client_for(&server)
            .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
            .expect_err(code);

        assert!(!error.is_retryable(), "{code}");
        assert_eq!(error.http_status(), Some(status), "{code}");
        assert_eq!(error.code(), Some(code));
        match error {
            Error::Charge {
                charge,
                transaction_id,
                raw,
                ..
            } => {
                assert!(charge.is_none(), "{code}");
                assert_eq!(transaction_id, None, "{code}");
                assert_eq!(raw["error"]["code"], code);
            }
            other => panic!("{code}: expected a charge error, got {other:?}"),
        }
    }
}

#[test]
fn a_charge_against_a_method_that_is_not_yours_is_a_404_api_error() {
    let server = MockServer::start(vec![Reply::error_envelope(
        404,
        "PAYMENT_METHOD_NOT_FOUND",
        "No stored payment method with this id.",
    )]);
    let error = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("not found");

    assert!(matches!(error, Error::Api { .. }), "{error:?}");
    assert_eq!(error.http_status(), Some(404));
    assert_eq!(error.code(), Some("PAYMENT_METHOD_NOT_FOUND"));
}

#[test]
fn a_charge_keeps_the_generic_errors_for_the_generic_statuses() {
    // A 400 with a code is input validation, not a charge outcome.
    let server = MockServer::start(vec![Reply::error_envelope(
        400,
        "IDEMPOTENCY_KEY_REQUIRED",
        "Idempotency-Key is required",
    )]);
    let error = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("rejected");
    assert!(matches!(error, Error::Api { .. }), "{error:?}");
    assert_eq!(error.http_status(), Some(400));
    assert_eq!(error.code(), Some("IDEMPOTENCY_KEY_REQUIRED"));

    // A 5xx without a gateway code is an outage, whatever the body looks like.
    for reply in [
        Reply::Html(503, "<h1>down</h1>".to_string()),
        Reply::Json(503, r#"{"success":false}"#.to_string()),
        Reply::Json(502, String::new()),
    ] {
        let server = MockServer::start(vec![reply.clone()]);
        let error = client_for(&server)
            .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
            .expect_err("down");
        assert!(
            matches!(error, Error::Transport { .. }),
            "{reply:?}: {error:?}"
        );
        assert!(error.is_retryable(), "{reply:?}");
    }
}

#[test]
fn a_201_without_a_charge_body_is_an_api_error() {
    for body in [r#"{"success":true}"#, r#"{"success":true,"data":{}}"#] {
        let server = MockServer::start(vec![Reply::Json(201, body.to_string())]);
        let error = client_for(&server)
            .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
            .expect_err("no charge id, no charge");
        assert!(matches!(error, Error::Api { .. }), "{body}: {error:?}");
        assert_eq!(error.http_status(), Some(201), "{body}");
        assert_eq!(error.code(), None, "{body}");
    }
}

#[test]
fn charge_payment_method_validates_money_params_like_a_session() {
    let server = MockServer::start(vec![charge_ok()]);
    let client = client_for(&server);

    for (label, bad) in [
        ("zero amount", charge_for(0, "EUR", "order-1")),
        ("negative amount", charge_for(-500, "EUR", "order-1")),
        ("missing currency", charge_for(2500, "", "order-1")),
        ("missing order reference", charge_for(2500, "EUR", "")),
        (
            "over-long order reference",
            charge_for(2500, "EUR", &"x".repeat(101)),
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
fn revoke_payment_method_returns_revoke_errors_for_coded_failures() {
    for (status, code, message) in [
        (
            502,
            revoke_error_code::UPSTREAM_CONTRACT_ERROR,
            "The payment provider refused to delete the stored credential.",
        ),
        (
            503,
            revoke_error_code::MERCHANT_API_UNAVAILABLE,
            "The payment provider is unavailable. Nothing changed; retry later.",
        ),
        (502, "A_NEW_CODE", "refused"),
    ] {
        let server = MockServer::start(vec![Reply::error_envelope(status, code, message)]);
        let error = client_for(&server)
            .revoke_payment_method(PAYMENT_METHOD_ID)
            .expect_err(code);

        assert!(!error.is_retryable(), "{code}");
        assert_eq!(error.http_status(), Some(status), "{code}");
        assert_eq!(error.code(), Some(code));
        assert_eq!(
            error.to_string(),
            format!("revoke error (HTTP {status}, {code}): {message}")
        );
        match error {
            Error::Revoke { raw, .. } => {
                assert_eq!(raw["success"], false);
                assert_eq!(raw["error"]["code"], code);
            }
            other => panic!("{code}: expected a revoke error, got {other:?}"),
        }
    }
}

#[test]
fn revoke_payment_method_maps_a_404_and_a_codeless_5xx() {
    // The gateway's 404 for an id that is not yours is a VALIDATION_ERROR
    // envelope; it stays the generic API error with that code.
    let server = MockServer::start(vec![Reply::Json(
        404,
        format!(
            r#"{{"success":false,"error":{{"message":"Validation failed","code":"VALIDATION_ERROR","statusCode":404,"validationErrors":[{{"field":"id","message":"'{PAYMENT_METHOD_ID}' not found","code":"VALIDATION_FAILED"}}]}}}}"#
        ),
    )]);
    let error = client_for(&server)
        .revoke_payment_method(PAYMENT_METHOD_ID)
        .expect_err("not found");
    assert!(matches!(error, Error::Api { .. }), "{error:?}");
    assert_eq!(error.http_status(), Some(404));
    assert_eq!(error.code(), Some("VALIDATION_ERROR"));

    for reply in [
        Reply::Json(503, r#"{"success":false}"#.to_string()),
        Reply::Html(503, "<h1>down</h1>".to_string()),
    ] {
        let server = MockServer::start(vec![reply.clone()]);
        let error = client_for(&server)
            .revoke_payment_method(PAYMENT_METHOD_ID)
            .expect_err("down");
        assert!(
            matches!(error, Error::Transport { .. }),
            "{reply:?}: {error:?}"
        );
        assert!(error.is_retryable());
    }
}
