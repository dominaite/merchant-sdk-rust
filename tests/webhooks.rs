//! Known-answer tests for webhook verification.
//!
//! The vector below is the canonical cross-SDK one: the same secret, timestamp,
//! body and header are pinned by every Dominaite SDK and by the gateway that
//! signs the deliveries. Do NOT reformat the body - it is signed byte for byte,
//! and re-indenting it silently changes what is being tested.
//!
//! The five cases are the contract: the vector verifies, a tampered body fails,
//! a wrong secret fails, an out-of-tolerance timestamp fails even with a good
//! MAC, and malformed headers fail as `MalformedSignature` rather than by
//! panicking.

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
