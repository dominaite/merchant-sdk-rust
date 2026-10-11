//! Subscription agreements against the loopback mock: `create_agreement` on
//! `/merchant-api/agreements`, and `get_agreement` and `cancel_agreement` on
//! `/merchant-api/agreements/{id}`. No network, no credentials.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    agreement_status, past_due_reason, sign_request, AgreementRequest, Client, Consent, Error,
    IdempotencyKey, SignRequest, AGREEMENTS_PATH,
};
use serde_json::json;
use support::{MockServer, Recorded, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const PLAN_ID: &str = "plan_0123456789abcdef0123456789abcdef";
// Mixed case on purpose: an agreement id is case-sensitive and must reach the
// path exactly as given.
const AGREEMENT_ID: &str = "agr_0123456789ABCDEF0123456789abcdef";
const PAYMENT_METHOD_ID: &str = "pm_0123456789abcdef0123456789abcdef";
const AGREEMENT_KEY: &str = "subscription-8817";

const PENDING: &str = r#"{"id":"agr_0123456789ABCDEF0123456789abcdef","planId":"plan_0123456789abcdef0123456789abcdef","customerReference":"customer-77","status":"pending","amount":1990,"currency":"EUR","intervalUnit":"month","intervalCount":1,"trialDays":14,"version":1,"sequence":0,"createdAt":"2026-10-11T09:00:00Z"}"#;

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(format!("{}/api", server.base_url()))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

fn key() -> IdempotencyKey {
    IdempotencyKey::new(AGREEMENT_KEY).expect("a valid idempotency key")
}

fn enveloped(status: u16, data: &str) -> Reply {
    Reply::Json(
        status,
        format!(r#"{{"success":true,"data":{data},"metadata":{{"requestId":"r1"}}}}"#),
    )
}

fn agreement_path() -> String {
    format!("{AGREEMENTS_PATH}/{AGREEMENT_ID}")
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
fn create_agreement_sends_every_field_in_order_and_signs_the_key() {
    let server = MockServer::start(vec![enveloped(201, PENDING)]);
    let request = AgreementRequest::new(PLAN_ID, "customer-77", key())
        .stored_payment_method_id(PAYMENT_METHOD_ID)
        .consent(Consent::new(
            "terms-2026-09",
            "2026-10-11T08:59:00Z",
            "203.0.113.7",
        ))
        .max_amount_per_period(4990)
        .extra("futureField", json!(true));

    let agreement = client_for(&server)
        .create_agreement(&request)
        .expect("an agreement");

    assert_eq!(agreement.id, AGREEMENT_ID);
    assert_eq!(agreement.status, agreement_status::PENDING);
    assert!(!agreement.is_active());
    let recorded = server.only_request();
    assert_eq!(recorded.method, "POST");
    assert_eq!(recorded.path, format!("/api{AGREEMENTS_PATH}"));
    assert_eq!(
        recorded.body,
        concat!(
            r#"{"planId":"plan_0123456789abcdef0123456789abcdef","customerReference":"customer-77","#,
            r#""storedPaymentMethodId":"pm_0123456789abcdef0123456789abcdef","#,
            r#""consent":{"textVersion":"terms-2026-09","acceptedAt":"2026-10-11T08:59:00Z","ip":"203.0.113.7"},"#,
            r#""maxAmountPerPeriod":4990,"futureField":true}"#
        )
    );
    assert_eq!(recorded.header("Idempotency-Key"), Some(AGREEMENT_KEY));
    assert_signature_matches(&recorded, AGREEMENTS_PATH, AGREEMENT_KEY);
}

#[test]
fn a_minimal_agreement_sends_only_the_plan_and_the_customer() {
    let server = MockServer::start(vec![enveloped(201, PENDING)]);
    client_for(&server)
        .create_agreement(&AgreementRequest::new(PLAN_ID, "customer-77", key()))
        .expect("an agreement");
    assert_eq!(
        server.only_request().body,
        r#"{"planId":"plan_0123456789abcdef0123456789abcdef","customerReference":"customer-77"}"#
    );
}

#[test]
fn create_agreement_refuses_a_missing_plan_or_customer_before_sending() {
    let server = MockServer::start(vec![enveloped(201, PENDING)]);
    for request in [
        AgreementRequest::new("", "customer-77", key()),
        AgreementRequest::new(PLAN_ID, " ", key()),
    ] {
        let error = client_for(&server)
            .create_agreement(&request)
            .expect_err("refused");
        assert!(matches!(error, Error::Validation { .. }), "{error:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn an_archived_plan_is_plan_not_active() {
    let server = MockServer::start(vec![Reply::error_envelope(
        409,
        "PLAN_NOT_ACTIVE",
        "This plan is archived; no new agreement can be created from it.",
    )]);
    let error = client_for(&server)
        .create_agreement(&AgreementRequest::new(PLAN_ID, "customer-77", key()))
        .expect_err("refused");
    assert!(matches!(error, Error::Api { status: 409, .. }), "{error:?}");
    assert_eq!(error.code(), Some("PLAN_NOT_ACTIVE"));
}

#[test]
fn get_agreement_reads_every_field_and_signs_like_a_get() {
    let past_due = r#"{"id":"agr_0123456789ABCDEF0123456789abcdef","planId":"plan_0123456789abcdef0123456789abcdef","customerReference":"customer-77","storedPaymentMethodId":"pm_0123456789abcdef0123456789abcdef","status":"past_due","amount":1990,"maxAmountPerPeriod":4990,"currency":"EUR","intervalUnit":"month","intervalCount":1,"periodCount":12,"trialDays":0,"nextChargeAt":"2026-11-11T09:00:00Z","pastDueReason":"card_expired","pastDueAt":"2026-10-11T09:00:00Z","consent":{"textVersion":"recurring-2026-09","acceptedAt":"2026-09-11T08:59:00Z","ip":"203.0.113.7","receivedAt":"2026-09-11T09:00:00Z","receivedFromIp":"198.51.100.4"},"version":2,"sequence":4,"createdAt":"2026-09-11T09:00:00Z","activatedAt":"2026-09-11T09:05:00Z"}"#;
    let server = MockServer::start(vec![enveloped(200, past_due)]);

    let agreement = client_for(&server)
        .get_agreement(AGREEMENT_ID)
        .expect("the agreement");

    assert_eq!(agreement.status, agreement_status::PAST_DUE);
    assert_eq!(
        agreement.past_due_reason.as_deref(),
        Some(past_due_reason::CARD_EXPIRED)
    );
    assert_eq!(
        agreement.past_due_at.as_deref(),
        Some("2026-10-11T09:00:00Z")
    );
    assert_eq!(
        agreement.stored_payment_method_id.as_deref(),
        Some(PAYMENT_METHOD_ID)
    );
    assert_eq!(agreement.max_amount_per_period, Some(4990));
    assert_eq!(agreement.period_count, Some(12));
    assert_eq!(agreement.version, 2);
    assert_eq!(agreement.sequence, 4);
    assert_eq!(
        agreement.activated_at.as_deref(),
        Some("2026-09-11T09:05:00Z")
    );
    assert_eq!(agreement.cancelled_at, None);
    let consent = agreement.consent.as_ref().expect("a consent record");
    assert_eq!(consent.text_version, "recurring-2026-09");
    assert_eq!(consent.received_at.as_deref(), Some("2026-09-11T09:00:00Z"));
    assert_eq!(consent.received_from_ip.as_deref(), Some("198.51.100.4"));
    assert_eq!(agreement.raw["pastDueReason"], "card_expired");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "GET");
    assert_eq!(recorded.path, format!("/api{}", agreement_path()));
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_eq!(recorded.body, "");
    assert_signature_matches(&recorded, &agreement_path(), "");
}

/// A pending agreement has no card, no consent and no next charge yet.
#[test]
fn a_pending_agreement_reads_its_missing_fields_as_none() {
    let server = MockServer::start(vec![enveloped(200, PENDING)]);
    let agreement = client_for(&server)
        .get_agreement(AGREEMENT_ID)
        .expect("the agreement");
    assert_eq!(agreement.stored_payment_method_id, None);
    assert_eq!(agreement.consent, None);
    assert_eq!(agreement.next_charge_at, None);
    assert_eq!(agreement.past_due_reason, None);
    assert_eq!(agreement.max_amount_per_period, None);
    assert_eq!(agreement.sequence, 0);
}

#[test]
fn an_agreement_that_is_not_yours_is_agreement_not_found() {
    let server = MockServer::start(vec![Reply::error_envelope(
        404,
        "AGREEMENT_NOT_FOUND",
        "No agreement with this id.",
    )]);
    let error = client_for(&server)
        .get_agreement(AGREEMENT_ID)
        .expect_err("not found");
    assert!(matches!(error, Error::Api { status: 404, .. }), "{error:?}");
    assert_eq!(error.code(), Some("AGREEMENT_NOT_FOUND"));
}

#[test]
fn cancel_agreement_is_a_delete_that_resolves_on_a_204() {
    let server = MockServer::start(vec![Reply::Json(204, String::new())]);
    client_for(&server)
        .cancel_agreement(AGREEMENT_ID)
        .expect("cancelled");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "DELETE");
    assert_eq!(recorded.path, format!("/api{}", agreement_path()));
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_eq!(recorded.body, "");
    assert_signature_matches(&recorded, &agreement_path(), "");
}

#[test]
fn cancelling_an_ended_agreement_is_agreement_ended() {
    let server = MockServer::start(vec![Reply::error_envelope(
        409,
        "AGREEMENT_ENDED",
        "This agreement has already ended and cannot be cancelled.",
    )]);
    let error = client_for(&server)
        .cancel_agreement(AGREEMENT_ID)
        .expect_err("ended");
    assert!(matches!(error, Error::Api { status: 409, .. }), "{error:?}");
    assert_eq!(error.code(), Some("AGREEMENT_ENDED"));
}

#[test]
fn an_agreement_id_that_is_not_one_path_segment_is_refused_before_sending() {
    let server = MockServer::start(vec![enveloped(200, PENDING)]);
    let client = client_for(&server);
    for id in ["", "agr_1/../x", "agr_1?x=1", "agr 1"] {
        for error in [
            client.get_agreement(id).expect_err("refused"),
            client.cancel_agreement(id).expect_err("refused"),
        ] {
            assert!(
                matches!(error, Error::Validation { .. }),
                "{id:?}: {error:?}"
            );
        }
    }
    assert!(server.requests().is_empty());
}
