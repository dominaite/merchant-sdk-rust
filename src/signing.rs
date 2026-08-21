//! The request signing recipe, exported so you can pin it against the offline vectors.

use std::fmt;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::client::REDACTED_SECRET;

/// Everything that goes into one request signature.
///
/// Borrowed strings: nothing here is stored, it is hashed and dropped.
// SECURITY: do not derive Serialize. The secret field would be emitted by any
// serde-based logger, which is how the Go SDK leaked it through json.Marshal.
#[derive(Clone, Copy)]
pub struct SignRequest<'a> {
    /// Your API secret (`dms_...`).
    pub secret: &'a str,
    /// Unix SECONDS, exactly as sent in `X-Timestamp`.
    pub timestamp: &'a str,
    /// The HTTP method. Uppercased before signing.
    pub method: &'a str,
    /// The canonical path only: no host, no query string, and never the base URL's
    /// own prefix (the `/api` or `/payments` segment is not part of the signed path).
    pub path: &'a str,
    /// The `Idempotency-Key` header value. Empty string for GET.
    pub idempotency_key: &'a str,
    /// The exact request body that will be sent. Empty string for GET.
    pub body: &'a str,
}

/// Hand-written so a debug-logged signing input cannot leak the secret. A derived
/// one prints it verbatim.
impl fmt::Debug for SignRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignRequest")
            .field("secret", &REDACTED_SECRET)
            .field("timestamp", &self.timestamp)
            .field("method", &self.method)
            .field("path", &self.path)
            .field("idempotency_key", &self.idempotency_key)
            .field("body", &self.body)
            .finish()
    }
}

/// Builds the `X-Signature` value for one request: lowercase hex HMAC-SHA256 over
///
/// ```text
/// "{timestamp}\n{METHOD}\n{path}\n{idempotency_key}\n{sha256hex(body)}"
/// ```
///
/// The idempotency key is INSIDE the signature, so a captured request cannot be
/// replayed with a different key to mint extra sessions. The server rejects
/// timestamps more than 5 minutes off, so keep your server clock on NTP.
///
/// [`Client`](crate::Client) signs for you. This is public so you can pin the recipe
/// against the offline test vectors before calling the live API, and so you can debug
/// an `INVALID_SIGNATURE` without reading this crate's source.
///
/// ```
/// let signature = dominaite::sign_request(dominaite::SignRequest {
///     secret: "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
///     timestamp: "1755302400",
///     method: "POST",
///     path: "/merchant-api/checkout/sessions",
///     idempotency_key: "00000000-0000-4000-8000-000000000001",
///     body: r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042"}"#,
/// });
/// assert_eq!(
///     signature,
///     "8f5fba0b29a8eea81b76a0e6d7119e79ec68f586910f77713b045652e5ce9b74"
/// );
/// ```
pub fn sign_request(request: SignRequest<'_>) -> String {
    let payload = format!(
        "{}\n{}\n{}\n{}\n{}",
        request.timestamp,
        request.method.to_uppercase(),
        request.path,
        request.idempotency_key,
        sha256_hex(request.body),
    );

    // HMAC accepts a key of any length, so this cannot fail.
    let mut mac = Hmac::<Sha256>::new_from_slice(request.secret.as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Lowercase hex SHA-256 of a body. `""` hashes to the well-known empty digest,
/// which is what a GET signs.
pub fn sha256_hex(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    hex::encode(hasher.finalize())
}
