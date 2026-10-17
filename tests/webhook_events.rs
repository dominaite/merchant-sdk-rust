//! Typed `data` on the `charge.*` and `agreement.*` events. Webhooks send
//! explicit nulls, and every field the platform can leave empty must read as
//! `None`, not fail the parse.

use dominaite::{agreement_status, charge_outcome, decline_class, event_type, WebhookEvent};

fn parse_event(event_type: &str, data: &str) -> WebhookEvent {
    WebhookEvent::parse(&format!(
        r#"{{"id":"7f9c24e5-1d1f-4c0a-9b6c-2f3a4d5e6f70","type":"{event_type}","apiVersion":"2026-09-25","createdAt":"2026-09-25T10:00:00Z","data":{data}}}"#
    ))
    .expect("a valid envelope")
}

#[test]
fn a_charge_failed_event_reads_every_field() {
    let event = parse_event(
        event_type::CHARGE_FAILED,
        r#"{"chargeId":"ch_1","transactionId":"33333333-3333-4333-8333-333333333333","storedPaymentMethodId":"pm_1","agreementId":"agr_1","customerReference":"cust-9","outcome":"failed","periodNumber":3,"attemptNumber":4,"amount":1990,"currency":"EUR","paymentMethod":{"brand":"visa","last4":"4242"},"orderReference":"sub-9-3","description":"Monthly","declineClass":"hard","declineCode":"05","nextAttemptAt":null,"nextChargeAt":"2026-10-25T10:00:00.000000Z","sequence":7}"#,
    );
    let data = event.charge_data().expect("typed charge data");
    assert_eq!(data.charge_id, "ch_1");
    assert_eq!(
        data.transaction_id.as_deref(),
        Some("33333333-3333-4333-8333-333333333333")
    );
    assert_eq!(data.stored_payment_method_id.as_deref(), Some("pm_1"));
    assert_eq!(data.agreement_id.as_deref(), Some("agr_1"));
    assert_eq!(data.customer_reference.as_deref(), Some("cust-9"));
    assert_eq!(data.outcome.as_deref(), Some(charge_outcome::FAILED));
    assert_eq!(data.period_number, Some(3));
    assert_eq!(data.attempt_number, Some(4));
    assert_eq!(data.amount, Some(1990));
    assert_eq!(data.currency.as_deref(), Some("EUR"));
    let card = data.payment_method.expect("the card");
    assert_eq!(card.brand.as_deref(), Some("visa"));
    assert_eq!(card.last4.as_deref(), Some("4242"));
    assert_eq!(data.order_reference.as_deref(), Some("sub-9-3"));
    assert_eq!(data.description.as_deref(), Some("Monthly"));
    assert_eq!(data.decline_class.as_deref(), Some(decline_class::HARD));
    assert_eq!(data.decline_code.as_deref(), Some("05"));
    assert_eq!(data.next_attempt_at, None);
    assert_eq!(
        data.next_charge_at.as_deref(),
        Some("2026-10-25T10:00:00.000000Z")
    );
    assert_eq!(data.sequence, Some(7));

    assert!(event.agreement_data().is_none());
    assert!(event.payment_data().is_none());
}

#[test]
fn a_one_off_charge_with_explicit_nulls_reads_none() {
    let event = parse_event(
        event_type::CHARGE_SUCCEEDED,
        r#"{"chargeId":"ch_2","transactionId":"t","storedPaymentMethodId":null,"agreementId":null,"customerReference":null,"outcome":"succeeded","periodNumber":null,"attemptNumber":null,"amount":2500,"currency":"EUR","paymentMethod":null,"orderReference":null,"description":null,"declineClass":null,"declineCode":null,"nextAttemptAt":null,"nextChargeAt":null,"sequence":null}"#,
    );
    let data = event.charge_data().expect("typed charge data");
    assert_eq!(data.charge_id, "ch_2");
    assert_eq!(data.stored_payment_method_id, None);
    assert_eq!(data.agreement_id, None);
    assert_eq!(data.payment_method, None);
    assert_eq!(data.period_number, None);
    assert_eq!(data.sequence, None);

    // A card whose brand is not known yet.
    let event = parse_event(
        event_type::CHARGE_RETRYING,
        r#"{"chargeId":"ch_3","paymentMethod":{"brand":null,"last4":null}}"#,
    );
    let card = event
        .charge_data()
        .expect("typed charge data")
        .payment_method
        .expect("the card object");
    assert_eq!(card.brand, None);
    assert_eq!(card.last4, None);
}

#[test]
fn an_agreement_event_reads_every_field_and_tolerates_nulls() {
    let event = parse_event(
        event_type::AGREEMENT_CANCELLED,
        r#"{"id":"agr_1","planId":"plan_1","customerReference":"cust-9","storedPaymentMethodId":"pm_1","status":"cancelled","previousStatus":"active","amount":1990,"currency":"EUR","intervalUnit":"month","intervalCount":1,"periodCount":null,"trialDays":0,"nextChargeAt":null,"activatedAt":"2026-09-25T10:00:00.000000Z","cancelledAt":"2026-10-01T10:00:00.000000Z","version":4,"sequence":5}"#,
    );
    let data = event.agreement_data().expect("typed agreement data");
    assert_eq!(data.id, "agr_1");
    assert_eq!(data.plan_id.as_deref(), Some("plan_1"));
    assert_eq!(data.customer_reference.as_deref(), Some("cust-9"));
    assert_eq!(data.stored_payment_method_id.as_deref(), Some("pm_1"));
    assert_eq!(data.status.as_deref(), Some(agreement_status::CANCELLED));
    assert_eq!(
        data.previous_status.as_deref(),
        Some(agreement_status::ACTIVE)
    );
    assert_eq!(data.amount, Some(1990));
    assert_eq!(data.currency.as_deref(), Some("EUR"));
    assert_eq!(data.interval_unit.as_deref(), Some("month"));
    assert_eq!(data.interval_count, Some(1));
    assert_eq!(data.period_count, None);
    assert_eq!(data.trial_days, Some(0));
    assert_eq!(data.next_charge_at, None);
    assert_eq!(
        data.activated_at.as_deref(),
        Some("2026-09-25T10:00:00.000000Z")
    );
    assert_eq!(
        data.cancelled_at.as_deref(),
        Some("2026-10-01T10:00:00.000000Z")
    );
    assert_eq!(data.version, Some(4));
    assert_eq!(data.sequence, Some(5));
    assert!(event.charge_data().is_none());

    // The same event with nothing but the id still parses.
    let bare = event_with_only_id();
    let data = bare.agreement_data().expect("typed agreement data");
    assert_eq!(data.id, "agr_2");
    assert_eq!(data.status, None);
    assert_eq!(data.amount, None);
}

fn event_with_only_id() -> WebhookEvent {
    parse_event(
        event_type::AGREEMENT_ACTIVATED,
        r#"{"id":"agr_2","status":null,"amount":null}"#,
    )
}

#[test]
fn the_charge_outcome_vocabulary_matches_the_charge_events() {
    assert_eq!(charge_outcome::ALL, ["succeeded", "retrying", "failed"]);
}
