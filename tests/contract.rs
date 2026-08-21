//! The response contract, pinned against the canonical fixture.
//!
//! `merchant-api-contract.json` next to this file is a byte-identical copy of the
//! one every Dominaite SDK vendors. It is generated from the gateway DTOs; do NOT
//! edit it here to make a test pass - a mismatch means either this crate drifted
//! or the gateway changed, and both are fixed somewhere other than the fixture.
//!
//! What this file asserts: the status vocabulary is exactly the fixture's, each
//! response type deserializes the fixture's example, each type's serde field set
//! is exactly the fixture's field list (no extra, none missing), and every
//! documented session refusal code comes back out of the client as a refusal that
//! keeps its code.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::cell::RefCell;
use std::time::Duration;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};
use serde::forward_to_deserialize_any;
use serde_json::Value;

use dominaite::{status, CheckoutSession, CheckoutStatus, Client, Error, Ping};
use support::{MockServer, Reply};

const FIXTURE: &str = include_str!("merchant-api-contract.json");

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
        .create_checkout_session(&dominaite::CheckoutSessionRequest::new(
            8440,
            "EUR",
            "order-1042",
        ))
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
        .create_checkout_session(&dominaite::CheckoutSessionRequest::new(
            8440,
            "EUR",
            "order-1042",
        ))
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
fn every_contract_refusal_code_survives_as_a_refusal() {
    let codes = strings(&contract()["sessionRefusalErrorCodes"]);
    assert!(!codes.is_empty(), "the contract lists refusal codes");

    // An unlisted code rides along: the gateway can add one (it already has
    // PRIOR_ATTEMPT_FAILED, which this fixture version does not list), and an
    // unknown code must still arrive as a refusal with its code intact rather
    // than as a generic API error the caller cannot branch on.
    for code in codes.iter().map(String::as_str).chain(["A_NEW_CODE"]) {
        let payload =
            format!(r#"{{"success":false,"errorCode":"{code}","errorMessage":"refused"}}"#);
        let server = MockServer::start(vec![Reply::enveloped(&payload)]);

        let error = client_for(&server)
            .create_checkout_session(&dominaite::CheckoutSessionRequest::new(
                8440,
                "EUR",
                "order-1042",
            ))
            .expect_err("a refusal is not a session");

        assert_eq!(error.code(), Some(code), "{code} did not survive");
        assert!(!error.is_retryable(), "{code} must never be blind-retried");
    }
}
