//! The session fields added for verify-only and subscription sessions: on the
//! request `verifyOnly`, `agreementId`, `consent` and `locationId`, and the
//! amount rule that changes with `verifyOnly`; on the checkout and the status
//! read the fields that answer them.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    sign_request, CheckoutSessionRequest, Client, Consent, Error, IdempotencyKey, Integration,
    SignRequest, SESSIONS_PATH,
};
use serde_json::{json, Value};
use support::{MockServer, Recorded, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const KEY: &str = "verify-customer-77";
const AGREEMENT_ID: &str = "agr_0123456789abcdef0123456789abcdef";
const TRANSACTION_ID: &str = "11111111-1111-4111-8111-111111111111";

const CHECKOUT: &str = r#"{"transactionId":"11111111-1111-4111-8111-111111111111","orderId":"dom_42","cashierKey":"ck_live","cashierToken":"ct_live","amount":100,"currency":"EUR","expiresAt":"2026-10-11T12:00:00Z","integration":"widget","verifyOnly":true}"#;

fn create_ok() -> Reply {
    Reply::enveloped(&format!(r#"{{"success":true,"checkout":{CHECKOUT}}}"#))
}

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(format!("{}/api", server.base_url()))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

fn key() -> IdempotencyKey {
    IdempotencyKey::new(KEY).expect("a valid idempotency key")
}

fn assert_signed(recorded: &Recorded) {
    let expected = sign_request(SignRequest {
        secret: SECRET,
        timestamp: recorded.header("X-Timestamp").expect("X-Timestamp sent"),
        method: "POST",
        path: SESSIONS_PATH,
        idempotency_key: KEY,
        body: &recorded.body,
    });
    assert_eq!(recorded.header("X-Signature"), Some(expected.as_str()));
    assert_eq!(recorded.header("Idempotency-Key"), Some(KEY));
}

#[test]
fn a_verify_only_session_sends_amount_0_and_verify_only() {
    let server = MockServer::start(vec![create_ok()]);
    let request = CheckoutSessionRequest::new(0, "EUR", "customer-77", key()).verify_only(true);

    client_for(&server)
        .create_checkout_session(&request)
        .expect("a verify-only session");

    let recorded = server.only_request();
    assert_eq!(
        recorded.body,
        r#"{"amount":0,"currency":"EUR","orderReference":"customer-77","verifyOnly":true}"#
    );
    assert_signed(&recorded);
}

/// The platform sets the verification amount, so a real amount with
/// verify_only is refused before anything is sent, exactly like the gateway
/// would refuse it.
#[test]
fn a_verify_only_session_with_an_amount_is_refused_before_sending() {
    let server = MockServer::start(vec![create_ok()]);
    for amount in [2500, 1, -1] {
        let request =
            CheckoutSessionRequest::new(amount, "EUR", "customer-77", key()).verify_only(true);
        let error = client_for(&server)
            .create_checkout_session(&request)
            .expect_err("refused");
        assert!(
            matches!(error, Error::Validation { .. }),
            "{amount}: {error:?}"
        );
    }
    assert!(server.requests().is_empty());
}

/// Every other session keeps the amount check: zero or less never leaves.
#[test]
fn amount_0_without_verify_only_is_still_refused_before_sending() {
    let server = MockServer::start(vec![create_ok()]);
    for request in [
        CheckoutSessionRequest::new(0, "EUR", "order-1042", key()),
        CheckoutSessionRequest::new(0, "EUR", "order-1042", key()).verify_only(false),
        CheckoutSessionRequest::new(-5, "EUR", "order-1042", key()),
    ] {
        let error = client_for(&server)
            .create_checkout_session(&request)
            .expect_err("refused");
        assert!(matches!(error, Error::Validation { .. }), "{error:?}");
    }
    assert!(server.requests().is_empty());
}

/// A verify-only session still names a currency and an order reference.
#[test]
fn a_verify_only_session_still_needs_currency_and_order_reference() {
    let server = MockServer::start(vec![create_ok()]);
    for (currency, order_reference) in [("", "customer-77"), ("EUR", " ")] {
        let request =
            CheckoutSessionRequest::new(0, currency, order_reference, key()).verify_only(true);
        let error = client_for(&server)
            .create_checkout_session(&request)
            .expect_err("refused");
        assert!(matches!(error, Error::Validation { .. }), "{error:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn the_subscription_and_storefront_fields_are_sent_with_their_wire_names() {
    let server = MockServer::start(vec![create_ok()]);
    let request = CheckoutSessionRequest::new(1990, "EUR", "sub-8817", key())
        .integration(Integration::Widget)
        .agreement_id(AGREEMENT_ID)
        .consent(Consent::new(
            "recurring-2026-09",
            "2026-10-11T09:30:00Z",
            "203.0.113.7",
        ))
        .location_id("6f1c2b8e-3d4a-4b5c-9e7f-0a1b2c3d4e5f")
        .extra("futureField", json!("kept"));

    client_for(&server)
        .create_checkout_session(&request)
        .expect("a subscription session");

    let recorded = server.only_request();
    // The new fields come after the 0.4.0 ones and before the extra fields, so
    // a body without them is byte for byte what 0.4.0 sent.
    assert_eq!(
        recorded.body,
        concat!(
            r#"{"amount":1990,"currency":"EUR","orderReference":"sub-8817","integration":"widget","#,
            r#""agreementId":"agr_0123456789abcdef0123456789abcdef","#,
            r#""consent":{"textVersion":"recurring-2026-09","acceptedAt":"2026-10-11T09:30:00Z","ip":"203.0.113.7"},"#,
            r#""locationId":"6f1c2b8e-3d4a-4b5c-9e7f-0a1b2c3d4e5f","futureField":"kept"}"#
        )
    );
    assert_signed(&recorded);
}

#[test]
fn every_session_field_the_gateway_accepts_has_a_wire_name() {
    let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key())
        .customer(
            dominaite::Customer::new()
                .first_name("Ana")
                .last_name("Petrova")
                .email("ana@example.com")
                .phone("+359888123456"),
        )
        .country("BG")
        .language("bg")
        .theme("dark")
        .description("Order 1042")
        .save_card(true)
        .integration(Integration::Widget)
        .verify_only(false)
        .agreement_id(AGREEMENT_ID)
        .consent(Consent::new(
            "recurring-2026-09",
            "2026-10-11T09:30:00Z",
            "::1",
        ))
        .location_id("6f1c2b8e-3d4a-4b5c-9e7f-0a1b2c3d4e5f");

    let body: Value = serde_json::to_value(&request).expect("serializes");
    let mut keys: Vec<&str> = body
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    // The gateway's CreateMerchantCheckoutSessionRequest, field for field.
    assert_eq!(
        keys,
        [
            "agreementId",
            "amount",
            "consent",
            "country",
            "currency",
            "customer",
            "description",
            "integration",
            "language",
            "locationId",
            "orderReference",
            "saveCard",
            "theme",
            "verifyOnly",
        ]
    );
    assert_eq!(
        body["customer"],
        json!({"firstName":"Ana","lastName":"Petrova","email":"ana@example.com","phone":"+359888123456"})
    );
}

#[test]
fn the_checkout_carries_verify_only_and_the_agreement() {
    let server = MockServer::start(vec![create_ok()]);
    let request = CheckoutSessionRequest::new(0, "EUR", "customer-77", key()).verify_only(true);
    let session = client_for(&server)
        .create_checkout_session(&request)
        .expect("a verify-only session");
    assert!(session.verify_only);
    // The platform's verification hold, not the 0 that was sent.
    assert_eq!(session.amount, 100);
    assert_eq!(session.agreement_id, None);

    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"success":true,"checkout":{{"transactionId":"11111111-1111-4111-8111-111111111111","amount":1990,"currency":"EUR","agreementId":"{AGREEMENT_ID}","verifyOnly":null}}}}"#
    ))]);
    let session = client_for(&server)
        .create_checkout_session(&CheckoutSessionRequest::new(1990, "EUR", "sub-8817", key()))
        .expect("a subscription session");
    assert_eq!(session.agreement_id.as_deref(), Some(AGREEMENT_ID));
    assert!(!session.verify_only);
}

#[test]
fn the_status_read_carries_the_decline_recurring_verify_only_and_agreement_fields() {
    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"transactionId":"{TRANSACTION_ID}","orderId":"dom_42","status":"failed","amount":1990,"currency":"EUR","declineClass":"soft_funds","isRecurring":true,"agreement":{{"id":"{AGREEMENT_ID}","status":"past_due","nextChargeAt":"2026-10-14T09:00:00Z"}},"sequence":3,"createdAt":"2026-10-11T09:00:00Z"}}"#
    ))]);
    let status = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect("a status read");

    assert_eq!(
        status.decline_class.as_deref(),
        Some(dominaite::decline_class::SOFT_FUNDS)
    );
    assert!(status.is_recurring);
    assert!(!status.verify_only);
    let agreement = status.agreement.as_ref().expect("an agreement");
    assert_eq!(agreement.id, AGREEMENT_ID);
    assert_eq!(agreement.status, dominaite::agreement_status::PAST_DUE);
    assert_eq!(
        agreement.next_charge_at.as_deref(),
        Some("2026-10-14T09:00:00Z")
    );
    assert_eq!(status.sequence, Some(3));
    assert_eq!(status.raw["agreement"]["status"], "past_due");
}

/// A verify-only session's read: the raw status, verifyOnly true, and the saved
/// card is what says the card is verified. Absent flags read as false.
#[test]
fn a_verify_only_status_read_and_an_older_one_both_parse() {
    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"transactionId":"{TRANSACTION_ID}","status":"requires_capture","amount":100,"currency":"EUR","verifyOnly":true,"isRecurring":false,"sequence":0,"storedPaymentMethod":{{"id":"pm_0123456789abcdef0123456789abcdef","status":"active"}}}}"#
    ))]);
    let status = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect("a status read");
    assert!(status.verify_only);
    assert!(status.stored_payment_method.is_some());
    assert_eq!(status.sequence, Some(0));

    // A 0.4.0-era answer: none of the new fields, and nulls where a server
    // writes them.
    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"transactionId":"{TRANSACTION_ID}","status":"succeeded","amount":2500,"currency":"EUR","declineClass":null,"isRecurring":null,"verifyOnly":null,"agreement":null}}"#
    ))]);
    let status = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect("a status read");
    assert_eq!(status.decline_class, None);
    assert!(!status.is_recurring);
    assert!(!status.verify_only);
    assert_eq!(status.agreement, None);
    assert_eq!(status.sequence, None);
}
