//! Idempotency keys: required on every call that can move money.

use std::fmt;

use crate::error::{Error, Result};

/// The gateway's limit, in characters.
const MAX_KEY_CHARS: usize = 100;

/// The idempotency key of one payment. Every create-style call takes one, and
/// the SDK never makes one up for you.
///
/// The key is what makes a retry safe: the gateway never opens a second payment
/// for a key it has already seen. That only works when the key belongs to the
/// ORDER rather than to the request, so the same order asked again (a page
/// reload, the back button, a retry after a timeout) sends the same key. A key
/// minted per call would turn every one of those into a new payment attempt.
///
/// Build it from the order with [`IdempotencyKey::for_order`], or wrap a key you
/// already derive yourself with [`IdempotencyKey::new`]. Both validate up
/// front, so a request that holds one is never refused for its key before it
/// is sent.
///
/// ```
/// use dominaite::{CheckoutSessionRequest, IdempotencyKey};
///
/// # fn main() -> Result<(), dominaite::Error> {
/// let key = IdempotencyKey::for_order("shop-a1b2c3d4", "order-1042", 2500, "eur")?;
/// assert_eq!(key.as_str(), "shop-a1b2c3d4-order-1042-2500-EUR");
///
/// let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Wraps a key you derive yourself. It must not be empty or whitespace, and
    /// it is at most 100 characters (characters, not bytes, so a Cyrillic key
    /// gets the full 100). Anything else is [`Error::Validation`].
    pub fn new(key: impl Into<String>) -> Result<IdempotencyKey> {
        let key = key.into();
        if key.trim().is_empty() {
            return Err(Error::validation("idempotency_key must not be empty"));
        }
        // Characters, not bytes. `len()` counts UTF-8 bytes, so a non-Latin key
        // would hit the limit at half its real length.
        if key.chars().count() > MAX_KEY_CHARS {
            return Err(Error::validation(
                "idempotency_key must be at most 100 characters",
            ));
        }
        Ok(IdempotencyKey(key))
    }

    /// The recommended key: `{scope}-{order_id}-{amount_minor}-{CURRENCY}`.
    ///
    /// - The same order at the same amount always produces the same key, so a
    ///   reload or the back button replays the session already open for it
    ///   instead of opening a second one.
    /// - A changed amount or currency produces a new key, so a re-priced order
    ///   never reuses a session opened for the old total (the gateway would
    ///   refuse that replay with `IDEMPOTENCY_KEY_REUSED`).
    /// - `scope` keeps deployments that share one merchant apart: a staging
    ///   and a production shop that both count orders from 1001 must not
    ///   collide. A short hash of the shop's public URL works well.
    ///
    /// `amount_minor` is the same MINOR-unit integer you put on the request and
    /// must be positive. `currency` is a three-letter ISO 4217 code in any case;
    /// the key carries it uppercased. Empty parts, and a result over 100
    /// characters, are [`Error::Validation`].
    pub fn for_order(
        scope: &str,
        order_id: &str,
        amount_minor: i64,
        currency: &str,
    ) -> Result<IdempotencyKey> {
        if scope.trim().is_empty() {
            return Err(Error::validation("scope must not be empty"));
        }
        if order_id.trim().is_empty() {
            return Err(Error::validation("order_id must not be empty"));
        }
        if amount_minor <= 0 {
            return Err(Error::validation(
                "amount_minor must be a positive integer in MINOR units (e.g. 2500 for 25.00 EUR)",
            ));
        }
        if currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return Err(Error::validation(
                "currency must be a three-letter ISO 4217 code",
            ));
        }
        let currency = currency.to_ascii_uppercase();
        IdempotencyKey::new(format!("{scope}-{order_id}-{amount_minor}-{currency}"))
    }

    /// The key as it is sent and signed.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for IdempotencyKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
