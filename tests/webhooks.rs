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
    let header = format!("t=0{TIMESTAMP},v1=5305bcf1302fdaba8f8c19a20c899e916fb4d2a7d8d547c62529ff87c4697b72");

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
