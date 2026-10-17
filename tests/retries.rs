//! The client-level retry policy: which calls it replays, which answers it
//! retries, what every attempt sends, and how long it waits in between.

#[allow(dead_code)]
mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dominaite::{
    sign_request, ChargeRequest, CheckoutSessionRequest, Client, ClientBuilder, Error,
    IdempotencyKey, PlanUpdate, RefundRequest, SignRequest, WebhookEndpointRequest,
};
use support::{MockServer, Recorded, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TRANSACTION_ID: &str = "11111111-1111-4111-8111-111111111111";
const PAYMENT_METHOD_ID: &str = "pm_0123456789abcdef0123456789abcdef";
const PLAN_ID: &str = "plan_0123456789ABCDEF0123456789abcdef";
const WEBHOOK_ID: &str = "5d0c6a52-8f1e-4b7a-9c3d-2e1f0a9b8c7d";
const CHECKOUT: &str = r#"{"success":true,"checkout":{"transactionId":"11111111-1111-4111-8111-111111111111","orderId":"dom_42","cashierKey":"ck_live","cashierToken":"ct_live","amount":2500,"currency":"EUR","expiresAt":"2026-08-20T12:00:00Z"}}"#;
const CHARGE: &str = r#"{"chargeId":"ch_33333333333343338333333333333333","status":"succeeded","transactionId":"33333333-3333-4333-8333-333333333333"}"#;
const REFUND: &str = r#"{"refundId":"re_7c1e9a2b4d6f48a0b3c5d7e9f1a2b3c4","transactionId":"11111111-1111-4111-8111-111111111111","status":"pending","amount":500,"currency":"EUR"}"#;
const PLAN: &str = r#"{"id":"plan_0123456789ABCDEF0123456789abcdef","name":"Pro","amount":1990,"currency":"EUR","intervalUnit":"month","intervalCount":1,"trialDays":14,"status":"active","createdAt":"2026-10-11T09:00:00Z"}"#;

/// Every wait the client asked for, in order.
#[derive(Clone, Default)]
struct Waits(Arc<Mutex<Vec<Duration>>>);

impl Waits {
    fn all(&self) -> Vec<Duration> {
        self.0.lock().unwrap().clone()
    }
}

fn builder(server: &MockServer) -> ClientBuilder {
    Client::builder(KEY_ID, SECRET)
        .base_url(server.base_url())
        .timeout(Duration::from_secs(5))
}

/// A client on the default policy that records its waits instead of sleeping.
fn client_for(server: &MockServer) -> (Client, Waits) {
    let waits = Waits::default();
    let recorder = waits.clone();
    let client = builder(server)
        .retry_sleep(move |delay| recorder.0.lock().unwrap().push(delay))
        .build()
        .expect("valid credentials");
    (client, waits)
}

/// Every warning the client handed to `on_warning`, in order.
#[derive(Clone, Default)]
struct Warnings(Arc<Mutex<Vec<String>>>);

impl Warnings {
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// A client on `max_retries` that records its waits and its warnings.
fn warning_client(server: &MockServer, max_retries: u32) -> (Client, Warnings) {
    let warnings = Warnings::default();
    let recorder = warnings.clone();
    let client = builder(server)
        .max_retries(max_retries)
        .retry_sleep(|_| {})
        .on_warning(move |line| recorder.0.lock().unwrap().push(line.to_string()))
        .build()
        .expect("valid credentials");
    (client, warnings)
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).expect("a valid idempotency key")
}

fn charge_request() -> ChargeRequest {
    ChargeRequest::new(2500, "EUR", "order-1043", key("charge-order-1043"))
}

fn unavailable() -> Reply {
    Reply::error_envelope(503, "MERCHANT_API_UNAVAILABLE", "Try later.")
}

/// Asserts that every attempt carried the same key, path and body, and that
/// each signature is the one for that same input at the attempt's timestamp.
fn assert_same_signed_input(requests: &[Recorded], method: &str) {
    let first = &requests[0];
    let key = first.headers.get("idempotency-key").expect("a key");
    for request in requests {
        assert_eq!(request.method, method);
        assert_eq!(request.path, first.path);
        assert_eq!(request.body, first.body);
        assert_eq!(request.headers.get("idempotency-key"), Some(key));
        let expected = sign_request(SignRequest {
            secret: SECRET,
            timestamp: &request.headers["x-timestamp"],
            method,
            path: &request.path,
            idempotency_key: key,
            body: &request.body,
        });
        assert_eq!(request.headers["x-signature"], expected);
    }
}

#[test]
fn a_charge_retried_after_a_transport_failure_reuses_its_key() {
    let server = MockServer::start(vec![Reply::HangUp, Reply::Json(201, ok(CHARGE))]);
    let (client, waits) = client_for(&server);
    let charge = client
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect("the retry lands");
    assert_eq!(charge.status, "succeeded");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_same_signed_input(&requests, "POST");
    assert_eq!(requests[0].headers["idempotency-key"], "charge-order-1043");
    assert_eq!(waits.all().len(), 1);
}

#[test]
fn a_refund_retried_after_a_transport_failure_reuses_its_key() {
    let server = MockServer::start(vec![Reply::HangUp, Reply::Json(202, ok(REFUND))]);
    let (client, _) = client_for(&server);
    let request = RefundRequest::new(key("refund-1")).amount(500);
    let refund = client
        .create_refund(TRANSACTION_ID, &request)
        .expect("the retry lands");
    assert_eq!(refund.status, "pending");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_same_signed_input(&requests, "POST");
    assert_eq!(requests[0].headers["idempotency-key"], "refund-1");
}

#[test]
fn the_default_sends_a_call_at_most_three_times() {
    let server = MockServer::start(vec![unavailable()]);
    let (client, waits) = client_for(&server);
    let error = client.get_status(TRANSACTION_ID).expect_err("still down");
    assert!(matches!(error, Error::Transport { .. }), "{error:?}");
    assert_eq!(error.code(), Some("MERCHANT_API_UNAVAILABLE"));
    assert_eq!(server.requests().len(), 3);
    assert_eq!(waits.all().len(), 2);
}

#[test]
fn max_retries_zero_sends_once() {
    let server = MockServer::start(vec![Reply::HangUp, Reply::Json(201, ok(CHARGE))]);
    let client = builder(&server)
        .max_retries(0)
        .retry_sleep(|_| panic!("no retry, no wait"))
        .build()
        .unwrap();
    let error = client
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("not retried");
    assert!(matches!(error, Error::Transport { .. }), "{error:?}");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_429_is_never_retried_and_keeps_retry_after() {
    let server = MockServer::start(vec![
        Reply::RateLimited(Some("30".to_string())),
        Reply::Json(201, ok(CHARGE)),
    ]);
    let (client, waits) = client_for(&server);
    let error = client
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("rate limited");
    match error {
        Error::RateLimited {
            retry_after_seconds,
            ..
        } => assert_eq!(retry_after_seconds, Some(30)),
        other => panic!("expected a rate limit, got {other:?}"),
    }
    assert_eq!(server.requests().len(), 1);
    assert!(waits.all().is_empty());
}

#[test]
fn processing_unavailable_is_retried_in_both_forms() {
    let refusal = r#"{"success":false,"errorCode":"PAYMENT_PROCESSING_UNAVAILABLE","errorMessage":"Card payments are off."}"#;
    let server = MockServer::start(vec![
        Reply::enveloped(refusal),
        Reply::error_envelope(
            503,
            "PAYMENT_PROCESSING_UNAVAILABLE",
            "Card payments are off.",
        ),
        Reply::enveloped(CHECKOUT),
    ]);
    let (client, _) = client_for(&server);
    let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key("order-1042"));
    let session = client
        .create_checkout_session(&request)
        .expect("the third attempt lands");
    assert_eq!(session.transaction_id, TRANSACTION_ID);
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert_same_signed_input(&requests, "POST");
}

#[test]
fn a_charge_with_an_unknown_or_failed_outcome_is_not_retried() {
    let pending = CHARGE.replace("succeeded", "pending");
    for code in ["CHARGE_OUTCOME_UNKNOWN", "CHARGE_FAILED"] {
        let body = format!(
            r#"{{"success":false,"data":{pending},"error":{{"code":"{code}","message":"No."}}}}"#
        );
        let server = MockServer::start(vec![Reply::Json(502, body), Reply::Json(201, ok(CHARGE))]);
        let (client, _) = client_for(&server);
        let error = client
            .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
            .expect_err(code);
        assert_eq!(error.code(), Some(code));
        assert_eq!(server.requests().len(), 1, "{code}");
    }
}

#[test]
fn a_charge_answered_charges_disabled_is_sent_once() {
    let server = MockServer::start(vec![
        Reply::error_envelope(
            503,
            "PAYMENT_METHOD_CHARGES_DISABLED",
            "Charges of stored payment methods are not enabled.",
        ),
        Reply::Json(201, ok(CHARGE)),
    ]);
    let (client, waits) = client_for(&server);
    let error = client
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("charges are off");
    assert!(matches!(error, Error::Charge { .. }), "{error:?}");
    assert_eq!(error.code(), Some("PAYMENT_METHOD_CHARGES_DISABLED"));
    assert_eq!(server.requests().len(), 1);
    assert!(waits.all().is_empty());
}

#[test]
fn a_refusal_or_a_4xx_is_not_retried() {
    let refusal =
        r#"{"success":false,"errorCode":"DUPLICATE_REQUEST","errorMessage":"Already open."}"#;
    let server = MockServer::start(vec![Reply::enveloped(refusal)]);
    let (client, _) = client_for(&server);
    let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key("order-1042"));
    assert!(client.create_checkout_session(&request).is_err());
    assert_eq!(server.requests().len(), 1);

    let server = MockServer::start(vec![Reply::error_envelope(404, "NOT_FOUND", "No.")]);
    let (client, _) = client_for(&server);
    assert!(client.get_plan(PLAN_ID).is_err());
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn webhook_endpoint_create_patch_and_delete_are_never_retried() {
    let server = MockServer::start(vec![unavailable()]);
    let (client, _) = client_for(&server);
    let request = WebhookEndpointRequest::new(
        "https://shop.example/hooks/dominaite",
        ["payment.succeeded"],
        key("webhook-1"),
    );
    assert!(client.create_webhook_endpoint(&request).is_err());
    assert_eq!(server.requests().len(), 1, "webhook endpoint create");

    let server = MockServer::start(vec![unavailable()]);
    let (client, _) = client_for(&server);
    assert!(client
        .update_plan(PLAN_ID, &PlanUpdate::new().name("Pro"))
        .is_err());
    assert_eq!(server.requests().len(), 1, "PATCH");

    let server = MockServer::start(vec![unavailable()]);
    let (client, _) = client_for(&server);
    assert!(client.delete_webhook_endpoint(WEBHOOK_ID).is_err());
    assert_eq!(server.requests().len(), 1, "DELETE");
}

#[test]
fn a_get_is_retried_after_a_5xx() {
    let server = MockServer::start(vec![unavailable(), Reply::enveloped(PLAN)]);
    let (client, _) = client_for(&server);
    let plan = client.get_plan(PLAN_ID).expect("the retry lands");
    assert_eq!(plan.id, PLAN_ID);
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn every_wait_is_full_jitter_under_a_doubling_ceiling_capped_at_8_seconds() {
    // Seven retries: ceilings 0.5, 1, 2, 4, 8 (cap), 8, 8 seconds.
    let ceilings = [0.5, 1.0, 2.0, 4.0, 8.0, 8.0, 8.0];
    for random in [0.0, 0.5, 0.999] {
        let server = MockServer::start(vec![Reply::HangUp]);
        let waits = Waits::default();
        let recorder = waits.clone();
        let client = builder(&server)
            .max_retries(7)
            .retry_random(move || random)
            .retry_sleep(move |delay| recorder.0.lock().unwrap().push(delay))
            .build()
            .unwrap();
        assert!(client.get_status(TRANSACTION_ID).is_err());

        let waits = waits.all();
        assert_eq!(waits.len(), ceilings.len(), "random {random}");
        for (wait, ceiling) in waits.iter().zip(ceilings) {
            let expected = ceiling * random;
            assert!(
                (wait.as_secs_f64() - expected).abs() < 1e-6,
                "random {random}: waited {wait:?}, expected {expected}s"
            );
            assert!(wait.as_secs_f64() <= 8.0);
        }
    }
}

#[test]
fn the_default_jitter_stays_inside_the_ceiling() {
    let server = MockServer::start(vec![Reply::HangUp]);
    let (client, waits) = client_for(&server);
    assert!(client.get_status(TRANSACTION_ID).is_err());
    let waits = waits.all();
    assert_eq!(waits.len(), 2);
    assert!(waits[0] <= Duration::from_millis(500), "{waits:?}");
    assert!(waits[1] <= Duration::from_secs(1), "{waits:?}");
}

fn ok(data: &str) -> String {
    format!(r#"{{"success":true,"data":{data},"metadata":{{}}}}"#)
}

#[test]
fn the_largest_max_retries_counts_its_attempts_without_overflow() {
    let server = MockServer::start(vec![Reply::HangUp, Reply::Json(201, ok(CHARGE))]);
    let (client, warnings) = warning_client(&server, u32::MAX);
    let charge = client
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect("the retry lands");
    assert_eq!(charge.status, "succeeded");
    assert_eq!(server.requests().len(), 2);
    assert_eq!(warnings.all().len(), 1);
    assert!(
        warnings.all()[0].ends_with(&format!("(attempt 2 of {})", u32::MAX)),
        "{:?}",
        warnings.all()
    );
}
