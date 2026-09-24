//! The idempotency key contract: required on every create-style call, never
//! minted by the SDK, and the recommended one derived from the order.

use dominaite::{Error, IdempotencyKey};

fn assert_validation(result: Result<IdempotencyKey, Error>, label: &str) {
    match result {
        Err(Error::Validation { .. }) => {}
        other => panic!("{label}: expected a validation error, got {other:?}"),
    }
}

#[test]
fn the_order_key_is_scope_order_amount_and_uppercased_currency() {
    let key = IdempotencyKey::for_order("a1b2c3d4", "DIM-1001", 3000, "eur").expect("valid");
    assert_eq!(key.as_str(), "a1b2c3d4-DIM-1001-3000-EUR");
    assert_eq!(key.to_string(), "a1b2c3d4-DIM-1001-3000-EUR");
}

/// Reload and back button: the same order at the same total is the same key,
/// so the gateway replays the session it already opened.
#[test]
fn the_same_order_and_amount_always_derive_the_same_key() {
    let first = IdempotencyKey::for_order("a1b2c3d4", "DIM-1001", 3000, "EUR").expect("valid");
    let second = IdempotencyKey::for_order("a1b2c3d4", "DIM-1001", 3000, "eur").expect("valid");
    assert_eq!(first, second);
}

/// Re-pricing: a changed total must never replay the session opened for the
/// old one, and neither may another deployment's order with the same number.
#[test]
fn a_changed_amount_currency_or_scope_derives_a_new_key() {
    let base = IdempotencyKey::for_order("a1b2c3d4", "DIM-1001", 3000, "EUR").expect("valid");
    for (label, other) in [
        (
            "amount",
            IdempotencyKey::for_order("a1b2c3d4", "DIM-1001", 3100, "EUR"),
        ),
        (
            "currency",
            IdempotencyKey::for_order("a1b2c3d4", "DIM-1001", 3000, "BGN"),
        ),
        (
            "scope",
            IdempotencyKey::for_order("ffff0000", "DIM-1001", 3000, "EUR"),
        ),
    ] {
        assert_ne!(
            base,
            other.expect("valid"),
            "a changed {label} kept the key"
        );
    }
}

#[test]
fn the_order_key_rejects_what_the_gateway_would() {
    assert_validation(
        IdempotencyKey::for_order("", "DIM-1001", 3000, "EUR"),
        "empty scope",
    );
    assert_validation(
        IdempotencyKey::for_order("a1b2c3d4", "  ", 3000, "EUR"),
        "blank order id",
    );
    assert_validation(
        IdempotencyKey::for_order("a1b2c3d4", "DIM-1001", 0, "EUR"),
        "zero amount",
    );
    assert_validation(
        IdempotencyKey::for_order("a1b2c3d4", "DIM-1001", -3000, "EUR"),
        "negative amount",
    );
    for currency in ["", "EU", "EURO", "E1R", "€UR"] {
        assert_validation(
            IdempotencyKey::for_order("a1b2c3d4", "DIM-1001", 3000, currency),
            &format!("currency {currency:?}"),
        );
    }
    // The whole key is bounded like any other: 100 characters.
    assert_validation(
        IdempotencyKey::for_order("a1b2c3d4", &"x".repeat(90), 3000, "EUR"),
        "a derived key over 100 characters",
    );
}

#[test]
fn a_caller_key_must_be_non_empty_and_at_most_100_characters() {
    assert_validation(IdempotencyKey::new(""), "empty");
    assert_validation(IdempotencyKey::new("k".repeat(101)), "101 characters");

    let longest = "k".repeat(100);
    assert_eq!(
        IdempotencyKey::new(longest.clone())
            .expect("100 characters fit")
            .as_str(),
        longest
    );
}

/// The shared cross-SDK rule: visible ASCII, 0x21 through 0x7E, nothing else.
#[test]
fn a_caller_key_must_be_visible_ascii() {
    for (label, bad) in [
        ("whitespace only", "   "),
        ("inner space", "order 1042"),
        ("tab", "order\t1042"),
        ("newline", "order-1042\n"),
        ("DEL", "order\u{7f}1042"),
        ("Cyrillic", "заказ-1042"),
        ("accented", "commande-1042-é"),
    ] {
        assert_validation(IdempotencyKey::new(bad), label);
    }

    // Both ends of the range are fine.
    let edges = "!order-1042_~";
    assert_eq!(IdempotencyKey::new(edges).expect("valid").as_str(), edges);
}

/// The helper's output is held to the same rule, so an order id with a space
/// or a non-Latin letter is refused rather than sent.
#[test]
fn the_order_key_is_checked_against_the_same_rule() {
    assert_validation(
        IdempotencyKey::for_order("a1b2c3d4", "order 1042", 3000, "EUR"),
        "space in the order id",
    );
    assert_validation(
        IdempotencyKey::for_order("магазин", "order-1042", 3000, "EUR"),
        "non-ASCII scope",
    );
}
