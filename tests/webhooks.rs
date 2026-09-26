//! Known-answer tests for webhook verification.
//!
//! The vector below is the canonical cross-SDK one: the same secret, timestamp,
//! body and header are pinned by every Dominaite SDK and by the gateway that
//! signs the deliveries. Do NOT reformat the body - it is signed byte for byte,
//! and re-indenting it silently changes what is being tested.
//!
//! The cases are the contract: the vector verifies, a tampered body fails, a
//! wrong secret fails, an out-of-tolerance timestamp fails even with a good MAC,
//! and every header outside the grammar fails as `MalformedSignature` rather
//! than by panicking. The ten shared header vectors live here too.

use dominaite::{verify_webhook, WebhookError, DEFAULT_TOLERANCE_SECS};

const SECRET: &str = "whsec_abababababababababababababababababababababababababababababababab";
const TIMESTAMP: u64 = 1755700000;
const BODY: &str = r#"{"id":"7f9c24e5-1d1f-4c0a-9b6c-2f3a4d5e6f70","type":"payment.succeeded","createdAt":"2026-08-20T14:00:00Z","data":{"transactionId":"0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0","status":"succeeded","previousStatus":"pending","kind":"sale","amount":8440,"grossAmount":8701,"surchargeAmount":261,"currency":"EUR","originalTransactionId":null,"idempotencyKey":"order-123"}}"#;
const HEADER: &str =
    "t=1755700000,v1=5305bcf1302fdaba8f8c19a20c899e916fb4d2a7d8d547c62529ff87c4697b72";

/// A clock inside tolerance of the vector's timestamp.
const NOW: u64 = TIMESTAMP + 10;

fn verify_at(body: &str, header: &str, secret: &str, now: u64) -> Result<(), WebhookError> {
    verify_webhook(body, header, secret, DEFAULT_TOLERANCE_SECS, Some(now))
}

#[test]
fn canonical_vector_verifies() {
    assert_eq!(verify_at(BODY, HEADER, SECRET, NOW), Ok(()));
}

#[test]
fn a_single_tampered_byte_fails() {
    // 8440 -> 8441: one digit, the smallest change an attacker would bother with.
    let tampered = BODY.replace(r#""amount":8440"#, r#""amount":8441"#);
    assert_ne!(tampered, BODY, "the tamper must actually change the body");

    assert_eq!(
        verify_at(&tampered, HEADER, SECRET, NOW),
        Err(WebhookError::SignatureMismatch)
    );
}

#[test]
fn a_wrong_secret_fails() {
    let wrong = "whsec_cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

    assert_eq!(
        verify_at(BODY, HEADER, wrong, NOW),
        Err(WebhookError::SignatureMismatch)
    );
}

#[test]
fn a_stale_timestamp_fails_even_with_a_valid_mac() {
    let now = TIMESTAMP + DEFAULT_TOLERANCE_SECS + 1;

    // Same header and secret that pass in `canonical_vector_verifies`, so the
    // only thing rejecting this delivery is its age.
    assert_eq!(
        verify_at(BODY, HEADER, SECRET, now),
        Err(WebhookError::TimestampOutOfTolerance {
            timestamp: TIMESTAMP,
            now,
            tolerance_secs: DEFAULT_TOLERANCE_SECS,
        })
    );

    // A clock that runs the other way is just as bad.
    let skewed = TIMESTAMP - DEFAULT_TOLERANCE_SECS - 1;
    assert!(matches!(
        verify_at(BODY, HEADER, SECRET, skewed),
        Err(WebhookError::TimestampOutOfTolerance { .. })
    ));

    // The edge itself is inside the window.
    assert_eq!(
        verify_at(BODY, HEADER, SECRET, TIMESTAMP + DEFAULT_TOLERANCE_SECS),
        Ok(())
    );
}

#[test]
fn malformed_headers_fail_without_surprises() {
    let valid_mac = "5305bcf1302fdaba8f8c19a20c899e916fb4d2a7d8d547c62529ff87c4697b72";

    let cases = [
        ("empty", ""),
        ("missing t", &format!("v1={valid_mac}")[..]),
        ("missing v1", "t=1755700000"),
        ("garbage", "not-a-signature"),
        ("no separators", &format!("t1755700000v1{valid_mac}")[..]),
        ("non-numeric t", &format!("t=yesterday,v1={valid_mac}")[..]),
        ("negative t", &format!("t=-1755700000,v1={valid_mac}")[..]),
        ("non-hex v1", "t=1755700000,v1=zzzz"),
        ("short v1", "t=1755700000,v1=5305bcf1"),
        (
            "long v1",
            &format!("t=1755700000,v1={valid_mac}{valid_mac}")[..],
        ),
        ("empty v1", "t=1755700000,v1="),
        ("only a comma", ","),
    ];

    for (name, header) in cases {
        let result = verify_at(BODY, header, SECRET, NOW);
        assert!(
            matches!(result, Err(WebhookError::MalformedSignature { .. })),
            "{name}: expected MalformedSignature, got {result:?}"
        );
    }
}

#[test]
fn unknown_fields_are_ignored_so_a_v2_rollout_does_not_break_v1() {
    let header = format!("{HEADER},v2=deadbeef");

    assert_eq!(verify_at(BODY, &header, SECRET, NOW), Ok(()));
}

/// The ten header vectors from WEBHOOKS-CONTRACT.md, pinned in every SDK suite.
/// They exist because the five verifiers had drifted apart on exactly these
/// shapes (audit A7); keep them byte-identical across the SDKs.
#[test]
fn the_shared_malformed_header_vectors_all_fail() {
    let mac = "5305bcf1302fdaba8f8c19a20c899e916fb4d2a7d8d547c62529ff87c4697b72";
    let upper = mac.to_uppercase();

    let cases = [
        (1, format!("t={TIMESTAMP}")),
        (2, format!("v1={mac}")),
        (3, format!("t={TIMESTAMP},v1={upper}")),
        (4, format!("t={TIMESTAMP},v1={mac},v1={mac}")),
        (5, format!("t={TIMESTAMP},t={TIMESTAMP},v1={mac}")),
        (6, format!("t=,v1=garbage,v1={mac}")),
        (7, format!("t={TIMESTAMP}, v1={mac}")),
        (8, format!("t=+{TIMESTAMP},v1={mac}")),
        (9, "garbage".to_string()),
    ];

    // Collected rather than asserted one at a time, so a regression reports
    // every vector it broke instead of only the first.
    let mut accepted = Vec::new();
    for (number, header) in cases {
        let result = verify_at(BODY, &header, SECRET, NOW);
        if !matches!(result, Err(WebhookError::MalformedSignature { .. })) {
            accepted.push(format!("vector {number} ({header:?}) gave {result:?}"));
        }
    }

    assert!(
        accepted.is_empty(),
        "expected MalformedSignature for every vector: {}",
        accepted.join("; ")
    );
}

#[test]
fn the_shared_unknown_key_vector_verifies() {
    // Vector 10. Unknown keys are reserved for a future scheme version, so they
    // are ignored rather than rejected.
    let header = format!("{HEADER},v9=deadbeef");

    assert_eq!(verify_at(BODY, &header, SECRET, NOW), Ok(()));
}

#[test]
fn a_leading_zero_timestamp_fails_even_with_a_mac_over_the_stripped_value() {
    // `01755700000` parsed as a number and printed back is `1755700000`, which
    // is exactly what the canonical MAC covers. A verifier that reformats `t`
    // before building the signed string accepts this delivery; the raw substring
    // is what the platform signed, so the MAC over `01755700000.` cannot match.
    //
    // The leading zero is grammatical (digits are digits), so this rejects at
    // the MAC rather than at the parser.
    let header = format!(
        "t=0{TIMESTAMP},v1=5305bcf1302fdaba8f8c19a20c899e916fb4d2a7d8d547c62529ff87c4697b72"
    );

    assert_eq!(
        verify_at(BODY, &header, SECRET, NOW),
        Err(WebhookError::SignatureMismatch),
        "a reformatted timestamp must never reach the MAC"
    );
}

#[test]
fn the_mac_is_checked_before_the_timestamp() {
    // A stale delivery with a bad MAC must report the signature failure, not the
    // age: an unsigned request should learn nothing about the tolerance window.
    let stale = TIMESTAMP + DEFAULT_TOLERANCE_SECS + 1;
    let wrong = "whsec_cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

    assert_eq!(
        verify_at(BODY, HEADER, wrong, stale),
        Err(WebhookError::SignatureMismatch)
    );
}

#[test]
fn the_signed_string_joins_timestamp_and_body_with_a_dot() {
    // Moving the dot's worth of data across the boundary must not verify, or the
    // scheme would be ambiguous about where the timestamp ends.
    let shifted = "t=175570000,v1=5305bcf1302fdaba8f8c19a20c899e916fb4d2a7d8d547c62529ff87c4697b72";
    let body_with_stolen_digit = format!("0.{BODY}");

    assert_eq!(
        verify_at(&body_with_stolen_digit, shifted, SECRET, NOW),
        Err(WebhookError::SignatureMismatch)
    );
}

/// A timing-safe comparison cannot be observed from the outside: a test can only
/// watch a wrong MAC get rejected, which a byte-by-byte `==` does too, just with
/// a runtime that leaks how far it got. Rust has no way to monkeypatch the
/// comparison at runtime either, so the property is pinned where it lives - in
/// the source.
///
/// `Mac::verify_slice` is the hmac crate's constant-time check (it goes through
/// subtle's `ConstantTimeEq`). Anything that compares the MAC bytes, or their
/// hex, with `==` or `!=` is the bug this test exists to catch.
#[test]
fn the_mac_comparison_stays_constant_time() {
    let webhooks = read_crate_source("src/webhooks.rs");

    assert!(
        webhooks.contains("hmac.verify_slice(&mac)"),
        "src/webhooks.rs no longer verifies the MAC with hmac's constant-time \
         verify_slice; a hand-rolled comparison leaks the MAC one byte at a time"
    );

    // The signing side has no comparison at all, and must not grow one.
    for path in ["src/webhooks.rs", "src/signing.rs"] {
        let source = read_crate_source(path);
        for (number, line) in source.lines().enumerate() {
            // Prose talks about signatures constantly; only code counts.
            let code = line.split("//").next().unwrap_or("");
            let compares = code.contains("==") || code.contains("!=");
            let touches_the_mac = ["mac", "signature", "digest", "finalize", "hex::encode"]
                .iter()
                .any(|name| code.contains(name));

            assert!(
                !(compares && touches_the_mac),
                "{path}:{} compares MAC material directly: {}\n\
                 Use hmac's verify_slice (constant-time) instead of == or !=.",
                number + 1,
                code.trim()
            );
        }
    }
}

fn read_crate_source(relative_path: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("could not read {}: {error}", path.display());
    })
}

// --- Parsing the envelope ---------------------------------------------------
//
// Parsing happens only after verification, and it must accept both the current
// envelope (apiVersion, data.sequence) and payloads from servers that predate
// those fields.

use dominaite::WebhookEvent;

const AGREEMENT_EVENT: &str = r#"{"id":"0b6f2c1e-8a41-4f7d-9c3e-5d2a1b0c9e8f","type":"agreement.past_due","apiVersion":"2026-09-25","createdAt":"2026-09-25T10:00:00Z","data":{"id":"agr_0123456789abcdef0123456789abcdef","planId":"plan_1","customerReference":"cust-7","storedPaymentMethodId":"pm_0123456789abcdef0123456789abcdef","status":"past_due","previousStatus":"active","amount":2500,"currency":"EUR","intervalUnit":"month","intervalCount":1,"periodCount":null,"trialDays":0,"nextChargeAt":"2026-09-24T00:00:00Z","activatedAt":"2026-08-24T00:00:00Z","cancelledAt":null,"version":4,"sequence":3}}"#;

const CHARGE_EVENT: &str = r#"{"id":"5a4b3c2d-1e0f-4a9b-8c7d-6e5f4a3b2c1d","type":"charge.retrying","apiVersion":"2026-09-25","createdAt":"2026-09-24T00:00:05Z","data":{"chargeId":"ch_33333333333343338333333333333333","transactionId":"33333333-3333-4333-8333-333333333333","storedPaymentMethodId":"pm_0123456789abcdef0123456789abcdef","agreementId":"agr_0123456789abcdef0123456789abcdef","customerReference":"cust-7","outcome":"retrying","periodNumber":2,"attemptNumber":1,"amount":2500,"currency":"EUR","paymentMethod":{"brand":"visa","last4":"4242"},"orderReference":null,"description":null,"declineClass":"soft_funds","declineCode":"51","nextAttemptAt":"2026-09-25T00:00:00Z","nextChargeAt":null,"sequence":7}}"#;

#[test]
fn the_canonical_vector_parses_as_an_old_envelope_without_api_version() {
    // The signed vector predates apiVersion: it must still verify AND parse.
    assert_eq!(verify_at(BODY, HEADER, SECRET, NOW), Ok(()));
    let event = WebhookEvent::parse(BODY).expect("old envelope parses");

    assert_eq!(event.id, "7f9c24e5-1d1f-4c0a-9b6c-2f3a4d5e6f70");
    assert_eq!(event.event_type, "payment.succeeded");
    assert_eq!(event.created_at, "2026-08-20T14:00:00Z");
    assert_eq!(event.api_version, None);
    assert_eq!(event.sequence(), None);
    assert_eq!(event.data["amount"], 8440);
}

#[test]
fn an_envelope_with_api_version_parses() {
    let body = BODY.replacen(
        r#""type":"payment.succeeded","#,
        r#""type":"payment.succeeded","apiVersion":"2026-09-25","#,
        1,
    );
    let event: WebhookEvent = serde_json::from_str(&body).expect("envelope parses");

    assert_eq!(event.api_version.as_deref(), Some("2026-09-25"));
    assert_eq!(event.event_type, "payment.succeeded");
    // payment.* events carry no sequence.
    assert_eq!(event.sequence(), None);
}

#[test]
fn an_agreement_event_carries_sequence_and_created_at() {
    let event: WebhookEvent = serde_json::from_str(AGREEMENT_EVENT).expect("parses");

    assert_eq!(event.event_type, "agreement.past_due");
    assert_eq!(event.api_version.as_deref(), Some("2026-09-25"));
    assert_eq!(event.created_at, "2026-09-25T10:00:00Z");
    assert_eq!(event.sequence(), Some(3));
    assert_eq!(event.data["id"], "agr_0123456789abcdef0123456789abcdef");
}

#[test]
fn a_charge_event_carries_sequence_and_created_at() {
    let event: WebhookEvent = serde_json::from_str(CHARGE_EVENT).expect("parses");

    assert_eq!(event.event_type, "charge.retrying");
    assert_eq!(event.created_at, "2026-09-24T00:00:05Z");
    assert_eq!(event.sequence(), Some(7));
    assert_eq!(
        event.data["agreementId"],
        "agr_0123456789abcdef0123456789abcdef"
    );
    assert_eq!(event.data["periodNumber"], 2);
}

#[test]
fn agreement_and_charge_events_from_before_the_counter_still_parse() {
    for current in [AGREEMENT_EVENT, CHARGE_EVENT] {
        let old = current
            .replacen(r#""apiVersion":"2026-09-25","#, "", 1)
            .replace(r#","sequence":3}"#, "}")
            .replace(r#","sequence":7}"#, "}");
        assert!(!old.contains("sequence") && !old.contains("apiVersion"));

        let event: WebhookEvent = serde_json::from_str(&old).expect("old payload parses");
        assert_eq!(event.api_version, None);
        assert_eq!(event.sequence(), None);
        assert!(!event.created_at.is_empty());
    }
}

#[test]
fn a_zero_sequence_is_reported_as_zero_not_absent() {
    // 0 means "recorded before the counter existed", which is older than any
    // positive number. It must not collapse into None.
    let body = AGREEMENT_EVENT.replace(r#""sequence":3"#, r#""sequence":0"#);
    let event: WebhookEvent = serde_json::from_str(&body).expect("parses");
    assert_eq!(event.sequence(), Some(0));
}

// --- storedPaymentMethod on payment.* events --------------------------------

use dominaite::{retired_reason, stored_payment_method_status};

/// The canonical payment.succeeded body with `tail` appended inside `data`.
fn payment_event_with(tail: &str) -> String {
    BODY.replacen(
        r#""idempotencyKey":"order-123"}"#,
        &format!(r#""idempotencyKey":"order-123"{tail}}}"#),
        1,
    )
}

#[test]
fn a_payment_event_carries_the_stored_payment_method() {
    let body = payment_event_with(
        r#","storedPaymentMethod":{"id":"pm_0123456789abcdef0123456789abcdef","brand":"visa","last4":"4242","expiryMonth":12,"expiryYear":2030,"status":"active","retiredReason":null}"#,
    );
    let event = WebhookEvent::parse(&body).expect("parses");

    let method = event
        .stored_payment_method()
        .expect("a stored payment method");
    assert_eq!(method.id, "pm_0123456789abcdef0123456789abcdef");
    assert_eq!(method.brand.as_deref(), Some("visa"));
    assert_eq!(method.last4.as_deref(), Some("4242"));
    assert_eq!(method.expiry_month, Some(12));
    assert_eq!(method.expiry_year, Some(2030));
    assert_eq!(method.status, stored_payment_method_status::ACTIVE);
    assert_eq!(method.retired_reason, None);
    assert!(method.is_chargeable());
}

#[test]
fn a_payment_event_without_a_saved_card_has_no_stored_payment_method() {
    // Absent (the canonical vector, and how the gateway omits nulls) and an
    // explicit null read the same.
    for body in [
        BODY.to_string(),
        payment_event_with(r#","storedPaymentMethod":null"#),
    ] {
        let event = WebhookEvent::parse(&body).expect("parses");
        assert_eq!(event.stored_payment_method(), None, "{body}");
    }
}

#[test]
fn a_retired_card_on_a_payment_event_reads_its_reason_and_is_not_chargeable() {
    let body = payment_event_with(
        r#","storedPaymentMethod":{"id":"pm_0123456789abcdef0123456789abcdef","brand":"visa","last4":"4242","expiryMonth":12,"expiryYear":2030,"status":"retired","retiredReason":"hard_decline"}"#,
    );
    let event = WebhookEvent::parse(&body).expect("parses");

    let method = event
        .stored_payment_method()
        .expect("a stored payment method");
    assert_eq!(method.status, stored_payment_method_status::RETIRED);
    assert_eq!(
        method.retired_reason.as_deref(),
        Some(retired_reason::HARD_DECLINE)
    );
    assert!(!method.is_chargeable());
}

#[test]
fn a_charge_event_names_its_card_by_id_not_by_object() {
    let event: WebhookEvent = serde_json::from_str(CHARGE_EVENT).expect("parses");
    assert_eq!(event.stored_payment_method(), None);
    assert_eq!(
        event.data["storedPaymentMethodId"],
        "pm_0123456789abcdef0123456789abcdef"
    );
}

// --- Typed payment.* data ---------------------------------------------------
//
// Webhooks are serialized with every null spelled out, unlike the merchant API
// responses, so a payment event carries explicit nulls for everything unset.

const FULL_PAYMENT_EVENT: &str = r#"{"id":"2c1d0e9f-8a7b-4c6d-9e5f-4a3b2c1d0e9f","type":"payment.succeeded","apiVersion":"2026-09-25","createdAt":"2026-09-26T10:00:00Z","data":{"transactionId":"0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0","status":"succeeded","previousStatus":"pending","kind":"sale","amount":8440,"grossAmount":8701,"surchargeAmount":261,"currency":"EUR","paymentMethod":"wallet","walletType":"apple_pay","originalTransactionId":null,"idempotencyKey":"order-123","orderReference":"order-123","orderId":"dom_9a8b7c6d5e4f","description":"Two tickets","paymentMethodBrand":"visa","paymentMethodLast4":"4242","storedPaymentMethod":{"id":"pm_0123456789abcdef0123456789abcdef","brand":"visa","last4":"4242","expiryMonth":12,"expiryYear":2030,"status":"active","retiredReason":null}}}"#;

const NULL_PAYMENT_EVENT: &str = r#"{"id":"3d2e1f0a-9b8c-4d7e-8f6a-5b4c3d2e1f0a","type":"payment.refunded","apiVersion":"2026-09-25","createdAt":"2026-09-26T11:00:00Z","data":{"transactionId":"9f8e7d6c-5b4a-4938-8271-6a5b4c3d2e1f","status":"refunded","previousStatus":null,"kind":null,"amount":2500,"grossAmount":2500,"surchargeAmount":null,"currency":"EUR","paymentMethod":null,"walletType":null,"originalTransactionId":"0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0","idempotencyKey":null,"orderReference":"order-123","orderId":null,"description":null,"paymentMethodBrand":null,"paymentMethodLast4":null,"storedPaymentMethod":null}}"#;

#[test]
fn a_payment_event_reads_every_gateway_field() {
    let event = WebhookEvent::parse(FULL_PAYMENT_EVENT).expect("parses");
    let data = event.payment_data().expect("a payment event");

    assert_eq!(data.transaction_id, "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0");
    assert_eq!(data.status, "succeeded");
    assert_eq!(data.previous_status.as_deref(), Some("pending"));
    assert_eq!(data.kind.as_deref(), Some("sale"));
    assert_eq!(data.amount, Some(8440));
    assert_eq!(data.gross_amount, Some(8701));
    assert_eq!(data.surcharge_amount, Some(261));
    assert_eq!(data.currency.as_deref(), Some("EUR"));
    assert_eq!(data.payment_method.as_deref(), Some("wallet"));
    assert_eq!(data.wallet_type.as_deref(), Some("apple_pay"));
    assert_eq!(data.original_transaction_id, None);
    assert_eq!(data.idempotency_key.as_deref(), Some("order-123"));
    assert_eq!(data.order_reference.as_deref(), Some("order-123"));
    assert_eq!(data.order_id.as_deref(), Some("dom_9a8b7c6d5e4f"));
    assert_eq!(data.description.as_deref(), Some("Two tickets"));
    assert_eq!(data.payment_method_brand.as_deref(), Some("visa"));
    assert_eq!(data.payment_method_last4.as_deref(), Some("4242"));
    let method = data.stored_payment_method.expect("a stored card");
    assert_eq!(method.id, "pm_0123456789abcdef0123456789abcdef");
    assert!(method.is_chargeable());
}

#[test]
fn explicit_nulls_on_a_payment_event_read_as_none() {
    let event = WebhookEvent::parse(NULL_PAYMENT_EVENT).expect("parses");
    let data = event.payment_data().expect("a payment event");

    assert_eq!(data.status, "refunded");
    assert_eq!(data.amount, Some(2500));
    assert_eq!(
        data.original_transaction_id.as_deref(),
        Some("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0")
    );
    assert_eq!(data.previous_status, None);
    assert_eq!(data.kind, None);
    assert_eq!(data.surcharge_amount, None);
    assert_eq!(data.payment_method, None);
    assert_eq!(data.wallet_type, None);
    assert_eq!(data.idempotency_key, None);
    assert_eq!(data.order_id, None);
    assert_eq!(data.description, None);
    assert_eq!(data.payment_method_brand, None);
    assert_eq!(data.payment_method_last4, None);
    assert_eq!(data.stored_payment_method, None);
    assert_eq!(event.stored_payment_method(), None);
}

#[test]
fn the_canonical_vector_reads_as_typed_payment_data() {
    // Predates the correlation fields and storedPaymentMethod: still parses.
    let data = WebhookEvent::parse(BODY)
        .expect("parses")
        .payment_data()
        .expect("a payment event");
    assert_eq!(data.amount, Some(8440));
    assert_eq!(data.original_transaction_id, None);
    assert_eq!(data.order_reference, None);
    assert_eq!(data.stored_payment_method, None);
}

#[test]
fn only_payment_events_have_payment_data() {
    for body in [AGREEMENT_EVENT, CHARGE_EVENT] {
        let event = WebhookEvent::parse(body).expect("parses");
        assert_eq!(event.payment_data(), None, "{}", event.event_type);
    }
}
