//! Refunds against the loopback mock: `create_refund` on
//! `/merchant-api/payments/{id}/refunds` and `get_refund` on
//! `/merchant-api/payments/{id}/refunds/{refund_id}`. No network, no
//! credentials.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    refund_error_code, refund_failure_code, refund_status, sign_request, to_minor_units, Client,
    Error, IdempotencyKey, RefundRequest, SignRequest, PAYMENTS_PATH,
};
use support::{MockServer, Recorded, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TRANSACTION_ID: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
const REFUND_ID: &str = "re_7c1e9a2b4d6f48a0b3c5d7e9f1a2b3c4";
const REFUND_KEY: &str = "return-7731";

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

fn refunds_path() -> String {
    format!("{PAYMENTS_PATH}/{TRANSACTION_ID}/refunds")
}

fn refund_path() -> String {
    format!("{PAYMENTS_PATH}/{TRANSACTION_ID}/refunds/{REFUND_ID}")
}

/// The gateway envelope around a refund, at the status the route answers with.
fn refund_reply(status: u16, data: &str) -> Reply {
    Reply::Json(
        status,
        format!(r#"{{"success":true,"data":{data},"metadata":{{"requestId":"r1"}}}}"#),
    )
}

fn pending(amount: Option<i64>, currency: &str) -> String {
    let amount = amount
        .map(|value| format!(r#""amount":{value},"#))
        .unwrap_or_default();
    format!(
        r#"{{"refundId":"{REFUND_ID}","transactionId":"{TRANSACTION_ID}","status":"pending",{amount}"currency":"{currency}"}}"#
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
fn a_partial_refund_sends_the_amount_and_signs_the_key_in_header_and_signature() {
    let amount = to_minor_units("1500", "HUF").expect("HUF has no minor unit");
    let server = MockServer::start(vec![refund_reply(202, &pending(Some(amount), "HUF"))]);

    let refund = client_for(&server)
        .create_refund(
            TRANSACTION_ID,
            &RefundRequest::new(key(REFUND_KEY))
                .amount(amount)
                .reason("Returned one item"),
        )
        .expect("a 202 is a refund");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "POST");
    // The signed path is the canonical path; the request goes to the /api prefix.
    assert_eq!(recorded.path, format!("/api{}", refunds_path()));
    assert_eq!(
        recorded.body,
        r#"{"amount":1500,"reason":"Returned one item"}"#
    );
    assert_eq!(recorded.header("Idempotency-Key"), Some(REFUND_KEY));
    assert_signature_matches(&recorded, &refunds_path(), REFUND_KEY);

    assert_eq!(refund.refund_id, REFUND_ID);
    assert_eq!(refund.transaction_id, TRANSACTION_ID);
    assert_eq!(refund.status, refund_status::PENDING);
    assert_eq!(refund.amount, Some(1500));
    assert_eq!(refund.currency, "HUF");
    assert_eq!(refund.failure_code, None);
    assert_eq!(refund.completed_at, None);
    assert!(!refund.is_terminal());
    assert_eq!(refund.failure(), None);
    // raw is the unwrapped refund object, not the envelope.
    assert_eq!(refund.raw["refundId"], REFUND_ID);
    assert!(refund.raw.get("success").is_none());
}

#[test]
fn a_full_refund_sends_no_amount_key_at_all() {
    let server = MockServer::start(vec![refund_reply(202, &pending(None, "EUR"))]);

    let refund = client_for(&server)
        .create_refund(TRANSACTION_ID, &RefundRequest::new(key(REFUND_KEY)))
        .expect("a 202 is a refund");

    let recorded = server.only_request();
    // Omitted, not null: a null amount is not "everything still refundable".
    assert_eq!(recorded.body, "{}");
    assert_eq!(recorded.header("Idempotency-Key"), Some(REFUND_KEY));
    assert_signature_matches(&recorded, &refunds_path(), REFUND_KEY);

    // A full refund reads no amount until it succeeds.
    assert_eq!(refund.amount, None);
    assert_eq!(refund.status, refund_status::PENDING);
}

#[test]
fn the_transaction_id_is_signed_in_lowercase() {
    let server = MockServer::start(vec![refund_reply(202, &pending(None, "EUR"))]);

    client_for(&server)
        .create_refund(
            &format!("  {}  ", TRANSACTION_ID.to_uppercase()),
            &RefundRequest::new(key(REFUND_KEY)),
        )
        .expect("a 202 is a refund");

    let recorded = server.only_request();
    assert_eq!(recorded.path, format!("/api{}", refunds_path()));
    assert_signature_matches(&recorded, &refunds_path(), REFUND_KEY);
}

#[test]
fn get_refund_is_a_get_signed_with_an_empty_key_and_body() {
    let server = MockServer::start(vec![refund_reply(
        200,
        &format!(
            r#"{{"refundId":"{REFUND_ID}","transactionId":"{TRANSACTION_ID}","status":"succeeded","amount":2500,"currency":"EUR","completedAt":"2026-09-26T10:05:40.1200000Z"}}"#
        ),
    )]);

    let refund = client_for(&server)
        .get_refund(TRANSACTION_ID, REFUND_ID)
        .expect("a 200 is a refund");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "GET");
    assert_eq!(recorded.path, format!("/api{}", refund_path()));
    assert_eq!(recorded.body, "");
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_signature_matches(&recorded, &refund_path(), "");

    assert_eq!(refund.status, refund_status::SUCCEEDED);
    assert!(refund.is_succeeded());
    assert!(refund.is_terminal());
    assert_eq!(refund.amount, Some(2500));
    assert_eq!(
        refund.completed_at.as_deref(),
        Some("2026-09-26T10:05:40.1200000Z")
    );
    assert_eq!(refund.failure(), None);
}

#[test]
fn a_failed_refund_has_no_amount_and_reads_its_failure_code() {
    // Both wire spellings: the gateway omits nulls, so an explicit null and a
    // missing amount must read the same.
    for amount in ["", r#""amount":null,"#] {
        let server = MockServer::start(vec![refund_reply(
            200,
            &format!(
                r#"{{"refundId":"{REFUND_ID}","transactionId":"{TRANSACTION_ID}","status":"failed",{amount}"currency":"EUR","failureCode":"REFUND_FAILED","failureMessage":"The refund could not be completed.","completedAt":"2026-09-26T10:05:41.0000000Z"}}"#
            ),
        )]);

        let refund = client_for(&server)
            .get_refund(TRANSACTION_ID, REFUND_ID)
            .expect("a failed refund is a result, not an error");

        assert_eq!(refund.status, refund_status::FAILED, "{amount:?}");
        assert_eq!(refund.amount, None, "{amount:?}");
        assert_eq!(
            refund.failure_code.as_deref(),
            Some(refund_failure_code::REFUND_FAILED)
        );
        assert_eq!(refund.failure(), Some(refund_failure_code::REFUND_FAILED));
        assert_eq!(
            refund.failure_message.as_deref(),
            Some("The refund could not be completed.")
        );
        assert!(refund.is_terminal());
        assert!(!refund.is_succeeded());
    }
}

#[test]
fn an_unknown_or_missing_failure_code_reads_as_refund_failed() {
    let parse = |failure: &str| {
        let refund: dominaite::Refund = serde_json::from_str(&format!(
            r#"{{"refundId":"{REFUND_ID}","transactionId":"{TRANSACTION_ID}","status":"failed","currency":"EUR"{failure}}}"#
        ))
        .expect("deserializes");
        refund.failure()
    };

    assert_eq!(
        parse(r#","failureCode":"PROVIDER_ON_FIRE""#),
        Some(refund_failure_code::REFUND_FAILED)
    );
    assert_eq!(parse(""), Some(refund_failure_code::REFUND_FAILED));
    for known in refund_failure_code::ALL {
        assert_eq!(
            parse(&format!(r#","failureCode":"{known}""#)),
            Some(known),
            "{known} is a known code and passes through"
        );
    }
}

#[test]
fn only_succeeded_and_failed_are_terminal() {
    for value in refund_status::ALL.into_iter().chain(["reversed"]) {
        let refund: dominaite::Refund = serde_json::from_str(&format!(
            r#"{{"refundId":"{REFUND_ID}","transactionId":"{TRANSACTION_ID}","status":"{value}","currency":"EUR"}}"#
        ))
        .expect("deserializes");
        assert_eq!(
            refund.is_terminal(),
            value == refund_status::SUCCEEDED || value == refund_status::FAILED,
            "{value}"
        );
        assert_eq!(refund.is_succeeded(), value == refund_status::SUCCEEDED);
    }
}

/// Every code the refund routes answer with, the HTTP status it comes with, and
/// how long the same request is worth resending.
const REFUND_ERRORS: [(&str, u16, Option<u64>); 7] = [
    (refund_error_code::PAYMENT_NOT_FOUND, 404, None),
    (refund_error_code::REFUND_NOT_FOUND, 404, Some(60)),
    (refund_error_code::PAYMENT_NOT_REFUNDABLE, 422, None),
    (refund_error_code::REFUND_AMOUNT_EXCEEDED, 422, None),
    (refund_error_code::IDEMPOTENCY_KEY_REUSED, 422, None),
    (refund_error_code::DUPLICATE_REQUEST, 409, Some(120)),
    (refund_error_code::IDEMPOTENCY_KEY_REQUIRED, 400, None),
];

#[test]
fn every_refund_error_code_is_an_api_error_with_its_status_and_retry_window() {
    assert_eq!(
        REFUND_ERRORS.map(|(code, _, _)| code),
        refund_error_code::ALL,
        "every listed code is covered here"
    );

    for (code, http_status, window) in REFUND_ERRORS {
        for read in [false, true] {
            let label = format!("{code} ({http_status}, read: {read})");
            let server =
                MockServer::start(vec![Reply::error_envelope(http_status, code, "refused")]);
            let client = client_for(&server);
            let error = if read {
                client.get_refund(TRANSACTION_ID, REFUND_ID)
            } else {
                client.create_refund(TRANSACTION_ID, &RefundRequest::new(key(REFUND_KEY)))
            }
            .expect_err(&label);

            assert!(matches!(error, Error::Api { .. }), "{label}: {error:?}");
            assert_eq!(error.code(), Some(code), "{label}");
            assert_eq!(error.http_status(), Some(http_status), "{label}");
            // Never a blind retry: the two codes worth resending want a pause,
            // which is what the window says.
            assert!(!error.is_retryable(), "{label}");
            assert_eq!(
                refund_error_code::retry_window_seconds(code),
                window,
                "{label}"
            );
        }
    }
}

#[test]
fn a_500_means_nothing_was_queued_and_is_retryable_with_the_same_key() {
    let server = MockServer::start(vec![Reply::error_envelope(
        500,
        "INTERNAL_ERROR",
        "Something went wrong.",
    )]);

    let error = client_for(&server)
        .create_refund(TRANSACTION_ID, &RefundRequest::new(key(REFUND_KEY)))
        .expect_err("a 500 is not a refund");

    assert!(matches!(error, Error::Transport { .. }), "{error:?}");
    assert!(error.is_retryable());
}

#[test]
fn bad_arguments_are_rejected_before_anything_is_sent() {
    let server = MockServer::start(vec![refund_reply(202, &pending(None, "EUR"))]);
    let client = client_for(&server);

    for amount in [0, -1] {
        let error = client
            .create_refund(
                TRANSACTION_ID,
                &RefundRequest::new(key(REFUND_KEY)).amount(amount),
            )
            .expect_err("a non-positive amount");
        assert!(matches!(error, Error::Validation { .. }), "{amount}");
    }

    let error = client
        .create_refund("order-1042", &RefundRequest::new(key(REFUND_KEY)))
        .expect_err("not a transaction id");
    assert!(matches!(error, Error::Validation { .. }));

    for refund_id in ["", "re_1/../x", "re_1?x=1", "re 1"] {
        let error = client
            .get_refund(TRANSACTION_ID, refund_id)
            .expect_err("not a refund id");
        assert!(matches!(error, Error::Validation { .. }), "{refund_id:?}");
    }

    assert!(server.requests().is_empty(), "nothing reached the network");
}

#[test]
fn a_202_without_a_refund_body_is_an_api_error() {
    let server = MockServer::start(vec![refund_reply(202, r#"{"unexpected":true}"#)]);

    let error = client_for(&server)
        .create_refund(TRANSACTION_ID, &RefundRequest::new(key(REFUND_KEY)))
        .expect_err("no refund in the body");

    assert!(matches!(error, Error::Api { status: 202, .. }), "{error:?}");
}
