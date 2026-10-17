//! Pins the crate's error-code constants against the gateway's code list.
//!
//! The wire fixture (`merchant-api-wire-contract.json`, refreshed from the live
//! contract) carries six `errorCodes` groups. Every code in it must have a
//! constant here. The charge, recurring, refund and webhook vocabularies have
//! no group in the fixture, so they are pinned to literal lists taken from the
//! contract's route outcomes.

use std::collections::BTreeSet;

use serde_json::Value;

use dominaite::{
    auth_error_code, charge_error_code, rate_limit_error_code, recurring_error_code,
    refund_error_code, revoke_error_code, session_error_code, transient_error_code,
    validation_error_code, webhook_error_code,
};

const WIRE: &str = include_str!("merchant-api-wire-contract.json");

fn group(name: &str) -> Vec<(String, u64)> {
    let wire: Value = serde_json::from_str(WIRE).expect("the wire fixture is valid JSON");
    wire["errorCodes"][name]
        .as_array()
        .unwrap_or_else(|| panic!("errorCodes.{name} is an array"))
        .iter()
        .map(|entry| {
            (
                entry["code"].as_str().expect("code").to_string(),
                entry["httpStatus"].as_u64().expect("httpStatus"),
            )
        })
        .collect()
}

fn codes(name: &str) -> Vec<String> {
    group(name).into_iter().map(|(code, _)| code).collect()
}

/// Every constant list this crate exports, flattened.
fn every_constant() -> BTreeSet<&'static str> {
    let mut all = BTreeSet::new();
    all.extend(session_error_code::REFUSALS);
    all.extend(session_error_code::STOREFRONT);
    all.insert(session_error_code::IDEMPOTENCY_KEY_REQUIRED);
    all.extend(charge_error_code::ALL);
    all.extend(revoke_error_code::ALL);
    all.extend(refund_error_code::ALL);
    all.extend(recurring_error_code::ALL);
    all.extend(webhook_error_code::ALL);
    all.extend(validation_error_code::ALL);
    all.insert(validation_error_code::VALIDATION_ERROR);
    all.extend(transient_error_code::ALL);
    all.extend(auth_error_code::AUTHENTICATION);
    all.extend(auth_error_code::AUTHORIZATION);
    all.insert(auth_error_code::VERIFY_ONLY_NOT_ENABLED);
    all.extend([
        rate_limit_error_code::RATE_LIMIT_EXCEEDED,
        rate_limit_error_code::CHARGE_VELOCITY_EXCEEDED,
        rate_limit_error_code::CHARGE_DECLINE_RATE_EXCEEDED,
    ]);
    all
}

#[test]
fn every_code_in_the_wire_fixture_has_a_constant() {
    let wire: Value = serde_json::from_str(WIRE).expect("valid JSON");
    let groups = wire["errorCodes"]
        .as_object()
        .expect("errorCodes is an object");
    assert!(!groups.is_empty());
    let known = every_constant();
    for name in groups.keys() {
        for code in codes(name) {
            assert!(
                known.contains(code.as_str()),
                "errorCodes.{name}: {code} has no constant in this crate"
            );
        }
    }
}

#[test]
fn the_storefront_constants_match_the_fixture_in_order() {
    assert_eq!(session_error_code::STOREFRONT.to_vec(), codes("storefront"));
}

#[test]
fn the_transient_constants_match_the_fixture_in_order() {
    assert_eq!(transient_error_code::ALL.to_vec(), codes("transient"));
}

#[test]
fn the_validation_field_constants_match_the_fixture_in_order() {
    assert_eq!(validation_error_code::ALL.to_vec(), codes("validation"));
}

#[test]
fn every_idempotency_code_is_a_session_constant_with_its_shape() {
    for (code, status) in group("idempotency") {
        if status == 200 {
            assert!(
                session_error_code::REFUSALS.contains(&code.as_str()),
                "{code} is an HTTP 200 refusal, so it belongs in REFUSALS"
            );
        } else {
            assert_eq!(code, session_error_code::IDEMPOTENCY_KEY_REQUIRED);
            assert!(!session_error_code::REFUSALS.contains(&code.as_str()));
        }
    }
}

#[test]
fn the_charge_constants_cover_every_coded_charge_answer() {
    // The contract's charge outcomes, minus CHARGE_DECLINED (a 402 result,
    // not an error), the two 429 codes (Error::RateLimited) and
    // IDEMPOTENCY_KEY_REQUIRED (a 400, Error::Api).
    let expected: BTreeSet<&str> = [
        "PAYMENT_METHOD_NOT_ACTIVE",
        "PAYMENT_METHOD_NOT_FOUND",
        "CHARGE_IN_PROGRESS",
        "DUPLICATE_REQUEST",
        "IDEMPOTENCY_KEY_REUSED",
        "CHARGE_CURRENCY_MISMATCH",
        "AGREEMENT_NOT_FOUND",
        "AGREEMENT_METHOD_MISMATCH",
        "AGREEMENT_CONSENT_MISSING",
        "AGREEMENT_CURRENCY_MISMATCH",
        "AGREEMENT_AMOUNT_MISMATCH",
        "CHARGE_AMOUNT_ABOVE_AGREEMENT",
        "AGREEMENT_PERIOD_ALREADY_CHARGED",
        "AGREEMENT_ENDED",
        "AGREEMENT_NOT_STARTED",
        "CHARGE_AMOUNT_ABOVE_CAP",
        "CHARGE_OUTCOME_UNKNOWN",
        "CHARGE_FAILED",
        "PAYMENT_METHOD_CHARGES_DISABLED",
        "PAYMENT_PROCESSING_UNAVAILABLE",
    ]
    .into();
    let actual: BTreeSet<&str> = charge_error_code::ALL.into();
    assert_eq!(actual, expected);
    assert_eq!(
        charge_error_code::ALL.len(),
        expected.len(),
        "no duplicates"
    );
}

#[test]
fn the_session_refusals_include_every_subscription_session_code() {
    for code in [
        "AGREEMENT_NOT_FOUND",
        "AGREEMENT_NOT_PENDING",
        "AGREEMENT_AMOUNT_MISMATCH",
        "AGREEMENT_CONSENT_REQUIRED",
        "AGREEMENT_SESSION_OPEN",
        "AGREEMENT_ALREADY_PAID",
        "AGREEMENT_VARIABLE_AMOUNT_UNSUPPORTED",
        "SUBSCRIPTIONS_NOT_ENABLED",
    ] {
        assert!(session_error_code::REFUSALS.contains(&code), "{code}");
    }
    // STOREFRONT_MISMATCH can also come back as a 200 refusal on a replay, but
    // it is a storefront code, never a refusal.
    assert!(!session_error_code::REFUSALS.contains(&session_error_code::STOREFRONT_MISMATCH));
}

#[test]
fn the_recurring_constants_match_the_contract_in_order() {
    assert_eq!(
        recurring_error_code::ALL.to_vec(),
        [
            "PLAN_NOT_FOUND",
            "PLAN_NOT_ACTIVE",
            "AGREEMENT_NOT_FOUND",
            "AGREEMENT_ENDED",
            "PAYMENT_METHOD_NOT_FOUND",
            "PAYMENT_METHOD_NOT_ACTIVE",
            "PAYMENT_METHOD_CURRENCY_MISMATCH",
            "MAX_AMOUNT_BELOW_PLAN_AMOUNT",
            "AGREEMENT_CONSENT_REQUIRED",
            "PAYMENT_METHOD_CURRENCY_UNKNOWN",
            "IDEMPOTENCY_KEY_REUSED",
            "INVALID_CURRENCY",
        ]
    );
}

#[test]
fn the_webhook_constants_match_the_contract_in_order() {
    assert_eq!(
        webhook_error_code::ALL.to_vec(),
        [
            "WEBHOOK_URL_REJECTED",
            "WEBHOOK_LIMIT_REACHED",
            "WEBHOOK_ALREADY_REGISTERED",
            "WEBHOOK_NOT_FOUND",
            "IDEMPOTENCY_KEY_REUSED",
            "DUPLICATE_REQUEST",
            "IDEMPOTENCY_KEY_REQUIRED",
        ]
    );
}
