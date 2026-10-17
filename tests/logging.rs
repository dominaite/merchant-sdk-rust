//! The warning hook, the `Dominaite-Sdk-Notice` header and the debug log. What
//! the hooks receive is the contract; none of it may carry a secret.

// The deprecated create_checkout_session_with_retry keeps its tests until it
// is removed.
#![allow(deprecated)]

#[allow(dead_code)]
mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dominaite::{CheckoutSessionRequest, Client, ClientBuilder, IdempotencyKey, RetryOptions};
use support::{MockServer, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const SESSION_KEY: &str = "00000000-0000-4000-8000-000000000001";
const CASHIER_TOKEN: &str = "ct_live_secret_token";
const NOTICE: &str =
    r#"deprecated; current="0.7.0"; sunset="2027-01-31"; info="https://example.test/upgrade""#;

fn ping_body(skew: i64) -> String {
    format!(
        r#"{{"success":true,"data":{{"pong":true,"merchantId":"m_1","serverTime":"2026-10-17T10:00:00Z","serverUnixTime":1792231200,"clockSkewSeconds":{skew}}}}}"#
    )
}

fn create_body() -> String {
    format!(
        r#"{{"success":true,"data":{{"success":true,"checkout":{{"transactionId":"11111111-1111-4111-8111-111111111111","orderId":"dom_42","cashierKey":"ck_live","cashierToken":"{CASHIER_TOKEN}","amount":2500,"currency":"EUR","expiresAt":"2026-08-20T12:00:00Z"}}}}}}"#
    )
}

fn with_headers(status: u16, body: String, headers: &[(&str, &str)]) -> Reply {
    Reply::JsonWithHeaders(
        status,
        body,
        headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    )
}

fn request() -> CheckoutSessionRequest {
    CheckoutSessionRequest::new(
        2500,
        "EUR",
        "order-1042",
        IdempotencyKey::new(SESSION_KEY).unwrap(),
    )
}

/// A sink that records every line it gets.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    fn sink(&self) -> impl Fn(&str) + Send + Sync + 'static {
        let lines = Arc::clone(&self.0);
        move |line: &str| lines.lock().unwrap().push(line.to_string())
    }

    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

fn builder(server: &MockServer) -> ClientBuilder {
    Client::builder(KEY_ID, SECRET)
        .base_url(server.base_url())
        .timeout(Duration::from_secs(5))
}

#[test]
fn a_notice_warns_once_per_distinct_value() {
    let other = r#"deprecated; current="0.8.0""#;
    let server = MockServer::start(vec![
        with_headers(200, ping_body(0), &[("Dominaite-Sdk-Notice", NOTICE)]),
        with_headers(200, ping_body(0), &[("dominaite-sdk-notice", NOTICE)]),
        with_headers(200, ping_body(0), &[("Dominaite-Sdk-Notice", NOTICE)]),
        with_headers(200, ping_body(0), &[("Dominaite-Sdk-Notice", other)]),
    ]);
    let warnings = Lines::default();
    let client = builder(&server)
        .on_warning(warnings.sink())
        .build()
        .unwrap();

    for _ in 0..4 {
        client.ping().unwrap();
    }

    assert_eq!(
        warnings.all(),
        vec![
            format!(
                "dominaite-rust {} is deprecated; upgrade to 0.7.0 (sunset 2027-01-31). https://example.test/upgrade",
                dominaite::VERSION
            ),
            format!(
                "dominaite-rust {} is deprecated; upgrade to 0.8.0.",
                dominaite::VERSION
            ),
        ]
    );
}

#[test]
fn a_notice_on_an_error_response_also_warns() {
    let server = MockServer::start(vec![with_headers(
        404,
        r#"{"success":false,"data":null,"error":{"code":"NOT_FOUND","message":"no such session"}}"#
            .to_string(),
        &[("Dominaite-Sdk-Notice", NOTICE)],
    )]);
    let warnings = Lines::default();
    let client = builder(&server)
        .on_warning(warnings.sink())
        .build()
        .unwrap();

    assert!(client
        .get_status("11111111-1111-4111-8111-111111111111")
        .is_err());
    assert_eq!(warnings.all().len(), 1);
    assert!(warnings.all()[0].contains("is deprecated; upgrade to 0.7.0"));
}

#[test]
fn a_notice_of_another_kind_is_passed_through() {
    let server = MockServer::start(vec![with_headers(
        200,
        ping_body(0),
        &[("Dominaite-Sdk-Notice", "maintenance; at=\"2026-11-01\"")],
    )]);
    let warnings = Lines::default();
    let client = builder(&server)
        .on_warning(warnings.sink())
        .build()
        .unwrap();
    client.ping().unwrap();

    assert_eq!(
        warnings.all(),
        vec![format!(
            "dominaite-rust {}: maintenance; at=\"2026-11-01\"",
            dominaite::VERSION
        )]
    );
}

#[test]
fn no_notice_header_means_no_warning() {
    let server = MockServer::start(vec![Reply::Json(200, ping_body(0))]);
    let warnings = Lines::default();
    let client = builder(&server)
        .on_warning(warnings.sink())
        .build()
        .unwrap();
    client.ping().unwrap();
    client.ping().unwrap();

    assert!(warnings.all().is_empty());
}

#[test]
fn the_debug_log_is_off_by_default() {
    let server = MockServer::start(vec![with_headers(
        200,
        ping_body(0),
        &[("Dominaite-Request-Id", "req-123")],
    )]);
    let warnings = Lines::default();
    let client = builder(&server)
        .on_warning(warnings.sink())
        .build()
        .unwrap();
    client.ping().unwrap();

    assert!(
        warnings.all().is_empty(),
        "debug lines never reach the warning hook"
    );
}

#[test]
fn the_debug_log_writes_one_line_per_request() {
    let server = MockServer::start(vec![
        with_headers(201, create_body(), &[("Dominaite-Request-Id", "req-123")]),
        Reply::Json(200, ping_body(0)),
    ]);
    let debug = Lines::default();
    let client = builder(&server).on_debug(debug.sink()).build().unwrap();

    client.create_checkout_session(&request()).unwrap();
    client.ping().unwrap();

    let lines = debug.all();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let (head, tail) = lines[0]
        .split_once("ms ")
        .unwrap_or_else(|| panic!("no elapsed ms: {}", lines[0]));
    assert!(
        head.starts_with("dominaite-rust: POST /merchant-api/checkout/sessions 201 "),
        "{head}"
    );
    assert!(
        head.rsplit(' ').next().unwrap().parse::<u64>().is_ok(),
        "{head}"
    );
    assert_eq!(
        tail,
        format!("request_id=req-123 idempotency_key={SESSION_KEY}")
    );

    assert!(
        lines[1].starts_with("dominaite-rust: GET /merchant-api/ping 200 "),
        "{}",
        lines[1]
    );
    assert!(
        lines[1].ends_with("ms"),
        "no request id, no key: {}",
        lines[1]
    );
}

#[test]
fn the_debug_log_reports_a_transport_failure() {
    let server = MockServer::start(vec![Reply::HangUp]);
    let debug = Lines::default();
    let client = builder(&server)
        .on_debug(debug.sink())
        .max_retries(0)
        .build()
        .unwrap();
    assert!(client.ping().is_err());

    let lines = debug.all();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].starts_with("dominaite-rust: GET /merchant-api/ping error "),
        "{}",
        lines[0]
    );
}

#[test]
fn no_line_carries_the_secret_the_signature_a_token_or_a_body() {
    let server = MockServer::start(vec![
        with_headers(
            201,
            create_body(),
            &[
                ("Dominaite-Sdk-Notice", NOTICE),
                ("Dominaite-Request-Id", "req-1"),
            ],
        ),
        with_headers(
            400,
            r#"{"success":false,"data":null,"error":{"code":"VALIDATION_ERROR","message":"body fragment here"}}"#
                .to_string(),
            &[("Dominaite-Request-Id", "req-2")],
        ),
        Reply::Json(503, r#"{"success":false,"error":{"code":"UPSTREAM","message":"body fragment here"}}"#.to_string()),
        with_headers(201, create_body(), &[]),
        Reply::Json(200, ping_body(120)),
    ]);
    let lines = Lines::default();
    let client = builder(&server)
        .on_warning(lines.sink())
        .on_debug(lines.sink())
        .build()
        .unwrap();

    client.create_checkout_session(&request()).unwrap();
    assert!(client.create_checkout_session(&request()).is_err());
    client
        .create_checkout_session_with_retry(
            &request(),
            RetryOptions {
                attempts: 2,
                base_delay: Duration::from_millis(1),
            },
        )
        .unwrap();
    client.ping().unwrap();

    let signatures: Vec<String> = server
        .requests()
        .iter()
        .filter_map(|sent| sent.header("x-signature").map(str::to_string))
        .collect();
    let all = lines.all();
    assert!(all.len() >= 7, "debug and warning lines expected: {all:?}");
    for line in &all {
        assert!(!line.contains(SECRET), "secret in {line}");
        assert!(!line.contains(CASHIER_TOKEN), "cashier token in {line}");
        assert!(!line.contains("body fragment"), "body in {line}");
        assert!(!line.contains("2500"), "request body in {line}");
        for signature in &signatures {
            assert!(!line.contains(signature.as_str()), "signature in {line}");
        }
    }
}

#[test]
fn a_panicking_hook_does_not_fail_the_request() {
    let server = MockServer::start(vec![with_headers(
        200,
        ping_body(0),
        &[("Dominaite-Sdk-Notice", NOTICE)],
    )]);
    let client = builder(&server)
        .on_warning(|_| panic!("the logger is broken"))
        .on_debug(|_| panic!("the logger is broken"))
        .build()
        .unwrap();

    assert!(client.ping().is_ok());
}

#[test]
fn a_retry_is_warned_about() {
    let server = MockServer::start(vec![
        Reply::Html(503, "<html>unavailable</html>".to_string()),
        with_headers(201, create_body(), &[]),
    ]);
    let warnings = Lines::default();
    let client = builder(&server)
        .on_warning(warnings.sink())
        .build()
        .unwrap();
    client
        .create_checkout_session_with_retry(
            &request(),
            RetryOptions {
                attempts: 3,
                base_delay: Duration::from_millis(1),
            },
        )
        .unwrap();

    assert_eq!(
        warnings.all(),
        vec!["dominaite-rust: retrying POST /merchant-api/checkout/sessions after HTTP 503 (attempt 2 of 3)".to_string()]
    );
}

#[test]
fn every_attempt_gets_its_own_debug_line() {
    let server = MockServer::start(vec![Reply::HangUp, with_headers(200, ping_body(0), &[])]);
    let debug = Lines::default();
    let client = builder(&server)
        .on_debug(debug.sink())
        .retry_sleep(|_| {})
        .build()
        .unwrap();
    client.ping().unwrap();

    let lines = debug.all();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].starts_with("dominaite-rust: GET /merchant-api/ping error "));
    assert!(lines[1].starts_with("dominaite-rust: GET /merchant-api/ping 200 "));
}

#[test]
fn a_clock_skew_over_60_seconds_is_warned_about() {
    for (skew, warned) in [
        (61, true),
        (-61, true),
        (60, false),
        (-60, false),
        (0, false),
    ] {
        let server = MockServer::start(vec![Reply::Json(200, ping_body(skew))]);
        let warnings = Lines::default();
        let client = builder(&server)
            .on_warning(warnings.sink())
            .build()
            .unwrap();
        client.ping().unwrap();

        if warned {
            assert_eq!(
                warnings.all(),
                vec![format!(
                    "dominaite-rust: clock skew is {skew} s; requests fail at 300 s. Sync this server's clock."
                )]
            );
        } else {
            assert!(warnings.all().is_empty(), "skew {skew} must not warn");
        }
    }
}

#[test]
fn the_debug_output_of_a_client_with_hooks_names_no_secret() {
    let client = Client::builder(KEY_ID, SECRET)
        .base_url(dominaite::SANDBOX_BASE_URL)
        .on_warning(|_| {})
        .on_debug(|_| {})
        .build()
        .unwrap();
    let printed = format!("{client:?}");
    assert!(!printed.contains(SECRET));
    assert!(printed.contains("on_warning: Some(\"<fn>\")"), "{printed}");
}

#[test]
fn a_client_with_hooks_is_still_send_sync_and_clone() {
    fn assert_traits<T: Send + Sync + Clone>() {}
    assert_traits::<Client>();
    assert_traits::<ClientBuilder>();
}
