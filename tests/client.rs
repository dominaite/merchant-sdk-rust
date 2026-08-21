//! Client-level tests against a loopback mock server. No network, no credentials.

mod support;

use std::time::Duration;

use dominaite::{
    sign_request, CheckoutSessionRequest, Client, Error, RetryOptions, SignRequest, SESSIONS_PATH,
};
use support::{MockServer, Recorded, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TRANSACTION_ID: &str = "11111111-1111-4111-8111-111111111111";

const CHECKOUT: &str = r#"{"transactionId":"11111111-1111-4111-8111-111111111111","orderId":"dom_42","cashierKey":"ck_live","cashierToken":"ct_live","amount":2500,"currency":"EUR","expiresAt":"2026-08-20T12:00:00Z"}"#;

fn create_ok() -> Reply {
    Reply::enveloped(&format!(r#"{{"success":true,"checkout":{CHECKOUT}}}"#))
}

fn status_ok() -> Reply {
    Reply::enveloped(
        r#"{"transactionId":"11111111-1111-4111-8111-111111111111","orderId":"dom_42","orderReference":"order-1042","status":"succeeded","amount":2500,"currency":"EUR","createdAt":"2026-08-20T10:00:00Z"}"#,
    )
}

/// A refusal is an HTTP 200 whose unwrapped payload says success: false.
fn refusal(code: &str) -> Reply {
    Reply::enveloped(&format!(
        r#"{{"success":false,"errorCode":"{code}","errorMessage":"refused"}}"#
    ))
}

/// The base URL carries a path prefix (`/api` on dev), which must never end up in
/// the signed path.
fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(format!("{}/api", server.base_url()))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

fn request() -> CheckoutSessionRequest {
    CheckoutSessionRequest::new(2500, "EUR", "order-1042")
}

/// Recomputes the signature from what the server actually received, and asserts
/// it matches the X-Signature that came with it. The signed path is the canonical
/// path, never the `/api` prefix the request was sent to.
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
    assert_eq!(recorded.header("X-Api-Key-Id"), Some(KEY_ID));
}

#[test]
fn create_session_signs_exactly_what_it_sends() {
    let server = MockServer::start(vec![create_ok()]);
    let session = client_for(&server)
        .create_checkout_session(&request())
        .expect("session created");

    assert_eq!(session.transaction_id, TRANSACTION_ID);
    assert_eq!(session.cashier_key, "ck_live");
    assert_eq!(session.amount, 2500);

    let recorded = server.only_request();
    assert_eq!(recorded.method, "POST");
    assert_eq!(recorded.path, format!("/api{SESSIONS_PATH}"));
    assert_eq!(
        recorded.body,
        r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042"}"#
    );

    let key = recorded
        .header("Idempotency-Key")
        .expect("POST carries an Idempotency-Key")
        .to_string();
    assert_signature_matches(&recorded, SESSIONS_PATH, &key);
}

#[test]
fn get_status_sends_no_idempotency_key_and_signs_an_empty_one() {
    let server = MockServer::start(vec![status_ok()]);
    let status = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect("status read");

    assert!(status.is_paid());
    assert!(status.is_terminal());
    assert_eq!(status.order_reference.as_deref(), Some("order-1042"));

    let recorded = server.only_request();
    assert_eq!(recorded.method, "GET");
    assert_eq!(recorded.body, "");
    assert_eq!(
        recorded.header("Idempotency-Key"),
        None,
        "a GET must not send an Idempotency-Key header"
    );
    assert_signature_matches(&recorded, &format!("{SESSIONS_PATH}/{TRANSACTION_ID}"), "");
}

#[test]
fn ping_parses_the_flat_payload_and_signs_like_a_get() {
    let server = MockServer::start(vec![Reply::enveloped(
        r#"{"pong":true,"merchantId":"22222222-2222-4222-8222-222222222222","serverTime":"2026-08-20T10:00:00Z","serverUnixTime":1755302400,"clockSkewSeconds":-2}"#,
    )]);

    let ping = client_for(&server).ping().expect("ping answered");
    assert!(ping.pong);
    assert_eq!(ping.merchant_id, "22222222-2222-4222-8222-222222222222");
    assert_eq!(ping.clock_skew_seconds, -2);
    assert_eq!(ping.server_unix_time, Some(1755302400));

    let recorded = server.only_request();
    assert_eq!(recorded.path, "/api/merchant-api/ping");
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_signature_matches(&recorded, "/merchant-api/ping", "");
}

#[test]
fn create_unwraps_the_nested_shape_and_reads_unwrap_the_flat_one() {
    // Create: data.success + data.checkout. Reads: data with neither field.
    let created = MockServer::start(vec![create_ok()]);
    assert_eq!(
        client_for(&created)
            .create_checkout_session(&request())
            .expect("created")
            .transaction_id,
        TRANSACTION_ID
    );

    let read = MockServer::start(vec![status_ok()]);
    let status = client_for(&read).get_status(TRANSACTION_ID).expect("read");
    assert_eq!(status.status, "succeeded");
    assert_eq!(status.amount, 2500);
}

#[test]
fn an_unenveloped_response_is_read_as_the_payload_itself() {
    // Third shape: no gateway envelope at all, so there is no `data` to unwrap.
    let server = MockServer::start(vec![Reply::Json(
        200,
        format!(r#"{{"success":true,"checkout":{CHECKOUT}}}"#),
    )]);

    let session = client_for(&server)
        .create_checkout_session(&request())
        .expect("created");
    assert_eq!(session.transaction_id, TRANSACTION_ID);
}

#[test]
fn replay_codes_arrive_as_http_200_refusals() {
    for code in [
        "DUPLICATE_REQUEST",
        "ALREADY_PROCESSED",
        "PRIOR_ATTEMPT_FAILED",
        "IDEMPOTENCY_KEY_REUSED",
    ] {
        let server = MockServer::start(vec![refusal(code)]);
        let error = client_for(&server)
            .create_checkout_session(&request())
            .expect_err("refused");

        assert!(matches!(error, Error::Refusal { .. }), "{code}: {error}");
        assert_eq!(error.code(), Some(code));
        assert!(!error.is_retryable(), "{code} must never be retried");
    }
}

#[test]
fn every_401_code_becomes_an_auth_error() {
    for code in [
        "INVALID_API_KEY",
        "INVALID_SIGNATURE",
        "TIMESTAMP_OUT_OF_RANGE",
        "IP_NOT_ALLOWED",
    ] {
        let server = MockServer::start(vec![Reply::error_envelope(401, code, "nope")]);
        let error = client_for(&server)
            .create_checkout_session(&request())
            .expect_err("rejected");

        assert!(matches!(error, Error::Auth { .. }), "{code}: {error}");
        assert_eq!(error.code(), Some(code));
        assert!(!error.is_retryable());
    }
}

#[test]
fn a_403_is_an_auth_error_too() {
    let server = MockServer::start(vec![Reply::error_envelope(403, "IP_NOT_ALLOWED", "nope")]);
    let error = client_for(&server)
        .create_checkout_session(&request())
        .expect_err("rejected");

    assert_eq!(error.code(), Some("IP_NOT_ALLOWED"));
    assert!(matches!(error, Error::Auth { .. }));
}

#[test]
fn a_422_replay_with_a_different_body_is_an_api_error() {
    let server = MockServer::start(vec![Reply::error_envelope(
        422,
        "IDEMPOTENCY_KEY_REUSED",
        "same key, different body",
    )]);
    let error = client_for(&server)
        .create_checkout_session(&request())
        .expect_err("rejected");

    assert!(matches!(error, Error::Api { .. }), "{error}");
    assert_eq!(error.http_status(), Some(422));
    assert!(!error.is_retryable());
}

#[test]
fn an_unknown_transaction_id_is_a_404_api_error() {
    let server = MockServer::start(vec![Reply::error_envelope(404, "NOT_FOUND", "no such id")]);
    let error = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect_err("not found");

    assert_eq!(error.http_status(), Some(404));
}

#[test]
fn a_503_is_a_transport_error_not_a_refusal() {
    let server = MockServer::start(vec![Reply::error_envelope(
        503,
        "MERCHANT_API_UNAVAILABLE",
        "try later",
    )]);
    let error = client_for(&server)
        .create_checkout_session(&request())
        .expect_err("unavailable");

    assert!(matches!(error, Error::Transport { .. }), "{error}");
    assert!(error.is_retryable());
}

#[test]
fn a_dropped_connection_is_a_transport_error() {
    let server = MockServer::start(vec![Reply::HangUp]);
    let error = client_for(&server)
        .create_checkout_session(&request())
        .expect_err("hung up");

    assert!(matches!(error, Error::Transport { .. }), "{error}");
    assert!(error.is_retryable());
}

#[test]
fn retry_reuses_one_idempotency_key_across_attempts() {
    let server = MockServer::start(vec![
        Reply::error_envelope(503, "MERCHANT_API_UNAVAILABLE", "try later"),
        create_ok(),
    ]);

    let session = client_for(&server)
        .create_checkout_session_with_retry(
            &request(),
            RetryOptions {
                attempts: 3,
                base_delay: Duration::from_millis(0),
            },
        )
        .expect("second attempt succeeds");
    assert_eq!(session.transaction_id, TRANSACTION_ID);

    let requests = server.requests();
    assert_eq!(requests.len(), 2, "the first attempt must be retried once");

    let first = requests[0].header("Idempotency-Key").expect("key sent");
    let second = requests[1].header("Idempotency-Key").expect("key sent");
    assert_eq!(
        first, second,
        "a retry must reuse the key, or a landed first attempt becomes a second payment"
    );

    // Every attempt is signed independently, over its own timestamp.
    for recorded in &requests {
        assert_signature_matches(recorded, SESSIONS_PATH, first);
    }
}

#[test]
fn retry_gives_up_and_returns_the_transport_error() {
    let server = MockServer::start(vec![Reply::error_envelope(503, "X", "down")]);
    let error = client_for(&server)
        .create_checkout_session_with_retry(
            &request(),
            RetryOptions {
                attempts: 2,
                base_delay: Duration::from_millis(0),
            },
        )
        .expect_err("still down");

    assert!(matches!(error, Error::Transport { .. }));
    assert_eq!(server.requests().len(), 2);
}

/// Without the transaction id on the refusal, the documented recovery - read it
/// back with `get_status` - is unreachable from the error, leaving a second
/// payment as the caller's only option.
#[test]
fn a_replay_refusal_carries_the_transaction_id_for_recovery() {
    let transaction_id = "11111111-2222-4333-8444-555555555555";
    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"success":false,"transactionId":"{transaction_id}","errorCode":"DUPLICATE_REQUEST","errorMessage":"already open"}}"#
    ))]);

    let error = client_for(&server)
        .create_checkout_session(&request())
        .expect_err("refused");

    match error {
        Error::Refusal {
            code,
            transaction_id: Some(id),
            ..
        } => {
            assert_eq!(code, "DUPLICATE_REQUEST");
            assert_eq!(id, transaction_id);
        }
        other => panic!("expected a refusal carrying the transaction id, got {other}"),
    }
}

/// The concurrent-race `DUPLICATE_REQUEST` knows the key is taken, but not yet by
/// which row - so the id stays optional.
#[test]
fn a_refusal_without_a_transaction_id_leaves_it_none() {
    let server = MockServer::start(vec![refusal("DUPLICATE_REQUEST")]);

    let error = client_for(&server)
        .create_checkout_session(&request())
        .expect_err("refused");

    assert!(matches!(
        error,
        Error::Refusal {
            transaction_id: None,
            ..
        }
    ));
}

#[test]
fn retry_never_repeats_a_refusal_or_an_auth_failure() {
    let options = RetryOptions {
        attempts: 3,
        base_delay: Duration::from_millis(0),
    };

    let refused = MockServer::start(vec![refusal("DUPLICATE_REQUEST")]);
    let error = client_for(&refused)
        .create_checkout_session_with_retry(&request(), options)
        .expect_err("refused");
    assert!(matches!(error, Error::Refusal { .. }));
    assert_eq!(refused.requests().len(), 1);

    let rejected = MockServer::start(vec![Reply::error_envelope(401, "INVALID_SIGNATURE", "no")]);
    let error = client_for(&rejected)
        .create_checkout_session_with_retry(&request(), options)
        .expect_err("rejected");
    assert!(matches!(error, Error::Auth { .. }));
    assert_eq!(rejected.requests().len(), 1);
}

#[test]
fn a_pinned_idempotency_key_is_the_one_that_gets_sent() {
    let server = MockServer::start(vec![create_ok()]);
    let key = "00000000-0000-4000-8000-000000000001";

    client_for(&server)
        .create_checkout_session(&request().idempotency_key(key))
        .expect("created");

    let recorded = server.only_request();
    assert_eq!(recorded.header("Idempotency-Key"), Some(key));
    assert_signature_matches(&recorded, SESSIONS_PATH, key);
}

#[test]
fn bad_arguments_are_rejected_before_anything_is_sent() {
    let server = MockServer::start(vec![create_ok()]);
    let client = client_for(&server);

    for (label, bad) in [
        (
            "zero amount",
            CheckoutSessionRequest::new(0, "EUR", "order-1"),
        ),
        (
            "negative amount",
            CheckoutSessionRequest::new(-500, "EUR", "order-1"),
        ),
        (
            "missing currency",
            CheckoutSessionRequest::new(2500, "", "order-1"),
        ),
        (
            "missing order reference",
            CheckoutSessionRequest::new(2500, "EUR", ""),
        ),
        (
            "over-long order reference",
            CheckoutSessionRequest::new(2500, "EUR", "x".repeat(101)),
        ),
    ] {
        let error = client
            .create_checkout_session(&bad)
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
fn a_malformed_transaction_id_never_reaches_the_network() {
    let server = MockServer::start(vec![status_ok()]);
    let error = client_for(&server)
        .get_status("not-a-uuid")
        .expect_err("rejected");

    assert!(matches!(error, Error::Validation { .. }), "{error}");
    assert!(server.requests().is_empty());
}

#[test]
fn credentials_with_the_wrong_prefix_are_rejected_at_construction() {
    assert!(matches!(
        Client::new("nope", SECRET).expect_err("bad key id"),
        Error::Validation { .. }
    ));
    assert!(matches!(
        Client::new(KEY_ID, "nope").expect_err("bad secret"),
        Error::Validation { .. }
    ));
}

/// Debug output ends up in logs, panics, and crash reporters. The secret must not
/// travel with it.
#[test]
fn debug_output_never_carries_the_secret() {
    let builder = Client::builder(KEY_ID, SECRET);
    let client = Client::builder(KEY_ID, SECRET).build().expect("built");
    let signing = SignRequest {
        secret: SECRET,
        timestamp: "1755302400",
        method: "POST",
        path: SESSIONS_PATH,
        idempotency_key: "00000000-0000-4000-8000-000000000001",
        body: "{}",
    };

    for (label, printed) in [
        ("ClientBuilder", format!("{builder:?}")),
        ("Client", format!("{client:?}")),
        ("SignRequest", format!("{signing:?}")),
    ] {
        assert!(
            !printed.contains(SECRET),
            "{label} debug output leaks the secret: {printed}"
        );
        assert!(
            printed.contains("redacted"),
            "{label} debug output does not mark the secret as redacted: {printed}"
        );
        // The key id is not a secret, and losing it would make debug output useless.
        assert!(printed.contains(KEY_ID) || label == "SignRequest", "{label}");
    }
}

#[test]
fn an_empty_base_url_keeps_the_production_default() {
    let client = Client::builder(KEY_ID, SECRET)
        .base_url("")
        .base_url("   ")
        .build()
        .expect("built");

    assert_eq!(client.base_url(), dominaite::DEFAULT_BASE_URL);
}

/// Following a 3xx would hand the signed headers to the redirect target and read
/// its JSON as an authentic answer, which is a forged session. The gateway never
/// redirects, so a 3xx stops the call.
#[test]
fn a_redirect_is_never_followed() {
    for status in [302u16, 307] {
        let target = MockServer::start(vec![create_ok()]);
        let server = MockServer::start(vec![Reply::Redirect(
            status,
            format!("{}{SESSIONS_PATH}", target.base_url()),
        )]);

        let error = client_for(&server)
            .create_checkout_session(&request())
            .expect_err("a redirect must not be followed");

        assert!(matches!(error, Error::Api { .. }), "{status}: {error}");
        assert_eq!(error.http_status(), Some(status));
        assert!(
            error.to_string().contains("redirect"),
            "{status}: {error} does not name the redirect"
        );
        assert!(
            target.requests().is_empty(),
            "{status}: the redirect target was hit"
        );
        assert_eq!(server.requests().len(), 1);
    }
}

/// The redirect policy has to survive a caller's own agent: ureq's default is ten
/// redirects, and an agent supplied through `ClientBuilder::agent` never went
/// through the SDK's config. The SDK forces the policy per request instead of
/// trusting the agent it was handed.
#[test]
fn a_caller_supplied_agent_cannot_re_enable_redirects() {
    let attacker = MockServer::start(vec![create_ok()]);
    let server = MockServer::start(vec![Reply::Redirect(
        302,
        format!("{}{SESSIONS_PATH}", attacker.base_url()),
    )]);

    // Everything ureq defaults to, including max_redirects = 10.
    let client = Client::builder(KEY_ID, SECRET)
        .base_url(format!("{}/api", server.base_url()))
        .agent(ureq::Agent::new_with_defaults())
        .build()
        .expect("valid credentials");

    let error = client
        .create_checkout_session(&request())
        .expect_err("a redirect must not be followed");

    assert!(matches!(error, Error::Api { .. }), "{error}");
    assert_eq!(error.http_status(), Some(302));
    assert!(
        attacker.requests().is_empty(),
        "the signed headers reached the redirect target"
    );
}

#[test]
fn a_redirect_is_not_retried() {
    let target = MockServer::start(vec![create_ok()]);
    let server = MockServer::start(vec![Reply::Redirect(
        302,
        format!("{}{SESSIONS_PATH}", target.base_url()),
    )]);

    let error = client_for(&server)
        .create_checkout_session_with_retry(
            &request(),
            RetryOptions {
                attempts: 3,
                base_delay: Duration::from_millis(0),
            },
        )
        .expect_err("a redirect must not be followed");

    assert!(!error.is_retryable(), "{error}");
    assert_eq!(server.requests().len(), 1, "a redirect must not be retried");
    assert!(target.requests().is_empty());
}

#[test]
fn a_non_json_response_is_an_api_error() {
    let server = MockServer::start(vec![Reply::Json(
        200,
        "<html>gateway timeout</html>".into(),
    )]);
    let error = client_for(&server)
        .create_checkout_session(&request())
        .expect_err("not JSON");

    assert!(matches!(error, Error::Api { .. }), "{error}");
}

#[test]
fn an_unknown_status_keeps_the_payment_open() {
    let server = MockServer::start(vec![Reply::enveloped(
        r#"{"transactionId":"11111111-1111-4111-8111-111111111111","status":"awaiting_something_new","amount":2500,"currency":"EUR"}"#,
    )]);

    let status = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect("status read");

    assert!(!status.is_paid());
    assert!(
        !status.is_terminal(),
        "an unrecognised status must keep the caller polling, not close the order"
    );
}
