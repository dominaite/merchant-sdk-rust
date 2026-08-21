//! Verifying inbound webhook deliveries.
//!
//! Dominaite signs every webhook POST with the endpoint's `whsec_` secret. Verify
//! the signature BEFORE you parse the body or trust a single field in it: an
//! unverified webhook is just an unauthenticated stranger POSTing JSON at your
//! server.
//!
//! Deliveries are AT-LEAST-ONCE. Dedupe on the envelope's `id`, respond 2xx fast,
//! and queue the real work instead of doing it inline.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::error::Error as StdError;
use std::fmt;

use crate::client::unix_seconds;

/// The default clock skew allowed between the signature's timestamp and your
/// server's clock, in seconds. Matches the server's own tolerance.
pub const DEFAULT_TOLERANCE_SECS: u64 = 300;

/// Why a webhook failed verification.
///
/// Every variant means the same thing operationally: do not process the delivery.
/// They are split apart so you can tell a misconfigured secret from a clock
/// problem in your logs. Match on the variant rather than on the message.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebhookError {
    /// The signature header was not `t={digits},v1={64 lowercase hex}`. The
    /// header never reached the crypto: it was missing a field, repeated one,
    /// carried whitespace, held a `t` that was not raw ASCII digits, or held a
    /// `v1` that was not exactly 64 lowercase hex characters.
    ///
    /// In production this usually means the wrong header was read off the
    /// request, not an attack.
    MalformedSignature {
        /// What could not be parsed.
        message: String,
    },

    /// The header parsed, but the MAC did not match. Either the body was
    /// modified in flight, or you verified against the wrong endpoint's secret.
    ///
    /// Body mismatches are far more often a framework that re-serialized the
    /// JSON before you got to it than an actual attacker. Verify the RAW bytes.
    SignatureMismatch,

    /// The MAC was valid but the timestamp is too far from your clock, so this
    /// is a replay of a genuine delivery (or your server clock has drifted off
    /// NTP).
    TimestampOutOfTolerance {
        /// The `t` value carried by the signature, in unix seconds.
        timestamp: u64,
        /// The current time this check compared against, in unix seconds.
        now: u64,
        /// The tolerance that was applied, in seconds.
        tolerance_secs: u64,
    },
}

impl fmt::Display for WebhookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebhookError::MalformedSignature { message } => {
                write!(f, "malformed webhook signature: {message}")
            }
            WebhookError::SignatureMismatch => {
                write!(f, "webhook signature does not match the payload")
            }
            WebhookError::TimestampOutOfTolerance {
                timestamp,
                now,
                tolerance_secs,
            } => write!(
                f,
                "webhook timestamp {timestamp} is outside the {tolerance_secs}s \
                 tolerance around {now}"
            ),
        }
    }
}

impl StdError for WebhookError {}

/// Verifies one webhook delivery: `Ok(())` means the payload is authentic and
/// fresh, and only then is it safe to parse.
///
/// - `payload` is the RAW request body, byte for byte as received. Do not
///   pretty-print it, do not round-trip it through a JSON parser, do not let a
///   framework re-serialize it. One changed byte is one failed signature.
/// - `signature_header` is the `X-Webhook-Signature` value
///   (`t={digits},v1={64 lowercase hex}`). Unknown keys are ignored; anything
///   else outside that grammar is rejected.
/// - `secret` is that endpoint's `whsec_...` secret, used as UTF-8 key bytes.
/// - `tolerance_secs` bounds `|now - t|`. [`DEFAULT_TOLERANCE_SECS`] mirrors the
///   server. Passing `0` accepts only the exact second, which is not what you
///   want in production.
/// - `now` is unix seconds, for tests and pinned vectors. `None` reads the
///   system clock.
///
/// The comparison is constant-time, and the MAC is checked before the timestamp
/// so an unsigned request can never learn anything about your tolerance window.
///
/// ```
/// use dominaite::{verify_webhook, DEFAULT_TOLERANCE_SECS};
///
/// # fn main() -> Result<(), dominaite::WebhookError> {
/// let secret = "whsec_abababababababababababababababababababababababababababababababab";
/// let payload = r#"{"id":"7f9c24e5-1d1f-4c0a-9b6c-2f3a4d5e6f70","type":"payment.succeeded","createdAt":"2026-08-20T14:00:00Z","data":{"transactionId":"0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0","status":"succeeded","previousStatus":"pending","kind":"sale","amount":8440,"grossAmount":8701,"surchargeAmount":261,"currency":"EUR","originalTransactionId":null,"idempotencyKey":"order-123"}}"#;
/// let header = "t=1755700000,v1=5305bcf1302fdaba8f8c19a20c899e916fb4d2a7d8d547c62529ff87c4697b72";
///
/// // In your handler, pass `None` for `now`; the vector below pins a fixed clock.
/// verify_webhook(payload, header, secret, DEFAULT_TOLERANCE_SECS, Some(1755700010))?;
/// # Ok(())
/// # }
/// ```
pub fn verify_webhook(
    payload: &str,
    signature_header: &str,
    secret: &str,
    tolerance_secs: u64,
    now: Option<u64>,
) -> Result<(), WebhookError> {
    let SignatureHeader { timestamp, mac } = parse_signature_header(signature_header)?;

    // HMAC accepts a key of any length, so this cannot fail.
    let mut hmac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts keys of any length");
    hmac.update(format!("{timestamp}.{payload}").as_bytes());
    hmac.verify_slice(&mac)
        .map_err(|_| WebhookError::SignatureMismatch)?;

    // Only authentic deliveries get this far, so a tolerance failure is a replay
    // rather than a probe.
    //
    // `timestamp` is raw digits, so the only way this parse fails is a value too
    // large for u64. Saturating puts such a delivery outside every tolerance
    // window, which is the answer we want anyway.
    let seconds = timestamp.parse::<u64>().unwrap_or(u64::MAX);
    let now = now.unwrap_or_else(unix_seconds);
    if now.abs_diff(seconds) > tolerance_secs {
        return Err(WebhookError::TimestampOutOfTolerance {
            timestamp: seconds,
            now,
            tolerance_secs,
        });
    }

    Ok(())
}

struct SignatureHeader<'a> {
    /// The RAW `t` substring off the wire. This, and never a reparsed number,
    /// is what goes into the signed string.
    timestamp: &'a str,
    mac: Vec<u8>,
}

/// Splits `t={digits},v1={64 lowercase hex}` into its parts.
///
/// The grammar is closed: comma-separated `key=value` elements, no whitespace
/// anywhere, exactly one `t` and one `v1`, and an element without `=` rejects
/// the whole header. Keys are matched by name rather than by position, so a
/// future scheme that reorders them still verifies, and unknown keys are
/// ignored so a `v2` can roll out alongside `v1`.
fn parse_signature_header(header: &str) -> Result<SignatureHeader<'_>, WebhookError> {
    if header.contains(|c: char| c.is_ascii_whitespace()) {
        return Err(malformed("header contains whitespace"));
    }

    let mut timestamp = None;
    let mut mac = None;

    for field in header.split(',') {
        let Some((name, value)) = field.split_once('=') else {
            return Err(malformed(format!(
                "field {:?} is not name=value",
                truncate(field)
            )));
        };

        match name {
            "t" => {
                if timestamp.is_some() {
                    return Err(malformed("repeated t field"));
                }
                // One or more raw ASCII digits, nothing else: no sign, no
                // leading zeros stripped, no reformatting. Parsing this into a
                // number and printing it back would silently accept `+1755700000`
                // and `01755700000` as the timestamp the platform signed, and
                // the other SDKs reject both.
                if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(malformed(format!(
                        "timestamp {:?} is not unix seconds",
                        truncate(value)
                    )));
                }
                timestamp = Some(value);
            }
            "v1" => {
                if mac.is_some() {
                    return Err(malformed("repeated v1 field"));
                }
                if value.len() != 64 {
                    return Err(malformed(format!(
                        "v1 signature is {} characters, expected 64",
                        value.len()
                    )));
                }
                // Uppercase hex decodes fine but the platform never emits it,
                // so accepting it would widen the accept set for nothing.
                if !value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                    return Err(malformed("v1 signature is not lowercase hex"));
                }
                let decoded = hex::decode(value)
                    .map_err(|_| malformed("v1 signature is not lowercase hex"))?;
                mac = Some(decoded);
            }
            // A scheme version we do not know about yet.
            _ => {}
        }
    }

    match (timestamp, mac) {
        (Some(timestamp), Some(mac)) => Ok(SignatureHeader { timestamp, mac }),
        (None, Some(_)) => Err(malformed("no t field")),
        (Some(_), None) => Err(malformed("no v1 field")),
        (None, None) => Err(malformed("no t or v1 field")),
    }
}

fn malformed(message: impl Into<String>) -> WebhookError {
    WebhookError::MalformedSignature {
        message: message.into(),
    }
}

/// Keeps an attacker-controlled header out of your logs at full length.
fn truncate(value: &str) -> String {
    const LIMIT: usize = 32;
    if value.len() <= LIMIT {
        return value.to_string();
    }
    let mut end = LIMIT;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}
