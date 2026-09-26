//! The response contract, pinned against the canonical fixture.
//!
//! `merchant-api-contract.json` next to this file is a byte-identical copy of the
//! one every Dominaite SDK vendors. It is generated from the gateway DTOs; do NOT
//! edit it here to make a test pass - a mismatch means either this crate drifted
//! or the gateway changed, and both are fixed somewhere other than the fixture.
//!
//! What this file asserts: the status vocabulary is exactly the fixture's, each
//! response type deserializes the fixture's example, each type's serde field set
//! is exactly the fixture's field list (no extra, none missing), every session
//! refusal code comes back out of the client as a refusal that keeps its code,
//! every validation code comes back as a 400 that keeps its code, and the
//! stored-payment-method vocabularies, field sets and examples (the saved-card
//! status, a retired card and its reason, a 201 charge, a 402 decline, every
//! coded charge and revoke error, the bodiless revoke) match too, and the
//! storefront codes come back as API errors with their status, never as
//! refusals. The refund routes are pinned the same way: their status, error
//! and failure vocabularies, the refund's field set, and every example (a
//! partial and a full 202, a succeeded and a failed status read, every error)
//! through `create_refund` and `get_refund`. Every example is pushed through the client
//! twice: as spelled, and with its null members removed, because the gateway
//! omits null fields on the wire.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::cell::RefCell;
use std::time::Duration;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};
use serde::forward_to_deserialize_any;
use serde_json::Value;

use dominaite::{
    charge_error_code, charge_status, decline_class, refund_error_code, refund_failure_code,
    refund_status, retired_reason, revoke_error_code, session_error_code, status,
    stored_payment_method_status, ChargeRequest, CheckoutSession, CheckoutSessionRequest,
    CheckoutStatus, Client, Error, IdempotencyKey, PaymentMethodCharge, Ping, Refund,
    RefundRequest, StoredPaymentMethod,
};
use support::{MockServer, Reply};

const FIXTURE: &str = include_str!("merchant-api-contract.json");
const WIRE_FIXTURE: &str = include_str!("merchant-api-wire-contract.json");

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn contract() -> Value {
    serde_json::from_str(FIXTURE).expect("the fixture is valid JSON")
}

/// A list of strings from the fixture, e.g. `statusVocabulary` or an endpoint's
/// `fields`.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("a JSON array")
        .iter()
        .map(|item| item.as_str().expect("a JSON string").to_string())
        .collect()
}

fn endpoint(name: &str) -> Value {
    contract()["endpoints"][name].clone()
}

/// The example with every null object member removed, recursively: the
/// gateway's serializer omits nulls, so this is what actually crosses the wire.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(key, member)| (key.clone(), without_nulls(member)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_nulls).collect()),
        other => other.clone(),
    }
}

/// An example in both wire forms, labelled.
fn both_wire_forms(example: &Value) -> [(&'static str, Value); 2] {
    [
        ("as spelled", example.clone()),
        ("without nulls", without_nulls(example)),
    ]
}

/// The sorted member names of a JSON object.
fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The serde field names a response struct declares, in declaration order and
/// after `rename_all`. The derived `Deserialize` hands its field list to
/// `deserialize_struct`, so this reads the wire contract straight off the type
/// instead of a copy of it maintained by hand.
fn serde_fields<T: DeserializeOwned>() -> Vec<String> {
    let captured = RefCell::new(Vec::new());
    let _ = T::deserialize(FieldSpy(&captured));
    captured.into_inner()
}

struct FieldSpy<'a>(&'a RefCell<Vec<String>>);

impl<'de> Deserializer<'de> for FieldSpy<'_> {
    type Error = de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Self::Error> {
        Err(de::Error::custom("the spy only handles structs"))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        *self.0.borrow_mut() = fields.iter().map(|field| field.to_string()).collect();
        // Nothing can be built from a spy, and the caller only wants the names.
        Err(de::Error::custom("field names captured"))
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

fn assert_fields<T: DeserializeOwned>(type_name: &str, expected: &[String]) {
    let mut actual = serde_fields::<T>();
    let mut expected = expected.to_vec();
    actual.sort();
    expected.sort();

    assert_eq!(
        actual, expected,
        "{type_name} does not model exactly the fields the contract lists"
    );
}

fn idempotency_key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).expect("a valid idempotency key")
}

fn session_request() -> CheckoutSessionRequest {
    CheckoutSessionRequest::new(8440, "EUR", "order-1042", idempotency_key("order-1042"))
}

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(server.base_url())
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

#[test]
fn the_status_vocabulary_is_exactly_the_contracts() {
    let vocabulary = strings(&contract()["statusVocabulary"]);

    assert_eq!(
        status::ALL.to_vec(),
        vocabulary,
        "the SDK status vocabulary drifted from the contract"
    );
}

#[test]
fn every_contract_status_round_trips_through_the_status_response() {
    let example = endpoint("getStatus")["example"].clone();

    for value in strings(&contract()["statusVocabulary"]) {
        let mut payload = example.clone();
        payload["status"] = Value::String(value.clone());

        let status: CheckoutStatus =
            serde_json::from_value(payload).unwrap_or_else(|error| panic!("{value}: {error}"));

        assert_eq!(status.status, value);
        assert_eq!(
            status.is_paid(),
            value == status::SUCCEEDED,
            "{value}: only succeeded means the customer paid"
        );
    }
}

/// The polling contract, status by status: stop on an outcome, keep polling on
/// anything that can still move. `disputed` keeps polling, because a dispute
/// can resolve either way.
#[test]
fn every_contract_status_has_the_documented_terminal_verdict() {
    let example = endpoint("getStatus")["example"].clone();
    let terminal = [
        status::SUCCEEDED,
        status::FAILED,
        status::REFUNDED,
        status::PARTIALLY_REFUNDED,
        status::CANCELLED,
        status::ABANDONED,
    ];

    for value in strings(&contract()["statusVocabulary"]) {
        let mut payload = example.clone();
        payload["status"] = Value::String(value.clone());
        let parsed: CheckoutStatus = serde_json::from_value(payload).expect("deserializes");

        assert_eq!(
            parsed.is_terminal(),
            terminal.contains(&value.as_str()),
            "{value}: wrong terminal verdict"
        );
    }
}

#[test]
fn an_unknown_status_stays_non_terminal() {
    // Not in the fixture on purpose: the contract can grow, and a status this
    // crate has never heard of must keep the caller polling rather than close an
    // order that is still open.
    let mut payload = endpoint("getStatus")["example"].clone();
    payload["status"] = Value::String("chargeback_reversed".to_string());

    let status: CheckoutStatus = serde_json::from_value(payload).expect("deserializes");

    assert!(!status.is_paid());
    assert!(!status.is_terminal());
}

#[test]
fn ping_matches_the_contract() {
    let ping = endpoint("ping");
    assert_fields::<Ping>("Ping", &strings(&ping["fields"]));

    let parsed: Ping = serde_json::from_value(ping["example"].clone()).expect("deserializes");

    assert!(parsed.pong);
    assert_eq!(
        parsed.merchant_id,
        ping["example"]["merchantId"].as_str().expect("a string")
    );
    assert_eq!(parsed.server_unix_time, Some(1755767730));
    assert_eq!(parsed.clock_skew_seconds, 2);
}

#[test]
fn the_checkout_object_matches_the_contract() {
    let create = endpoint("createCheckoutSession");
    assert_fields::<CheckoutSession>("CheckoutSession", &strings(&create["checkoutFields"]));

    let checkout = create["successExample"]["checkout"].clone();
    let parsed: CheckoutSession = serde_json::from_value(checkout.clone()).expect("deserializes");

    assert_eq!(
        parsed.transaction_id,
        checkout["transactionId"].as_str().expect("a string")
    );
    assert_eq!(parsed.order_id, "dom_9a8b7c6d5e4f");
    assert_eq!(parsed.cashier_key, "ck_live_2f3a4d5e6f708192");
    assert_eq!(parsed.cashier_token, "ctok_5e4f3a2b1c0d9e8f");
    assert_eq!(parsed.amount, 8440);
    assert_eq!(parsed.currency, "EUR");
    assert_eq!(
        parsed.expires_at.as_deref(),
        Some("2026-08-21T11:15:30.000Z")
    );
}

#[test]
fn get_status_matches_the_contract() {
    let get_status = endpoint("getStatus");
    assert_fields::<CheckoutStatus>("CheckoutStatus", &strings(&get_status["fields"]));

    let example = get_status["example"].clone();
    let parsed: CheckoutStatus = serde_json::from_value(example).expect("deserializes");

    assert_eq!(parsed.order_id, "dom_9a8b7c6d5e4f");
    assert_eq!(parsed.order_reference.as_deref(), Some("order-1042"));
    assert_eq!(parsed.status, status::SUCCEEDED);
    assert_eq!(parsed.amount, 8440);
    assert_eq!(parsed.currency, "EUR");
    // Null in the example, and null is not zero: nothing was refunded, and the
    // SDK must not invent a 0 that reads as "a refund of nothing happened".
    assert_eq!(parsed.refunded_amount, None);
    assert_eq!(
        parsed.created_at.as_deref(),
        Some("2026-08-21T09:15:30.000Z")
    );
    assert_eq!(
        parsed.updated_at.as_deref(),
        Some("2026-08-21T09:16:05.000Z")
    );
    // Terminal, so there is no window left for the payer to act in.
    assert_eq!(parsed.expires_at, None);
    assert!(parsed.is_paid());
    assert!(parsed.is_terminal());
}

/// The create envelope is the one shape with a `success` flag of its own, and the
/// SDK splits it into `Ok`/`Err` rather than handing the flag to the caller. So
/// the envelope's field list is asserted against the two examples the fixture
/// pins, and the split itself is asserted through the client.
#[test]
fn the_create_envelope_carries_exactly_the_contract_fields() {
    let create = endpoint("createCheckoutSession");
    let mut expected = strings(&create["fields"]);
    expected.sort();

    for example in ["successExample", "refusalExample"] {
        let mut keys: Vec<String> = create[example]
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        keys.sort();

        assert_eq!(
            keys, expected,
            "{example} does not carry the contract fields"
        );
    }
}

#[test]
fn the_success_example_comes_back_as_a_session() {
    let create = endpoint("createCheckoutSession");
    let server = MockServer::start(vec![Reply::enveloped(
        &create["successExample"].to_string(),
    )]);

    let session = client_for(&server)
        .create_checkout_session(&session_request())
        .expect("the contract's success example is a session");

    assert_eq!(
        session.transaction_id,
        "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"
    );
    assert_eq!(session.cashier_token, "ctok_5e4f3a2b1c0d9e8f");
}

#[test]
fn the_refusal_example_comes_back_as_a_refusal_with_its_transaction() {
    let create = endpoint("createCheckoutSession");
    let server = MockServer::start(vec![Reply::enveloped(
        &create["refusalExample"].to_string(),
    )]);

    let error = client_for(&server)
        .create_checkout_session(&session_request())
        .expect_err("a refusal is not a session");

    // HTTP 200 all the way, and never retryable: it will not change on its own.
    assert!(!error.is_retryable());

    match error {
        Error::Refusal {
            code,
            message,
            transaction_id,
        } => {
            assert_eq!(code, "DUPLICATE_REQUEST");
            assert_eq!(message, "An identical request was already processed.");
            // The recovery path: read the colliding payment back instead of
            // minting a second one for the same order.
            assert_eq!(
                transaction_id.as_deref(),
                Some("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0")
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn the_session_refusal_constants_are_exactly_the_contracts() {
    assert_eq!(
        session_error_code::REFUSALS.to_vec(),
        strings(&contract()["sessionRefusalErrorCodes"]),
        "the SDK refusal codes drifted from the contract"
    );
}

#[test]
fn every_contract_refusal_code_survives_as_a_refusal() {
    let codes = strings(&contract()["sessionRefusalErrorCodes"]);
    assert_eq!(codes.len(), 5, "the contract lists five refusal codes");
    assert!(codes.contains(&"PRIOR_ATTEMPT_FAILED".to_string()));

    // An unlisted code rides along too: the gateway can add one, and it must
    // still arrive as a refusal with its code intact rather than as a generic
    // API error the caller cannot branch on.
    for code in codes.iter().map(String::as_str).chain(["A_NEW_CODE"]) {
        let payload =
            format!(r#"{{"success":false,"errorCode":"{code}","errorMessage":"refused"}}"#);
        let server = MockServer::start(vec![Reply::enveloped(&payload)]);

        let error = client_for(&server)
            .create_checkout_session(&session_request())
            .expect_err("a refusal is not a session");

        assert_eq!(error.code(), Some(code), "{code} did not survive");
        assert!(!error.is_retryable(), "{code} must never be blind-retried");
        assert!(
            matches!(error, Error::Refusal { .. }),
            "{code} is a business refusal, got {error:?}"
        );
    }
}

/// Validation codes are the other half of the create endpoint's error surface,
/// and they are a different shape: a real HTTP 400 rather than the 200 with
/// `success: false` that carries a refusal. Both must stay machine-readable, and
/// they must not collapse into each other - a caller that treats a rejected
/// request as a refused payment would go looking for a transaction that was
/// never created.
#[test]
fn every_contract_validation_code_survives_with_its_status() {
    let codes = strings(&contract()["validationErrorCodes"]);
    assert!(!codes.is_empty(), "the contract lists validation codes");

    for code in codes.iter().map(String::as_str) {
        let server = MockServer::start(vec![Reply::error_envelope(400, code, "rejected")]);

        let error = client_for(&server)
            .create_checkout_session(&session_request())
            .expect_err("a validation rejection is not a session");

        assert_eq!(error.code(), Some(code), "{code} did not survive");
        assert_eq!(error.http_status(), Some(400), "{code} lost its status");
        assert!(!error.is_retryable(), "{code} will not fix itself");
        assert!(
            matches!(error, Error::Api { .. }),
            "{code} is a rejected request, not a refused payment: got {error:?}"
        );
    }
}

const PAYMENT_METHOD_ID: &str = "pm_0123456789abcdef0123456789abcdef";

fn charge_request() -> ChargeRequest {
    ChargeRequest::new(2500, "EUR", "order-1043", idempotency_key("order-1043"))
}

#[test]
fn the_stored_payment_method_status_vocabulary_is_exactly_the_contracts() {
    assert_eq!(
        stored_payment_method_status::ALL.to_vec(),
        strings(&contract()["storedPaymentMethodStatusVocabulary"]),
        "the SDK stored payment method status vocabulary drifted from the contract"
    );
}

#[test]
fn the_charge_vocabularies_are_exactly_the_contracts() {
    let contract = contract();
    assert_eq!(
        charge_status::ALL.to_vec(),
        strings(&contract["chargeStatusVocabulary"]),
        "the SDK charge status vocabulary drifted from the contract"
    );
    assert_eq!(
        decline_class::ALL.to_vec(),
        strings(&contract["declineClassVocabulary"]),
        "the SDK decline class vocabulary drifted from the contract"
    );
    assert_eq!(
        charge_error_code::ALL.to_vec(),
        strings(&contract["chargeErrorCodes"]),
        "the SDK charge error codes drifted from the contract"
    );
    assert_eq!(
        revoke_error_code::ALL.to_vec(),
        strings(&contract["revokeErrorCodes"]),
        "the SDK revoke error codes drifted from the contract"
    );
}

#[test]
fn the_stored_payment_method_object_matches_the_contract() {
    let get_status = endpoint("getStatus");
    assert_fields::<StoredPaymentMethod>(
        "StoredPaymentMethod",
        &strings(&get_status["storedPaymentMethodFields"]),
    );

    for (form, example) in both_wire_forms(&get_status["savedCardExample"]) {
        let parsed: CheckoutStatus =
            serde_json::from_value(example).unwrap_or_else(|error| panic!("{form}: {error}"));

        // The saved-card example is the plain example plus a storedPaymentMethod:
        // nothing else may move when a card was stored.
        assert_eq!(parsed.status, status::SUCCEEDED, "{form}");
        assert_eq!(parsed.order_reference.as_deref(), Some("order-1042"));
        let method = parsed
            .stored_payment_method
            .unwrap_or_else(|| panic!("{form}: the example carries a stored payment method"));
        assert_eq!(method.id, PAYMENT_METHOD_ID);
        assert_eq!(method.brand.as_deref(), Some("visa"));
        assert_eq!(method.last4.as_deref(), Some("4242"));
        assert_eq!(method.expiry_month, Some(12));
        assert_eq!(method.expiry_year, Some(2029));
        assert_eq!(method.status, stored_payment_method_status::ACTIVE);
        assert_eq!(method.retired_reason, None, "{form}");
        assert!(method.is_chargeable());
    }
}

#[test]
fn the_retired_reason_vocabulary_is_exactly_the_contracts() {
    assert_eq!(
        retired_reason::ALL.to_vec(),
        strings(&contract()["storedPaymentMethodRetiredReasonVocabulary"]),
        "the SDK retired reason vocabulary drifted from the contract"
    );
}

/// A card the platform retired because the payment that saved it was refunded:
/// it reads as retired with its reason, and it is not chargeable.
#[test]
fn the_retired_card_example_reads_as_retired_and_not_chargeable() {
    let get_status = endpoint("getStatus");
    for (form, example) in both_wire_forms(&get_status["retiredCardExample"]) {
        let server = MockServer::start(vec![Reply::enveloped(&example.to_string())]);
        let parsed = client_for(&server)
            .get_status("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0")
            .unwrap_or_else(|error| panic!("{form}: {error}"));

        assert_eq!(parsed.status, status::REFUNDED, "{form}");
        assert_eq!(parsed.refunded_amount, Some(8440), "{form}");
        let method = parsed
            .stored_payment_method
            .unwrap_or_else(|| panic!("{form}: the example carries a stored payment method"));
        assert_eq!(method.id, PAYMENT_METHOD_ID, "{form}");
        assert_eq!(
            method.status,
            stored_payment_method_status::RETIRED,
            "{form}"
        );
        assert_eq!(
            method.retired_reason.as_deref(),
            Some(retired_reason::SOURCE_SALE_REVERSED),
            "{form}"
        );
        assert!(!method.is_chargeable(), "{form}");
    }
}

#[test]
fn the_storefront_constants_are_exactly_the_contracts_and_not_refusals() {
    let codes = strings(&contract()["storefrontErrorCodes"]);
    assert_eq!(
        session_error_code::STOREFRONT.to_vec(),
        codes,
        "the SDK storefront codes drifted from the contract"
    );
    for code in &codes {
        assert!(
            !session_error_code::REFUSALS.contains(&code.as_str()),
            "{code} is not a session refusal"
        );
    }
}

/// The wire contract carries each storefront code's HTTP status and retry
/// verdict. Each must come back as an API error with that status and code, not
/// as a refusal, and never retryable.
#[test]
fn every_contract_storefront_code_comes_back_as_an_api_error_with_its_status() {
    let wire: Value = serde_json::from_str(WIRE_FIXTURE).expect("the wire fixture is valid JSON");
    let entries = wire["errorCodes"]["storefront"]
        .as_array()
        .expect("the wire contract lists storefront codes");
    let codes: Vec<&str> = entries
        .iter()
        .map(|entry| entry["code"].as_str().expect("a code"))
        .collect();
    assert_eq!(session_error_code::STOREFRONT.to_vec(), codes);

    for entry in entries {
        let code = entry["code"].as_str().expect("a code");
        let http_status = entry["httpStatus"].as_u64().expect("an HTTP status") as u16;
        assert_eq!(
            entry["retry"],
            Value::Bool(false),
            "{code} is never retryable"
        );

        let server = MockServer::start(vec![Reply::error_envelope(http_status, code, "refused")]);
        let error = client_for(&server)
            .create_checkout_session(&session_request())
            .expect_err("a storefront rejection is not a session");

        assert_eq!(error.code(), Some(code), "{code} lost its code");
        assert_eq!(
            error.http_status(),
            Some(http_status),
            "{code} lost its status"
        );
        assert!(!error.is_retryable(), "{code} will not fix itself");
        assert!(
            matches!(error, Error::Api { .. }),
            "{code} is not a refused payment: got {error:?}"
        );
    }
}

#[test]
fn the_status_example_without_a_saved_card_reads_none_in_both_wire_forms() {
    let get_status = endpoint("getStatus");
    assert!(
        get_status["example"]["storedPaymentMethod"].is_null(),
        "the plain example has no stored card"
    );

    for (form, example) in both_wire_forms(&get_status["example"]) {
        let server = MockServer::start(vec![Reply::enveloped(&example.to_string())]);
        let parsed = client_for(&server)
            .get_status("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0")
            .unwrap_or_else(|error| panic!("{form}: {error}"));
        assert_eq!(parsed.stored_payment_method, None, "{form}");
    }
}

#[test]
fn a_stored_payment_method_with_null_details_reads_none_in_both_wire_forms() {
    let bare = serde_json::json!({
        "id": PAYMENT_METHOD_ID,
        "brand": null,
        "last4": null,
        "expiryMonth": null,
        "expiryYear": null,
        "status": "active",
    });
    for (form, example) in both_wire_forms(&bare) {
        let method: StoredPaymentMethod =
            serde_json::from_value(example).unwrap_or_else(|error| panic!("{form}: {error}"));
        assert_eq!(method.brand, None, "{form}");
        assert_eq!(method.last4, None, "{form}");
        assert_eq!(method.expiry_month, None, "{form}");
        assert_eq!(method.expiry_year, None, "{form}");
        assert!(method.is_chargeable(), "{form}");
    }
}

#[test]
fn every_stored_payment_method_status_round_trips_and_only_active_is_chargeable() {
    for value in strings(&contract()["storedPaymentMethodStatusVocabulary"]) {
        let method: StoredPaymentMethod = serde_json::from_value(serde_json::json!({
            "id": PAYMENT_METHOD_ID,
            "brand": "visa",
            "last4": "4242",
            "expiryMonth": 12,
            "expiryYear": 2029,
            "status": value,
        }))
        .expect("deserializes");
        assert_eq!(method.status, value);
        assert_eq!(
            method.is_chargeable(),
            value == stored_payment_method_status::ACTIVE,
            "{value}"
        );
    }
}

#[test]
fn charge_payment_method_matches_the_contract() {
    let charge = endpoint("chargePaymentMethod");
    assert_eq!(charge["method"], "POST");
    assert_eq!(
        charge["path"],
        "/merchant-api/payment-methods/{paymentMethodId}/charges"
    );
    assert_eq!(charge["httpStatus"], 201);
    assert_eq!(charge["declinedHttpStatus"], 402);

    // `sequence` is modelled ahead of the fixture: the gateway adds it to the
    // charge answer in a follow-up, and the SDK reads it as optional so it works
    // against servers that do not send it yet. Once the refreshed fixture lists
    // it, this allowance fails and must be deleted.
    const AHEAD_OF_FIXTURE: [&str; 1] = ["sequence"];
    let mut modelled = strings(&charge["fields"]);
    for field in AHEAD_OF_FIXTURE {
        assert!(
            !modelled.iter().any(|f| f == field),
            "the fixture now lists {field}; remove it from AHEAD_OF_FIXTURE"
        );
        modelled.push(field.to_string());
    }
    assert_fields::<PaymentMethodCharge>("PaymentMethodCharge", &modelled);

    // Both examples are envelopes whose data carries exactly the declared
    // fields, nulls included.
    let mut fields = strings(&charge["fields"]);
    fields.sort();
    for name in ["successExample", "declinedExample"] {
        assert_eq!(
            keys(&charge[name]["data"]),
            fields,
            "{name} does not carry exactly the declared fields"
        );
    }
    assert_eq!(charge["successExample"]["success"], true);
    assert_eq!(charge["declinedExample"]["success"], false);
    assert_eq!(
        charge["declinedExample"]["error"]["code"],
        "CHARGE_DECLINED"
    );
}

#[test]
fn every_charge_status_round_trips_and_only_succeeded_is_paid() {
    for value in strings(&contract()["chargeStatusVocabulary"]) {
        let mut data = endpoint("chargePaymentMethod")["successExample"]["data"].clone();
        data["status"] = Value::String(value.clone());
        let charge: PaymentMethodCharge = serde_json::from_value(data).expect("deserializes");
        assert_eq!(charge.status, value);
        assert_eq!(
            charge.is_paid(),
            value == charge_status::SUCCEEDED,
            "{value}"
        );
        assert_eq!(
            charge.is_terminal(),
            value != charge_status::PENDING,
            "{value}: only pending keeps the caller polling"
        );
    }
}

#[test]
fn the_charge_success_example_comes_back_as_a_paid_charge() {
    let charge = endpoint("chargePaymentMethod");
    for (form, example) in both_wire_forms(&charge["successExample"]) {
        let server = MockServer::start(vec![Reply::Json(201, example.to_string())]);

        let result = client_for(&server)
            .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
            .unwrap_or_else(|error| panic!("{form}: the success example is a charge: {error}"));

        assert_eq!(result.charge_id, "ch_1a2b3c4d5e6f4a7b8c9d0e1f2a3b4c5d");
        assert_eq!(result.status, charge_status::SUCCEEDED);
        assert_eq!(result.decline_class, None, "{form}");
        assert_eq!(result.decline_code, None, "{form}");
        assert_eq!(
            result.transaction_id,
            "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"
        );
        assert!(result.is_paid());
        assert!(result.is_terminal());
        assert_eq!(
            result.raw, example["data"],
            "{form}: raw is the charge object"
        );
    }
}

#[test]
fn the_charge_declined_example_comes_back_as_a_failed_charge_not_an_error() {
    let charge = endpoint("chargePaymentMethod");
    for (form, example) in both_wire_forms(&charge["declinedExample"]) {
        let server = MockServer::start(vec![Reply::Json(402, example.to_string())]);

        let result = client_for(&server)
            .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
            .unwrap_or_else(|error| panic!("{form}: a decline is a result: {error}"));

        assert_eq!(result.charge_id, "ch_1a2b3c4d5e6f4a7b8c9d0e1f2a3b4c5e");
        assert_eq!(result.status, charge_status::FAILED);
        assert_eq!(
            result.decline_class.as_deref(),
            Some(decline_class::SOFT_FUNDS)
        );
        assert_eq!(result.decline_code.as_deref(), Some("51"));
        assert_eq!(
            result.transaction_id,
            "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5e"
        );
        assert!(!result.is_paid());
        assert!(result.is_terminal());
        assert!(
            decline_class::ALL.contains(&result.decline_class.as_deref().unwrap()),
            "the example's decline class is inside the vocabulary"
        );
    }
}

#[test]
fn every_charge_error_example_comes_back_as_a_charge_error() {
    let charge = endpoint("chargePaymentMethod");
    let examples = charge["errorExamples"].as_array().expect("an array");
    // Eight examples for seven codes: CHARGE_FAILED is spelled both with and
    // without an attached charge row.
    assert_eq!(
        examples.len(),
        8,
        "the contract lists eight charge error examples"
    );
    let codes = strings(&contract()["chargeErrorCodes"]);
    let mut seen: Vec<String> = examples
        .iter()
        .map(|example| example["code"].as_str().expect("a code").to_string())
        .collect();
    seen.sort();
    seen.dedup();
    let mut expected = codes.clone();
    expected.sort();
    assert_eq!(seen, expected, "every charge error code has an example");

    for example in examples {
        let http_status = example["httpStatus"].as_u64().expect("a status") as u16;
        let code = example["code"].as_str().expect("a code");
        assert!(codes.contains(&code.to_string()), "{code} is a listed code");
        assert_eq!(example["body"]["error"]["code"], code);
        let expects_charge = example["body"]["data"]["chargeId"].is_string();

        for (form, body) in both_wire_forms(&example["body"]) {
            let label = format!("{code} ({http_status}, {form})");
            let server = MockServer::start(vec![Reply::Json(http_status, body.to_string())]);
            let error = client_for(&server)
                .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
                .expect_err(&label);

            assert!(!error.is_retryable(), "{label}");
            assert_eq!(error.http_status(), Some(http_status), "{label}");
            assert_eq!(error.code(), Some(code), "{label}");
            match error {
                Error::Charge {
                    status,
                    code: got,
                    message,
                    charge,
                    transaction_id,
                    raw,
                } => {
                    assert_eq!(status, http_status, "{label}");
                    assert_eq!(got, code, "{label}");
                    assert_eq!(message, example["body"]["error"]["message"], "{label}");
                    assert_eq!(raw, body, "{label}: raw is the whole envelope");
                    if expects_charge {
                        let charge = charge.unwrap_or_else(|| panic!("{label}: charge attached"));
                        assert_eq!(charge.charge_id, example["body"]["data"]["chargeId"]);
                        assert_eq!(charge.status, example["body"]["data"]["status"]);
                        assert_eq!(charge.decline_class, None, "{label}");
                        assert_eq!(charge.decline_code, None, "{label}");
                        assert_eq!(
                            transaction_id.as_deref(),
                            example["body"]["data"]["transactionId"].as_str(),
                            "{label}"
                        );
                    } else {
                        assert!(charge.is_none(), "{label}: no data, no charge");
                        assert_eq!(transaction_id, None, "{label}");
                    }
                }
                other => panic!("{label}: expected a charge error, got {other:?}"),
            }
        }
    }
}

#[test]
fn the_charge_outcome_unknown_example_names_the_transaction_to_poll() {
    let example = endpoint("chargePaymentMethod")["errorExamples"][0].clone();
    assert_eq!(example["code"], charge_error_code::CHARGE_OUTCOME_UNKNOWN);

    let server = MockServer::start(vec![Reply::Json(502, example["body"].to_string())]);
    let error = client_for(&server)
        .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
        .expect_err("no verdict is not a charge");

    match error {
        Error::Charge {
            transaction_id: Some(id),
            ..
        } => assert_eq!(id, "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5f"),
        other => panic!("expected the transaction to poll, got {other:?}"),
    }
}

#[test]
fn the_charge_not_found_example_is_an_api_error_with_its_code() {
    let example = endpoint("chargePaymentMethod")["notFoundExample"].clone();
    assert_eq!(example["httpStatus"], 404);

    for (form, body) in both_wire_forms(&example["body"]) {
        let server = MockServer::start(vec![Reply::Json(404, body.to_string())]);
        let error = client_for(&server)
            .charge_payment_method(PAYMENT_METHOD_ID, &charge_request())
            .expect_err("not found");
        assert!(matches!(error, Error::Api { .. }), "{form}: {error:?}");
        assert_eq!(error.http_status(), Some(404), "{form}");
        assert_eq!(error.code(), Some("PAYMENT_METHOD_NOT_FOUND"), "{form}");
    }
}

#[test]
fn revoke_payment_method_matches_the_contract() {
    let revoke = endpoint("revokePaymentMethod");
    assert_eq!(revoke["method"], "DELETE");
    assert_eq!(
        revoke["path"],
        "/merchant-api/payment-methods/{paymentMethodId}"
    );
    assert_eq!(revoke["httpStatus"], 204);
    assert!(
        strings(&revoke["fields"]).is_empty(),
        "a revoke has no response body"
    );

    let server = MockServer::start(vec![Reply::Json(204, String::new())]);
    client_for(&server)
        .revoke_payment_method(PAYMENT_METHOD_ID)
        .expect("a 204 resolves");
    let recorded = server.only_request();
    assert_eq!(recorded.method, "DELETE");
    assert_eq!(recorded.body, "");
    assert_eq!(recorded.header("Idempotency-Key"), None);
}

#[test]
fn every_revoke_error_example_comes_back_as_a_revoke_error() {
    let revoke = endpoint("revokePaymentMethod");
    let examples = revoke["errorExamples"].as_array().expect("an array");
    assert_eq!(
        examples.len(),
        2,
        "the contract lists two revoke error examples"
    );
    let codes = strings(&contract()["revokeErrorCodes"]);

    for example in examples {
        let http_status = example["httpStatus"].as_u64().expect("a status") as u16;
        let code = example["code"].as_str().expect("a code");
        assert!(codes.contains(&code.to_string()), "{code} is a listed code");

        for (form, body) in both_wire_forms(&example["body"]) {
            let label = format!("{code} ({http_status}, {form})");
            let server = MockServer::start(vec![Reply::Json(http_status, body.to_string())]);
            let error = client_for(&server)
                .revoke_payment_method(PAYMENT_METHOD_ID)
                .expect_err(&label);

            assert!(!error.is_retryable(), "{label}");
            assert_eq!(error.http_status(), Some(http_status), "{label}");
            assert_eq!(error.code(), Some(code), "{label}");
            match error {
                Error::Revoke {
                    status,
                    code: got,
                    message,
                    raw,
                } => {
                    assert_eq!(status, http_status, "{label}");
                    assert_eq!(got, code, "{label}");
                    assert_eq!(message, example["body"]["error"]["message"], "{label}");
                    assert_eq!(raw, body, "{label}: raw is the whole envelope");
                }
                other => panic!("{label}: expected a revoke error, got {other:?}"),
            }
        }
    }
}

#[test]
fn the_revoke_not_found_example_is_a_validation_error_api_error() {
    let example = endpoint("revokePaymentMethod")["notFoundExample"].clone();
    assert_eq!(example["httpStatus"], 404);
    assert_eq!(example["code"], "VALIDATION_ERROR");
    assert_eq!(
        example["body"]["error"]["validationErrors"][0]["field"],
        "id"
    );

    for (form, body) in both_wire_forms(&example["body"]) {
        let server = MockServer::start(vec![Reply::Json(404, body.to_string())]);
        let error = client_for(&server)
            .revoke_payment_method(PAYMENT_METHOD_ID)
            .expect_err("not found");
        assert!(matches!(error, Error::Api { .. }), "{form}: {error:?}");
        assert_eq!(error.http_status(), Some(404), "{form}");
        assert_eq!(error.code(), Some("VALIDATION_ERROR"), "{form}");
    }
}

const REFUNDED_TRANSACTION_ID: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";

fn refund_request() -> RefundRequest {
    RefundRequest::new(idempotency_key("return-7731"))
}

#[test]
fn the_refund_vocabularies_are_exactly_the_contracts() {
    let contract = contract();
    assert_eq!(
        refund_status::ALL.to_vec(),
        strings(&contract["refundStatusVocabulary"]),
        "the SDK refund status vocabulary drifted from the contract"
    );
    assert_eq!(
        refund_error_code::ALL.to_vec(),
        strings(&contract["refundErrorCodes"]),
        "the SDK refund error codes drifted from the contract"
    );
    assert_eq!(
        refund_failure_code::ALL.to_vec(),
        strings(&contract["refundFailureCodes"]),
        "the SDK refund failure codes drifted from the contract"
    );
}

#[test]
fn the_refund_endpoints_match_the_contract() {
    let create = endpoint("createRefund");
    assert_eq!(create["method"], "POST");
    assert_eq!(
        create["path"],
        "/merchant-api/payments/{transactionId}/refunds"
    );
    assert_eq!(create["httpStatus"], 202);
    assert_fields::<Refund>("Refund", &strings(&create["fields"]));

    let read = endpoint("getRefund");
    assert_eq!(read["method"], "GET");
    assert_eq!(
        read["path"],
        "/merchant-api/payments/{transactionId}/refunds/{refundId}"
    );
    assert_eq!(read["httpStatus"], 200);
    assert_fields::<Refund>("Refund", &strings(&read["fields"]));

    // The examples omit null fields, as the wire does, so each carries a subset
    // of the declared fields and never anything else.
    let declared = strings(&create["fields"]);
    for (name, example) in [
        ("partialExample", &create["partialExample"]),
        ("fullExample", &create["fullExample"]),
        ("succeededExample", &read["succeededExample"]),
        ("failedExample", &read["failedExample"]),
    ] {
        for key in keys(&example["data"]) {
            assert!(declared.contains(&key), "{name} carries undeclared {key}");
        }
    }
}

#[test]
fn every_contract_refund_status_round_trips_with_its_terminal_verdict() {
    let data = endpoint("getRefund")["succeededExample"]["data"].clone();
    for value in strings(&contract()["refundStatusVocabulary"]) {
        let mut payload = data.clone();
        payload["status"] = Value::String(value.clone());
        let refund: Refund = serde_json::from_value(payload).expect("deserializes");
        assert_eq!(refund.status, value);
        assert_eq!(
            refund.is_terminal(),
            value == refund_status::SUCCEEDED || value == refund_status::FAILED,
            "{value}"
        );
        assert_eq!(
            refund.is_succeeded(),
            value == refund_status::SUCCEEDED,
            "{value}"
        );
    }
}

#[test]
fn the_create_refund_examples_come_back_as_pending_refunds() {
    let create = endpoint("createRefund");
    for (example, amount, currency) in [
        ("partialExample", Some(2500), "EUR"),
        ("fullExample", None, "HUF"),
    ] {
        for (form, body) in both_wire_forms(&create[example]) {
            let label = format!("{example} ({form})");
            let server = MockServer::start(vec![Reply::Json(202, body.to_string())]);
            let refund = client_for(&server)
                .create_refund(REFUNDED_TRANSACTION_ID, &refund_request())
                .unwrap_or_else(|error| panic!("{label}: {error}"));

            assert_eq!(refund.refund_id, create[example]["data"]["refundId"]);
            assert_eq!(refund.transaction_id, REFUNDED_TRANSACTION_ID);
            assert_eq!(refund.status, refund_status::PENDING, "{label}");
            assert_eq!(refund.amount, amount, "{label}");
            assert_eq!(refund.currency, currency, "{label}");
            assert_eq!(refund.failure_code, None, "{label}");
            assert_eq!(refund.failure_message, None, "{label}");
            assert_eq!(refund.completed_at, None, "{label}");
            assert!(!refund.is_terminal(), "{label}");
            assert_eq!(
                refund.raw, body["data"],
                "{label}: raw is the refund object"
            );
        }
    }
}

#[test]
fn the_get_refund_examples_come_back_succeeded_and_failed() {
    let read = endpoint("getRefund");

    for (form, body) in both_wire_forms(&read["succeededExample"]) {
        let server = MockServer::start(vec![Reply::Json(200, body.to_string())]);
        let refund = client_for(&server)
            .get_refund(
                REFUNDED_TRANSACTION_ID,
                "re_7c1e9a2b4d6f48a0b3c5d7e9f1a2b3c4",
            )
            .unwrap_or_else(|error| panic!("succeeded ({form}): {error}"));
        assert!(refund.is_succeeded(), "{form}");
        assert!(refund.is_terminal(), "{form}");
        assert_eq!(refund.amount, Some(2500), "{form}");
        assert_eq!(refund.currency, "EUR", "{form}");
        assert_eq!(
            refund.completed_at.as_deref(),
            Some("2026-09-26T10:05:40.1200000Z"),
            "{form}"
        );
        assert_eq!(refund.failure(), None, "{form}");
    }

    for (form, body) in both_wire_forms(&read["failedExample"]) {
        let server = MockServer::start(vec![Reply::Json(200, body.to_string())]);
        let refund = client_for(&server)
            .get_refund(
                REFUNDED_TRANSACTION_ID,
                "re_9e3a1c4d6f8b40c2d5e7f9a1b3c4d5e6",
            )
            .unwrap_or_else(|error| panic!("failed ({form}): {error}"));
        assert_eq!(refund.status, refund_status::FAILED, "{form}");
        assert!(refund.is_terminal(), "{form}");
        // A failed refund never carries an amount.
        assert_eq!(refund.amount, None, "{form}");
        assert_eq!(
            refund.failure_code.as_deref(),
            Some(refund_failure_code::REFUND_FAILED),
            "{form}"
        );
        assert_eq!(
            refund.failure(),
            Some(refund_failure_code::REFUND_FAILED),
            "{form}"
        );
        assert_eq!(
            refund.failure_message.as_deref(),
            read["failedExample"]["data"]["failureMessage"].as_str(),
            "{form}"
        );
    }
}

#[test]
fn every_refund_error_example_comes_back_as_an_api_error_with_its_code() {
    let codes = strings(&contract()["refundErrorCodes"]);

    for (route, read) in [("createRefund", false), ("getRefund", true)] {
        let examples = endpoint(route)["errorExamples"].clone();
        let examples = examples.as_array().expect("an array");
        assert!(!examples.is_empty(), "{route} lists error examples");

        for example in examples {
            let http_status = example["httpStatus"].as_u64().expect("a status") as u16;
            let code = example["code"].as_str().expect("a code");
            assert!(codes.contains(&code.to_string()), "{code} is a listed code");
            assert_eq!(example["body"]["error"]["code"], code);

            for (form, body) in both_wire_forms(&example["body"]) {
                let label = format!("{route} {code} ({http_status}, {form})");
                let server = MockServer::start(vec![Reply::Json(http_status, body.to_string())]);
                let client = client_for(&server);
                let error = if read {
                    client.get_refund(
                        REFUNDED_TRANSACTION_ID,
                        "re_7c1e9a2b4d6f48a0b3c5d7e9f1a2b3c4",
                    )
                } else {
                    client.create_refund(REFUNDED_TRANSACTION_ID, &refund_request())
                }
                .expect_err(&label);

                assert!(!error.is_retryable(), "{label}");
                assert_eq!(error.http_status(), Some(http_status), "{label}");
                assert_eq!(error.code(), Some(code), "{label}");
                match error {
                    Error::Api { message, .. } => {
                        assert_eq!(message, example["body"]["error"]["message"], "{label}")
                    }
                    other => panic!("{label}: expected an API error, got {other:?}"),
                }
            }
        }
    }
}
