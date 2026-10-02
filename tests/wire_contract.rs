//! Pins this crate's hardcoded enumerations against the gateway's live contract.
//!
//! `merchant-api-wire-contract.json` next to this file is the machine-relevant
//! projection of the gateway's `GET /merchant-api/integration/contract`, refreshed
//! by `.github/workflows/contract-drift.yml`. When one of these fails the gateway
//! moved: fix the crate and release, never the fixture.

use serde_json::Value;

use dominaite::{status, wallet_type, CheckoutStatus};

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
fn the_status_type_carries_every_wallet_reporting_field_as_an_optional_string() {
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
        assert_eq!(field["type"], "string", "{} type", field["path"]);
        assert_eq!(
            field["required"],
            Value::Bool(false),
            "{} must be optional",
            field["path"]
        );
    }

    // Each path lands on its typed field, and absent reads as None.
    let mut body = serde_json::json!({ "transactionId": "t", "status": status::SUCCEEDED });
    for path in &paths {
        body[*path] = Value::String(format!("{path}-value"));
    }
    let parsed: CheckoutStatus = serde_json::from_value(body).expect("deserializes");
    assert_eq!(
        parsed.payment_method.as_deref(),
        Some("paymentMethod-value")
    );
    assert_eq!(parsed.wallet_type.as_deref(), Some("walletType-value"));

    let bare: CheckoutStatus =
        serde_json::from_value(serde_json::json!({ "transactionId": "t", "status": "pending" }))
            .expect("deserializes");
    assert_eq!(bare.payment_method, None);
    assert_eq!(bare.wallet_type, None);
}

#[test]
fn the_contract_still_lists_this_sdk() {
    let wire = wire();
    assert!(
        strings(&wire["sdks"]).contains(&"rust"),
        "sdks: {}",
        wire["sdks"]
    );
}
