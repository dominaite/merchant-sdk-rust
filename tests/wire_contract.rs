//! Pins this crate's hardcoded enumerations against the gateway's live contract.
//!
//! `merchant-api-wire-contract.json` next to this file is the machine-relevant
//! projection of the gateway's `GET /merchant-api/integration/contract`, refreshed
//! by `.github/workflows/contract-drift.yml`. When one of these fails the gateway
//! moved: fix the crate and release, never the fixture.

use serde_json::Value;

use dominaite::{status, wallet_type};

const WIRE: &str = include_str!("merchant-api-wire-contract.json");

fn wire() -> Value {
    serde_json::from_str(WIRE).expect("merchant-api-wire-contract.json is valid JSON")
}

fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item.as_str().expect("string"))
        .collect()
}

#[test]
fn status_vocabulary_matches_the_gateway_in_order() {
    let wire = wire();
    assert_eq!(status::ALL.to_vec(), strings(&wire["statuses"]));
}

#[test]
fn wallet_types_match_the_gateway_in_order() {
    let wire = wire();
    assert_eq!(
        wallet_type::ALL.to_vec(),
        strings(&wire["wallets"]["walletTypes"])
    );
}

#[test]
fn wallet_reporting_fields_are_payment_method_and_wallet_type_both_optional() {
    let wire = wire();
    let fields = wire["wallets"]["reportingFields"]
        .as_array()
        .expect("wallets.reportingFields is an array");

    let paths: Vec<&str> = fields
        .iter()
        .map(|field| field["path"].as_str().expect("path is a string"))
        .collect();
    assert_eq!(paths, ["paymentMethod", "walletType"]);

    for field in fields {
        assert_eq!(
            field["required"],
            Value::Bool(false),
            "{} must be optional",
            field["path"]
        );
    }
}

#[test]
fn the_contract_still_lists_this_sdk() {
    let wire = wire();
    assert!(strings(&wire["sdks"]).contains(&"rust"), "sdks: {}", wire["sdks"]);
}
