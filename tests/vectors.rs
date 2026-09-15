//! Known-answer tests for the signing recipe.
//!
//! Both vectors are shared with the gateway (the source of the recipe) and with
//! the dashboard's Website integration tab, which pins them with real crypto in
//! its own suite. If either of these fails, nothing else in this crate matters:
//! every live call will come back `INVALID_SIGNATURE`.

use dominaite::{sha256_hex, sign_request, SignRequest};

const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const SESSIONS_PATH: &str = "/merchant-api/checkout/sessions";

#[test]
fn post_vector_reproduces_byte_for_byte() {
    let body = r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042"}"#;

    assert_eq!(
        sha256_hex(body),
        "aa3edd72cd1829f4e053abb048b08c1ae91c2d67b08955997c4b6c4dab4f98ff"
    );

    let signature = sign_request(SignRequest {
        secret: SECRET,
        timestamp: "1755302400",
        method: "POST",
        path: SESSIONS_PATH,
        idempotency_key: "00000000-0000-4000-8000-000000000001",
        body,
    });

    assert_eq!(
        signature,
        "8f5fba0b29a8eea81b76a0e6d7119e79ec68f586910f77713b045652e5ce9b74"
    );
}

#[test]
fn get_vector_signs_empty_idempotency_key_and_empty_body() {
    // The payload is still FIVE lines here. A dropped separator is the classic bug.
    assert_eq!(
        sha256_hex(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );

    let signature = sign_request(SignRequest {
        secret: SECRET,
        timestamp: "1755302400",
        method: "GET",
        path: &format!("{SESSIONS_PATH}/00000000-0000-4000-8000-000000000002"),
        idempotency_key: "",
        body: "",
    });

    assert_eq!(
        signature,
        "70002896ec8411efb7754de6c49c2fd6f35bb2d001966978a2f573de1914e68d"
    );
}

#[test]
fn method_is_uppercased_before_signing() {
    let lowercase = sign_request(SignRequest {
        secret: SECRET,
        timestamp: "1755302400",
        method: "post",
        path: SESSIONS_PATH,
        idempotency_key: "00000000-0000-4000-8000-000000000001",
        body: r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042"}"#,
    });

    assert_eq!(
        lowercase,
        "8f5fba0b29a8eea81b76a0e6d7119e79ec68f586910f77713b045652e5ce9b74"
    );
}

#[test]
fn the_idempotency_key_is_inside_the_signature() {
    let signed = |key: &str| {
        sign_request(SignRequest {
            secret: SECRET,
            timestamp: "1755302400",
            method: "POST",
            path: SESSIONS_PATH,
            idempotency_key: key,
            body: r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042"}"#,
        })
    };

    assert_ne!(
        signed("00000000-0000-4000-8000-000000000001"),
        signed("00000000-0000-4000-8000-000000000002"),
        "swapping the idempotency key must change the signature, or a captured \
         request could be replayed with a fresh key to mint extra sessions"
    );
}

// Stored-payment-method vectors, same secret and timestamp. The charge vector is
// the only POST besides sessions and the only one whose canonical path carries a
// resource id; the revoke vector pins that DELETE signs an empty key and an
// empty body exactly like GET. Shared byte-for-byte with the gateway's
// MerchantApiRequestAuthenticator tests.
const PAYMENT_METHODS_PATH: &str = "/merchant-api/payment-methods";
const PAYMENT_METHOD_ID: &str = "pm_0123456789abcdef0123456789abcdef";

#[test]
fn charge_vector_signs_a_post_with_a_body_and_a_key_on_the_payment_methods_path() {
    let body = r#"{"amount":2500,"currency":"EUR","orderReference":"order-1043"}"#;

    assert_eq!(
        sha256_hex(body),
        "641a0d2b08f88ebc458dca49410dede0a166359a5030bff5c977e507f13ab828"
    );

    let signature = sign_request(SignRequest {
        secret: SECRET,
        timestamp: "1755302400",
        method: "POST",
        path: &format!("{PAYMENT_METHODS_PATH}/{PAYMENT_METHOD_ID}/charges"),
        idempotency_key: "00000000-0000-4000-8000-000000000003",
        body,
    });

    assert_eq!(
        signature,
        "9ce9f54efa2533a46aa4493b97b56aeb657f41d6a18f1c008c7fd412029aebf9"
    );
}

#[test]
fn revoke_vector_signs_a_delete_with_an_empty_key_and_an_empty_body() {
    let path = format!("{PAYMENT_METHODS_PATH}/{PAYMENT_METHOD_ID}");
    let signature = sign_request(SignRequest {
        secret: SECRET,
        timestamp: "1755302400",
        method: "DELETE",
        path: &path,
        idempotency_key: "",
        body: "",
    });

    assert_eq!(
        signature,
        "9330100343c4b820504890a09829a193d5815ca39e92160fdfc13d320a802a02"
    );

    // Same recipe as the GET vector: only the method moved, so the signature
    // must move too.
    let as_get = sign_request(SignRequest {
        secret: SECRET,
        timestamp: "1755302400",
        method: "GET",
        path: &path,
        idempotency_key: "",
        body: "",
    });
    assert_ne!(signature, as_get, "the method is inside the signature");
}
