//! Every vocabulary the API answers in stays a plain string. A value this SDK
//! has never seen (a status, a decline class, a retired reason, an event type
//! and the rest) must never fail a call or a parse: it comes through as sent,
//! and the constants modules simply do not name it.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    agreement_status, charge_status, decline_class, event_type, interval_unit, past_due_reason,
    payment_method, plan_status, refund_failure_code, refund_status, retired_reason, status,
    stored_payment_method_status, transaction_type, wallet_type, webhook_endpoint_status,
    ChargeRequest, Client, IdempotencyKey, TransactionListParams, WebhookEvent,
};
use support::{MockServer, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const TRANSACTION_ID: &str = "11111111-1111-4111-8111-111111111111";

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(format!("{}/api", server.base_url()))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

/// A value from the future, and proof that the SDK does not know it yet.
fn unseen(value: &str, known: &[&str]) -> String {
    assert!(!known.contains(&value), "{value} is not new to this SDK");
    value.to_string()
}

#[test]
fn a_status_read_with_values_never_seen_parses_and_keeps_them() {
    let status_value = unseen("awaiting_review_2027", &status::ALL);
    let decline = unseen("soft_issuer_window", &decline_class::ALL);
    let method = unseen("orbit_transfer", &payment_method::ALL);
    let wallet = unseen("orbit_pay", &wallet_type::ALL);
    let card_status = unseen("frozen", &stored_payment_method_status::ALL);
    let reason = unseen("issuer_recall", &retired_reason::ALL);
    let agreement = unseen("paused", &agreement_status::ALL);
    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"transactionId":"{TRANSACTION_ID}","status":"{status_value}","amount":2500,"currency":"EUR","paymentMethod":"{method}","walletType":"{wallet}","declineClass":"{decline}","storedPaymentMethod":{{"id":"pm_0123456789abcdef0123456789abcdef","status":"{card_status}","retiredReason":"{reason}"}},"agreement":{{"id":"agr_0123456789abcdef0123456789abcdef","status":"{agreement}"}}}}"#
    ))]);

    let read = client_for(&server)
        .get_status(TRANSACTION_ID)
        .expect("an unknown value never fails the read");

    assert_eq!(read.status, status_value);
    assert_eq!(read.decline_class.as_deref(), Some(decline.as_str()));
    assert_eq!(read.payment_method.as_deref(), Some(method.as_str()));
    assert_eq!(read.wallet_type.as_deref(), Some(wallet.as_str()));
    let card = read.stored_payment_method.expect("the card");
    assert_eq!(card.status, card_status);
    assert_eq!(card.retired_reason.as_deref(), Some(reason.as_str()));
    assert!(
        !card.is_chargeable(),
        "an unknown card status is not chargeable"
    );
    assert_eq!(read.agreement.expect("the agreement").status, agreement);
}

#[test]
fn a_charge_and_a_refund_with_values_never_seen_parse_and_keep_them() {
    let charge_value = unseen("under_review", &charge_status::ALL);
    let decline = unseen("soft_issuer_window", &decline_class::ALL);
    let server = MockServer::start(vec![Reply::Json(
        201,
        format!(
            r#"{{"success":true,"data":{{"chargeId":"ch_33333333333343338333333333333333","status":"{charge_value}","declineClass":"{decline}","transactionId":"33333333-3333-4333-8333-333333333333"}}}}"#
        ),
    )]);
    let key = IdempotencyKey::new("charge-8817").expect("a valid key");
    let charge = client_for(&server)
        .charge_payment_method(
            "pm_0123456789abcdef0123456789abcdef",
            &ChargeRequest::new(2500, "EUR", "order-1043", key),
        )
        .expect("an unknown value never fails the charge");
    assert_eq!(charge.status, charge_value);
    assert_eq!(charge.decline_class.as_deref(), Some(decline.as_str()));

    let refund_value = unseen("queued_for_bank", &refund_status::ALL);
    let failure = unseen("bank_window_closed", &refund_failure_code::ALL);
    let server = MockServer::start(vec![Reply::enveloped(&format!(
        r#"{{"refundId":"re_7c1e9a2b4d6f48a0b3c5d7e9f1a2b3c4","transactionId":"{TRANSACTION_ID}","status":"{refund_value}","currency":"EUR","failureCode":"{failure}"}}"#
    ))]);
    let refund = client_for(&server)
        .get_refund(TRANSACTION_ID, "re_7c1e9a2b4d6f48a0b3c5d7e9f1a2b3c4")
        .expect("an unknown value never fails the refund read");
    assert_eq!(refund.status, refund_value);
    assert_eq!(refund.failure_code.as_deref(), Some(failure.as_str()));
}

#[test]
fn plans_agreements_endpoints_and_transactions_with_values_never_seen_parse() {
    let plan_value = unseen("frozen", &plan_status::ALL);
    let unit = unseen("fortnight", &interval_unit::ALL);
    let agreement_value = unseen("paused", &agreement_status::ALL);
    let reason = unseen("issuer_maintenance", &past_due_reason::ALL);
    let endpoint_value = unseen("suspended", &webhook_endpoint_status::ALL);
    let kind = unseen("payout", &transaction_type::ALL);
    let status_value = unseen("awaiting_review_2027", &status::ALL);
    let server = MockServer::start(vec![
        Reply::enveloped(&format!(
            r#"{{"id":"plan_0123456789abcdef0123456789abcdef","name":"Pro","amount":1990,"currency":"EUR","intervalUnit":"{unit}","intervalCount":1,"status":"{plan_value}"}}"#
        )),
        Reply::enveloped(&format!(
            r#"{{"id":"agr_0123456789abcdef0123456789abcdef","status":"{agreement_value}","intervalUnit":"{unit}","pastDueReason":"{reason}"}}"#
        )),
        Reply::enveloped(&format!(
            r#"[{{"id":"5d0c6a52-8f1e-4b7a-9c3d-2e1f0a9b8c7d","url":"https://shop.example/hook","events":["subscription.renewed"],"status":"{endpoint_value}"}}]"#
        )),
        Reply::enveloped(&format!(
            r#"{{"items":[{{"transactionId":"{TRANSACTION_ID}","type":"{kind}","status":"{status_value}","amount":1,"currency":"EUR","createdAt":"2026-10-01T00:00:00Z"}}],"nextCursor":null}}"#
        )),
    ]);
    let client = client_for(&server);

    let plan = client
        .get_plan("plan_0123456789abcdef0123456789abcdef")
        .expect("an unknown value never fails the plan read");
    assert_eq!(plan.status, plan_value);
    assert_eq!(plan.interval_unit, unit);
    assert!(!plan.is_active(), "an unknown plan status is not active");

    let agreement = client
        .get_agreement("agr_0123456789abcdef0123456789abcdef")
        .expect("an unknown value never fails the agreement read");
    assert_eq!(agreement.status, agreement_value);
    assert_eq!(agreement.past_due_reason.as_deref(), Some(reason.as_str()));
    assert!(!agreement.is_active(), "an unknown status is not active");

    let endpoints = client
        .list_webhook_endpoints()
        .expect("an unknown value never fails the endpoint list");
    assert_eq!(endpoints[0].status, endpoint_value);
    assert_eq!(endpoints[0].events, ["subscription.renewed"]);

    let page = client
        .list_transactions(&TransactionListParams::new(
            "2026-10-01T00:00:00Z",
            "2026-10-08T00:00:00Z",
        ))
        .expect("an unknown value never fails the transaction list");
    assert_eq!(page.items[0].transaction_type, kind);
    assert_eq!(page.items[0].status, status_value);
}

#[test]
fn webhook_events_with_types_and_values_never_seen_parse() {
    let known: Vec<&str> = event_type::PAYMENT
        .iter()
        .chain(&event_type::RECURRING)
        .chain(&event_type::VERIFICATION)
        .copied()
        .collect();

    // A new payment.* type still has payment data, with an unknown status.
    let new_payment = unseen("payment.held_for_review", &known);
    let status_value = unseen("awaiting_review_2027", &status::ALL);
    let event = WebhookEvent::parse(&format!(
        r#"{{"id":"e1","type":"{new_payment}","createdAt":"2026-10-11T09:00:00Z","data":{{"transactionId":"{TRANSACTION_ID}","status":"{status_value}","previousStatus":"processing"}}}}"#
    ))
    .expect("an unknown type never fails the parse");
    assert_eq!(event.event_type, new_payment);
    assert_eq!(
        event.payment_data().expect("payment data").status,
        status_value
    );

    // A type from a family this SDK does not know parses; its data stays raw.
    let new_family = unseen("subscription.renewed", &known);
    let event = WebhookEvent::parse(&format!(
        r#"{{"id":"e2","type":"{new_family}","createdAt":"2026-10-11T09:00:00Z","data":{{"anything":["goes"]}}}}"#
    ))
    .expect("an unknown family never fails the parse");
    assert_eq!(event.payment_data(), None);
    assert_eq!(event.data["anything"][0], "goes");

    // A verification event with a decline class from the future.
    let decline = unseen("soft_issuer_window", &decline_class::ALL);
    let event = WebhookEvent::parse(&format!(
        r#"{{"id":"e3","type":"verification.failed","createdAt":"2026-10-11T09:00:00Z","data":{{"transactionId":"{TRANSACTION_ID}","status":"failed","verifyOnly":true,"declineClass":"{decline}"}}}}"#
    ))
    .expect("an unknown decline class never fails the parse");
    assert_eq!(
        event.payment_data().expect("payment data").decline_class,
        Some(decline)
    );
}
