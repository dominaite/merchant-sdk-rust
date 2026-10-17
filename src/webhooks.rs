//! Verifying inbound webhook deliveries.
//!
//! Dominaite signs every webhook POST with the endpoint's `whsec_` secret. Verify
//! the signature BEFORE you parse the body or trust a single field in it: an
//! unverified webhook is just an unauthenticated stranger POSTing JSON at your
//! server.
//!
//! Deliveries are AT-LEAST-ONCE. Dedupe on the envelope's `id`, respond 2xx fast,
//! and queue the real work instead of doing it inline.
//!
//! Deliveries can also arrive OUT OF ORDER. [`WebhookEvent::sequence`] says how to
//! order `agreement.*` and `charge.*` events.

use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use std::error::Error as StdError;
use std::fmt;

use crate::client::unix_seconds;
use crate::types::StoredPaymentMethod;

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
    let mut hmac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
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

/// A parsed webhook envelope: `{ id, type, apiVersion, createdAt, data }`.
///
/// Parse it only after [`verify_webhook`] returned `Ok(())`, and parse the same
/// raw body you verified:
///
/// ```
/// use dominaite::WebhookEvent;
///
/// # fn main() -> Result<(), serde_json::Error> {
/// let body = r#"{"id":"7f9c24e5-1d1f-4c0a-9b6c-2f3a4d5e6f70","type":"agreement.past_due","apiVersion":"2026-09-25","createdAt":"2026-09-25T10:00:00Z","data":{"id":"agr_1","status":"past_due","sequence":3}}"#;
/// let event = WebhookEvent::parse(body)?;
/// assert_eq!(event.event_type, "agreement.past_due");
/// assert_eq!(event.api_version.as_deref(), Some("2026-09-25"));
/// assert_eq!(event.sequence(), Some(3));
/// # Ok(())
/// # }
/// ```
///
/// `data` stays a [`serde_json::Value`]: its shape depends on `type`, and fields
/// are only ever added under an `apiVersion`, never renamed or removed, so ignore
/// fields you do not recognise. [`WebhookEvent::payment_data`],
/// [`WebhookEvent::charge_data`] and [`WebhookEvent::agreement_data`] read it
/// typed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookEvent {
    /// The delivery id. Your dedupe key: the same event can arrive twice.
    pub id: String,
    /// The event type, e.g. `payment.succeeded`, `agreement.past_due`,
    /// `charge.failed`: one of the [`event_type`](crate::event_type)
    /// constants, or a type this SDK does not know yet. `type` on the wire.
    #[serde(rename = "type")]
    pub event_type: String,
    /// The dated version of the payload shape the event was rendered in, e.g.
    /// `2026-09-25`. A redelivery keeps the `apiVersion` of its first attempt.
    /// `None` on deliveries from servers that predate the field.
    #[serde(default)]
    pub api_version: Option<String>,
    /// ISO 8601 UTC instant of the charge or transition the event reports. It can
    /// repeat across events, so never order by it; see [`WebhookEvent::sequence`].
    #[serde(default)]
    pub created_at: String,
    /// The event payload, shaped by `type`.
    #[serde(default)]
    pub data: Value,
}

impl WebhookEvent {
    /// Parses a verified raw body. The same as `serde_json::from_str`.
    pub fn parse(payload: &str) -> Result<WebhookEvent, serde_json::Error> {
        serde_json::from_str(payload)
    }

    /// `data.sequence`: the per-object counter that orders `agreement.*` and
    /// `charge.*` events. `None` on `payment.*` events and on deliveries from
    /// servers that predate the field.
    ///
    /// Deliveries can arrive out of order. Keep the highest sequence you have
    /// processed per object and discard any event whose sequence is not higher;
    /// when you need current state, read the object by id. createdAt can repeat
    /// across events, so order by sequence, not createdAt. A sequence of 0 only
    /// comes from events recorded before the counter existed; treat it as older
    /// than any positive number.
    ///
    /// The object the counter belongs to:
    ///
    /// - `agreement.*`: the agreement, `data.id`.
    /// - `charge.*` the platform placed for an agreement: the agreement period,
    ///   `data.agreementId` plus `data.periodNumber`, so the `charge.retrying` and
    ///   `charge.failed` events of one period compare across attempts.
    /// - `charge.*` for a one-off charge you initiated: `data.chargeId`.
    pub fn sequence(&self) -> Option<i64> {
        self.data.get("sequence").and_then(Value::as_i64)
    }

    /// `data` of a `payment.*` or a `verification.*` event, typed. `None` on
    /// every other event type, and when `data` does not have the documented
    /// shape.
    ///
    /// A `verification.*` event carries the payment fields of the verify-only
    /// session, with [`PaymentEventData::verify_only`] true. A verify-only
    /// session never sends a `payment.*` event.
    pub fn payment_data(&self) -> Option<PaymentEventData> {
        if !(self.event_type.starts_with("payment.")
            || self.event_type.starts_with("verification."))
        {
            return None;
        }
        serde_json::from_value(self.data.clone()).ok()
    }

    /// `data` of a `charge.*` event, typed. `None` on every other event type,
    /// and when `data` does not have the documented shape.
    pub fn charge_data(&self) -> Option<ChargeEventData> {
        if !self.event_type.starts_with("charge.") {
            return None;
        }
        serde_json::from_value(self.data.clone()).ok()
    }

    /// `data` of an `agreement.*` event, typed. `None` on every other event
    /// type, and when `data` does not have the documented shape.
    pub fn agreement_data(&self) -> Option<AgreementEventData> {
        if !self.event_type.starts_with("agreement.") {
            return None;
        }
        serde_json::from_value(self.data.clone()).ok()
    }

    /// Shortcut for [`PaymentEventData::stored_payment_method`]: the card a
    /// `payment.succeeded`, `payment.requires_capture` or
    /// `verification.succeeded` event says this payment kept on file. `None`
    /// on every other event; read the caveat on the field.
    pub fn stored_payment_method(&self) -> Option<StoredPaymentMethod> {
        self.payment_data()?.stored_payment_method
    }
}

/// `data` on a `payment.*` or a `verification.*` event, from
/// [`WebhookEvent::payment_data`].
///
/// Webhooks spell a missing value as an explicit `null`; the SDK reads null
/// and absent alike as `None`. Fields are only ever added, so older deliveries
/// that lack one still parse.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentEventData {
    /// The transaction the event is about. On `payment.refunded` that is the
    /// refund's own transaction; `original_transaction_id` is the payment.
    pub transaction_id: String,
    /// The status the transaction moved to, one of the
    /// [`status`](crate::status) constants.
    pub status: String,
    /// The status it moved from.
    #[serde(default)]
    pub previous_status: Option<String>,
    /// The kind of transaction, e.g. `sale`.
    #[serde(default)]
    pub kind: Option<String>,
    /// What you are PAID, in MINOR units (the base, without surcharge). On
    /// `payment.refunded`, the amount of that refund.
    #[serde(default)]
    pub amount: Option<i64>,
    /// The card movement in MINOR units, surcharge included.
    #[serde(default)]
    pub gross_amount: Option<i64>,
    /// The surcharge portion in MINOR units; `None` when there was none.
    #[serde(default)]
    pub surcharge_amount: Option<i64>,
    /// ISO 4217 currency.
    #[serde(default)]
    pub currency: Option<String>,
    /// How the payer paid, one of the [`payment_method`](crate::payment_method)
    /// constants.
    #[serde(default)]
    pub payment_method: Option<String>,
    /// The wallet, usually one of the [`wallet_type`](crate::wallet_type)
    /// constants; set only for wallet payments. Unknown values are valid wallets.
    #[serde(default)]
    pub wallet_type: Option<String>,
    /// The transaction this one hangs off: the refunded payment on
    /// `payment.refunded`, the authorization on the `payment.succeeded` of a
    /// capture.
    #[serde(default)]
    pub original_transaction_id: Option<String>,
    /// The Idempotency-Key you sent on create session. `None` on refund and
    /// dispute events.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Your own order reference, the one to match events to orders on. Refund
    /// and cancel events carry the original payment's.
    #[serde(default)]
    pub order_reference: Option<String>,
    /// The hosted checkout order id; `None` off the hosted path.
    #[serde(default)]
    pub order_id: Option<String>,
    /// The payment processor's reference for the transaction. `None` until it
    /// is known, and on a sale settled by reconciliation without a processor
    /// webhook. Refund and cancel events carry the original sale's reference.
    /// Older gateways omit it.
    #[serde(default)]
    pub psp_reference: Option<String>,
    /// The description you sent on create session.
    #[serde(default)]
    pub description: Option<String>,
    /// Lower-cased card brand once a card payment was attempted.
    #[serde(default)]
    pub payment_method_brand: Option<String>,
    /// Last four card digits once a card payment was attempted.
    #[serde(default)]
    pub payment_method_last4: Option<String>,
    /// The card this payment kept on file, the same object and type as
    /// [`CheckoutStatus::stored_payment_method`](crate::CheckoutStatus::stored_payment_method).
    ///
    /// Only `payment.succeeded`, `payment.requires_capture` for an
    /// authorization, and `verification.succeeded` carry it, when the card was
    /// stored together with the approval or the check. It is `None` when no
    /// card was saved and on every other event. `charge.*` events name their
    /// card by `data.storedPaymentMethodId` instead.
    ///
    /// It can ALSO be `None` when a card was saved: the card can be stored after
    /// the approval was announced, as on a server-to-server sale that succeeded
    /// synchronously and on a sale settled by a later sweep. The status read is
    /// the source of truth: on a `save_card` session whose event has no stored
    /// payment method, call [`Client::get_status`](crate::Client::get_status) to
    /// pick it up.
    #[serde(default)]
    pub stored_payment_method: Option<StoredPaymentMethod>,
    /// True on a `verification.*` event: the data of a verify-only session.
    /// False on every `payment.*` event, and on deliveries from servers that
    /// predate the field.
    #[serde(default, deserialize_with = "crate::types::false_when_null")]
    pub verify_only: bool,
    /// On `payment.failed` and `verification.failed`: the class of the
    /// decline, one of the [`decline_class`](crate::decline_class) constants,
    /// or a class this SDK does not know yet. It can be `None`, for example
    /// when nobody declined the payment.
    #[serde(default)]
    pub decline_class: Option<String>,
}

/// `data` on a `charge.*` event, from [`WebhookEvent::charge_data`]: a charge
/// on a stored card, one you made or one the platform placed for an agreement
/// period.
///
/// Webhooks spell a missing value as an explicit `null`; the SDK reads null
/// and absent alike as `None`. Fields are only ever added, so older deliveries
/// that lack one still parse.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChargeEventData {
    /// The charge, `ch_...`.
    pub charge_id: String,
    /// The transaction of the charge. Poll
    /// [`Client::get_status`](crate::Client::get_status) with it.
    #[serde(default)]
    pub transaction_id: Option<String>,
    /// The card that was charged, `pm_...`.
    #[serde(default)]
    pub stored_payment_method_id: Option<String>,
    /// The agreement the charge is for, `agr_...`. `None` for a one-off charge
    /// you made.
    #[serde(default)]
    pub agreement_id: Option<String>,
    /// Your customer reference from the agreement.
    #[serde(default)]
    pub customer_reference: Option<String>,
    /// `succeeded`, `retrying` or `failed`, one of the
    /// [`charge_outcome`](crate::charge_outcome) constants, or a value this SDK
    /// does not know yet.
    #[serde(default)]
    pub outcome: Option<String>,
    /// The agreement period the charge is for, from 1.
    #[serde(default)]
    pub period_number: Option<i64>,
    /// The attempt within the period, from 1.
    #[serde(default)]
    pub attempt_number: Option<i64>,
    /// The amount in MINOR units.
    #[serde(default)]
    pub amount: Option<i64>,
    /// ISO 4217 currency.
    #[serde(default)]
    pub currency: Option<String>,
    /// The brand and the last four digits of the card.
    #[serde(default)]
    pub payment_method: Option<ChargeEventPaymentMethod>,
    /// The order reference of the charge.
    #[serde(default)]
    pub order_reference: Option<String>,
    /// The description of the charge.
    #[serde(default)]
    pub description: Option<String>,
    /// On `charge.failed` and `charge.retrying`: the class of the decline, one
    /// of the [`decline_class`](crate::decline_class) constants.
    #[serde(default)]
    pub decline_class: Option<String>,
    /// The provider's decline code.
    #[serde(default)]
    pub decline_code: Option<String>,
    /// On `charge.retrying`: when the platform tries the card again, ISO 8601.
    #[serde(default)]
    pub next_attempt_at: Option<String>,
    /// When the next period is charged, ISO 8601.
    #[serde(default)]
    pub next_charge_at: Option<String>,
    /// The counter that orders this event; see [`WebhookEvent::sequence`].
    #[serde(default)]
    pub sequence: Option<i64>,
}

/// The card on a `charge.*` event, [`ChargeEventData::payment_method`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChargeEventPaymentMethod {
    /// Lower-cased card brand.
    #[serde(default)]
    pub brand: Option<String>,
    /// The last four card digits.
    #[serde(default)]
    pub last4: Option<String>,
}

/// `data` on an `agreement.*` event, from [`WebhookEvent::agreement_data`]: the
/// agreement as it is after the change, plus the status it moved from.
///
/// Webhooks spell a missing value as an explicit `null`; the SDK reads null
/// and absent alike as `None`. A field this struct does not model is still in
/// [`WebhookEvent::data`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgreementEventData {
    /// The agreement, `agr_...`.
    pub id: String,
    /// The plan, `plan_...`.
    #[serde(default)]
    pub plan_id: Option<String>,
    /// Your customer reference.
    #[serde(default)]
    pub customer_reference: Option<String>,
    /// The card the agreement charges, `pm_...`.
    #[serde(default)]
    pub stored_payment_method_id: Option<String>,
    /// The status it moved to, one of the
    /// [`agreement_status`](crate::agreement_status) constants.
    #[serde(default)]
    pub status: Option<String>,
    /// The status it moved from.
    #[serde(default)]
    pub previous_status: Option<String>,
    /// The amount per period in MINOR units.
    #[serde(default)]
    pub amount: Option<i64>,
    /// ISO 4217 currency.
    #[serde(default)]
    pub currency: Option<String>,
    /// One of the [`interval_unit`](crate::interval_unit) constants.
    #[serde(default)]
    pub interval_unit: Option<String>,
    /// How many interval units one period is.
    #[serde(default)]
    pub interval_count: Option<i64>,
    /// How many periods the agreement bills. `None` when it runs until
    /// cancelled.
    #[serde(default)]
    pub period_count: Option<i64>,
    /// The trial in days.
    #[serde(default)]
    pub trial_days: Option<i64>,
    /// When the next period is charged, ISO 8601.
    #[serde(default)]
    pub next_charge_at: Option<String>,
    /// When the agreement went active, ISO 8601.
    #[serde(default)]
    pub activated_at: Option<String>,
    /// When the agreement was cancelled, ISO 8601.
    #[serde(default)]
    pub cancelled_at: Option<String>,
    /// The version of the agreement.
    #[serde(default)]
    pub version: Option<i64>,
    /// The counter that orders this event; see [`WebhookEvent::sequence`].
    #[serde(default)]
    pub sequence: Option<i64>,
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
                if !value
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                {
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
