//! The error taxonomy: what happened, and whether retrying can help.

use std::error::Error as StdError;
use std::fmt;

use serde_json::Value;

use crate::types::PaymentMethodCharge;

/// The result type every call in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

/// The codes [`Client::create_checkout_session`](crate::Client::create_checkout_session)
/// can answer with, so you can branch on [`Error::code`] without typing the
/// strings.
///
/// Two shapes. The replay and availability codes are business refusals: HTTP
/// 200 with `success: false`, arriving as [`Error::Refusal`]. The storefront
/// codes are real HTTP errors (409 or 400) and arrive as [`Error::Api`] with the
/// code set; they will not change on a retry and need a fix on the Dominaite
/// side (backoffice or onboarding), not in your code.
///
/// ```no_run
/// # use dominaite::{session_error_code, CheckoutSessionRequest, Client, Error, IdempotencyKey};
/// # fn main() -> Result<(), Error> {
/// # let client = Client::new("dmk_x", "dms_y")?;
/// # let key = IdempotencyKey::new("order-1042")?;
/// # let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key);
/// match client.create_checkout_session(&request) {
///     Err(error) if error.code() == Some(session_error_code::STOREFRONT_NOT_WHITELISTED) => {
///         // HTTP 409: this site's domain is not whitelisted with the payment
///         // provider yet. Show "payments unavailable" and alert your team.
///     }
///     _ => {}
/// }
/// # Ok(())
/// # }
/// ```
pub mod session_error_code {
    /// HTTP 409, [`Error::Api`](crate::Error::Api): the storefront's domain is
    /// not yet whitelisted with the payment provider. Nothing was created. Not
    /// retryable; the whitelisting is finished on the Dominaite side.
    pub const STOREFRONT_NOT_WHITELISTED: &str = "STOREFRONT_NOT_WHITELISTED";
    /// HTTP 409, [`Error::Api`](crate::Error::Api): the storefront (online
    /// location) this key or request points at was deactivated or deleted.
    pub const STOREFRONT_INACTIVE: &str = "STOREFRONT_INACTIVE";
    /// HTTP 400, [`Error::Api`](crate::Error::Api): the API key is bound to one
    /// storefront and the request named a different one.
    pub const STOREFRONT_MISMATCH: &str = "STOREFRONT_MISMATCH";

    /// HTTP 200 refusal: card payments are off right now; retry later with the
    /// SAME key.
    pub const PAYMENT_PROCESSING_UNAVAILABLE: &str = "PAYMENT_PROCESSING_UNAVAILABLE";
    /// HTTP 200 refusal: a session for this key is open but cannot be handed
    /// back right now (a concurrent create, or an expired one not yet
    /// replaced); re-send the SAME key shortly, never a fresh one. A clean
    /// replay of an open session is not this: it returns the original session.
    pub const DUPLICATE_REQUEST: &str = "DUPLICATE_REQUEST";
    /// HTTP 200 refusal: this key's payment already completed. Carries the
    /// transaction id; read it back with
    /// [`Client::get_status`](crate::Client::get_status).
    pub const ALREADY_PROCESSED: &str = "ALREADY_PROCESSED";
    /// HTTP 200 refusal: this key was replayed with a different amount,
    /// currency or body. With an order-derived key that means the order total
    /// changed without the key changing.
    pub const IDEMPOTENCY_KEY_REUSED: &str = "IDEMPOTENCY_KEY_REUSED";
    /// HTTP 200 refusal: the earlier attempt with this key failed, was
    /// cancelled or was abandoned. The key is spent; use a fresh one.
    pub const PRIOR_ATTEMPT_FAILED: &str = "PRIOR_ATTEMPT_FAILED";

    /// The business refusal codes, in the order the canonical contract lists
    /// them. An unlisted code still arrives as
    /// [`Error::Refusal`](crate::Error::Refusal).
    pub const REFUSALS: [&str; 5] = [
        PAYMENT_PROCESSING_UNAVAILABLE,
        DUPLICATE_REQUEST,
        ALREADY_PROCESSED,
        IDEMPOTENCY_KEY_REUSED,
        PRIOR_ATTEMPT_FAILED,
    ];

    /// The storefront codes, in the order the canonical contract lists them.
    /// They arrive as [`Error::Api`](crate::Error::Api) with a 400 or 409, are
    /// never retryable, and are not in [`REFUSALS`].
    pub const STOREFRONT: [&str; 3] = [
        STOREFRONT_MISMATCH,
        STOREFRONT_INACTIVE,
        STOREFRONT_NOT_WHITELISTED,
    ];
}

/// The codes [`Client::charge_payment_method`](crate::Client::charge_payment_method)
/// returns as [`Error::Charge`], in the gateway's own order. `CHARGE_DECLINED`
/// (HTTP 402) is deliberately not one of them: a decline is a charge result with
/// status `failed`, not an error.
pub mod charge_error_code {
    /// HTTP 409: the method is revoked or expired; ask the customer for another
    /// card via a hosted session with `save_card`.
    pub const PAYMENT_METHOD_NOT_ACTIVE: &str = "PAYMENT_METHOD_NOT_ACTIVE";
    /// HTTP 409: a request with this key is still in flight; retry with the
    /// SAME key in a moment.
    pub const DUPLICATE_REQUEST: &str = "DUPLICATE_REQUEST";
    /// HTTP 422: same key, different body or method; a bug on your side.
    pub const IDEMPOTENCY_KEY_REUSED: &str = "IDEMPOTENCY_KEY_REUSED";
    /// HTTP 502: the provider gave no verdict and the charge MAY have happened.
    /// The charge row is attached: poll `get_status` with its transaction id or
    /// wait for the webhook. Never retry under a new key.
    pub const CHARGE_OUTCOME_UNKNOWN: &str = "CHARGE_OUTCOME_UNKNOWN";
    /// HTTP 502: nothing was charged. The charge row is attached when one
    /// exists, absent when the provider refused before one.
    pub const CHARGE_FAILED: &str = "CHARGE_FAILED";
    /// HTTP 503: charges of stored methods are switched off; nothing was
    /// charged. Retry later with the SAME key.
    pub const PAYMENT_METHOD_CHARGES_DISABLED: &str = "PAYMENT_METHOD_CHARGES_DISABLED";
    /// HTTP 503: card payments are off right now; nothing was charged. Retry
    /// later with the SAME key.
    pub const PAYMENT_PROCESSING_UNAVAILABLE: &str = "PAYMENT_PROCESSING_UNAVAILABLE";

    /// The whole vocabulary, in the order the canonical contract lists it. An
    /// unlisted code still arrives as [`Error::Charge`](crate::Error::Charge).
    pub const ALL: [&str; 7] = [
        PAYMENT_METHOD_NOT_ACTIVE,
        DUPLICATE_REQUEST,
        IDEMPOTENCY_KEY_REUSED,
        CHARGE_OUTCOME_UNKNOWN,
        CHARGE_FAILED,
        PAYMENT_METHOD_CHARGES_DISABLED,
        PAYMENT_PROCESSING_UNAVAILABLE,
    ];
}

/// The codes [`Client::revoke_payment_method`](crate::Client::revoke_payment_method)
/// returns as [`Error::Revoke`], in the gateway's own order.
pub mod revoke_error_code {
    /// HTTP 502: the provider refused the deletion for a reason a retry will
    /// not fix; contact support with the payment method id.
    pub const UPSTREAM_CONTRACT_ERROR: &str = "UPSTREAM_CONTRACT_ERROR";
    /// HTTP 503: the provider is unavailable or throttling; retry later.
    pub const MERCHANT_API_UNAVAILABLE: &str = "MERCHANT_API_UNAVAILABLE";

    /// The whole vocabulary, in the order the canonical contract lists it. An
    /// unlisted code still arrives as [`Error::Revoke`](crate::Error::Revoke).
    pub const ALL: [&str; 2] = [UPSTREAM_CONTRACT_ERROR, MERCHANT_API_UNAVAILABLE];
}

/// The codes [`Client::create_refund`](crate::Client::create_refund) and
/// [`Client::get_refund`](crate::Client::get_refund) answer with, in the order the
/// canonical contract lists them. They arrive as [`Error::Api`](crate::Error::Api)
/// with the code and the HTTP status set. A 500 on these routes means nothing
/// was queued and arrives as [`Error::Transport`](crate::Error::Transport): retry
/// with the SAME key.
///
/// `REFUND_FAILED` is not one of them: it is a
/// [`refund_failure_code`](crate::refund_failure_code) on a refund that was
/// accepted and then failed.
pub mod refund_error_code {
    /// HTTP 404: no card-not-present payment with this id under your account.
    pub const PAYMENT_NOT_FOUND: &str = "PAYMENT_NOT_FOUND";
    /// HTTP 404, status read only: no refund with this id on this payment.
    /// Right after a 202 the refund may not be picked up yet; read it again for
    /// up to 60 seconds, after that the id is unknown.
    pub const REFUND_NOT_FOUND: &str = "REFUND_NOT_FOUND";
    /// HTTP 422: the payment is not paid, already fully refunded, or everything
    /// left on it is already being refunded. Nothing was queued and the key is
    /// not burnt.
    pub const PAYMENT_NOT_REFUNDABLE: &str = "PAYMENT_NOT_REFUNDABLE";
    /// HTTP 422: the amount is more than what is left to refund, counting
    /// refunds still in progress; the message names the amount left. Nothing
    /// was queued and the key is not burnt.
    pub const REFUND_AMOUNT_EXCEEDED: &str = "REFUND_AMOUNT_EXCEEDED";
    /// HTTP 422: this key was first used for a different amount, reason or
    /// payment. Use a fresh key for a genuinely new refund.
    pub const IDEMPOTENCY_KEY_REUSED: &str = "IDEMPOTENCY_KEY_REUSED";
    /// HTTP 409: a request with this key is being processed right now. Retry
    /// with the SAME key after a second, for up to 120 seconds.
    pub const DUPLICATE_REQUEST: &str = "DUPLICATE_REQUEST";
    /// HTTP 400: the Idempotency-Key header is missing or longer than 100
    /// characters.
    pub const IDEMPOTENCY_KEY_REQUIRED: &str = "IDEMPOTENCY_KEY_REQUIRED";

    /// The whole vocabulary, in the order the canonical contract lists it. An
    /// unlisted code still arrives as [`Error::Api`](crate::Error::Api) with
    /// its code.
    pub const ALL: [&str; 7] = [
        PAYMENT_NOT_FOUND,
        REFUND_NOT_FOUND,
        PAYMENT_NOT_REFUNDABLE,
        REFUND_AMOUNT_EXCEEDED,
        IDEMPOTENCY_KEY_REUSED,
        DUPLICATE_REQUEST,
        IDEMPOTENCY_KEY_REQUIRED,
    ];

    /// How long the same request is worth sending again after this code, in
    /// seconds: 120 for `DUPLICATE_REQUEST` (same key), 60 for
    /// `REFUND_NOT_FOUND` (the status read, right after a 202). `None` for every
    /// other code, which a retry will not change.
    ///
    /// Not what [`Error::is_retryable`](crate::Error::is_retryable) answers:
    /// that stays true only for a transport failure, which is safe to resend
    /// at once. These two want a pause between attempts.
    pub fn retry_window_seconds(code: &str) -> Option<u64> {
        match code {
            DUPLICATE_REQUEST => Some(120),
            REFUND_NOT_FOUND => Some(60),
            _ => None,
        }
    }
}

/// Everything that can go wrong, split by what you should do about it.
///
/// Match on the variant rather than on the message. Every variant that carries a
/// machine-readable code exposes it through [`Error::code`].
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The SDK rejected the call before sending anything. Nothing reached the
    /// network; fix the arguments.
    Validation {
        /// What was wrong with the call.
        message: String,
    },

    /// The API understood the request and refused it: HTTP 200 with
    /// `success: false`. The replay codes arrive this way too
    /// (`DUPLICATE_REQUEST`, `ALREADY_PROCESSED`, `PRIOR_ATTEMPT_FAILED`,
    /// `IDEMPOTENCY_KEY_REUSED`).
    ///
    /// Never blind-retry a refusal. It will not change on its own. The one
    /// exception is `PAYMENT_PROCESSING_UNAVAILABLE`, which is temporary:
    /// [`create_checkout_session_with_retry`](crate::Client::create_checkout_session_with_retry)
    /// retries it with the same key.
    Refusal {
        /// The machine-readable reason, e.g. `PAYMENT_PROCESSING_UNAVAILABLE`.
        code: String,
        /// The human-readable reason from the API.
        message: String,
        /// The payment this idempotency key collided with, when the API named
        /// one. That is the recovery path for a replay refusal: read it back
        /// with [`Client::get_status`](crate::Client::get_status) to find out
        /// what the earlier attempt did, instead of minting a second payment
        /// for the same order.
        ///
        /// `None` when the API did not name one - notably the concurrent-race
        /// `DUPLICATE_REQUEST`, which knows a key was taken but not yet by
        /// which row.
        ///
        /// ```no_run
        /// # use dominaite::{Client, CheckoutSessionRequest, Error, IdempotencyKey};
        /// # fn main() -> Result<(), Error> {
        /// # let client = Client::new("dmk_x", "dms_y")?;
        /// # let key = IdempotencyKey::new("order-1042")?;
        /// # let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key);
        /// match client.create_checkout_session(&request) {
        ///     Err(Error::Refusal { transaction_id: Some(id), .. }) => {
        ///         let status = client.get_status(&id)?;
        ///     }
        ///     _ => {}
        /// }
        /// # Ok(())
        /// # }
        /// ```
        transaction_id: Option<String>,
    },

    /// The API rejected your credentials or signature (HTTP 401/403). Not
    /// retryable: fix the key id, the secret, the server clock, or the caller
    /// allowlist.
    Auth {
        /// One of `INVALID_API_KEY`, `INVALID_SIGNATURE`,
        /// `TIMESTAMP_OUT_OF_RANGE`, `IP_NOT_ALLOWED`.
        code: String,
        /// The human-readable reason.
        message: String,
    },

    /// The API answered, but with an unexpected or rejecting response. A 422
    /// means an idempotency key was replayed with a different body; use a fresh
    /// key. A 404 from [`Client::get_status`](crate::Client::get_status) means an
    /// unknown transaction id. A 409 or 400 carrying one of the
    /// [`session_error_code::STOREFRONT`] codes means the storefront cannot take
    /// payments yet (`STOREFRONT_NOT_WHITELISTED`) or at all. The refund routes
    /// answer every [`refund_error_code`] this way.
    Api {
        /// The HTTP status code.
        status: u16,
        /// The machine-readable reason when the API sent one, e.g.
        /// `IDEMPOTENCY_KEY_REQUIRED` on a 400. Input validation answers with a
        /// code and a real status, unlike a business refusal, which is a 200
        /// carrying [`Error::Refusal`]. Also on [`Error::code`].
        ///
        /// `None` for a response that carried no code, including the ones this
        /// crate raises itself when a 200 body does not parse.
        code: Option<String>,
        /// The human-readable reason.
        message: String,
    },

    /// The gateway answered a charge with an error code instead of a charge
    /// result. `code` is one of the [`charge_error_code`] constants (an
    /// unlisted code arrives here too, as a plain string), `status` is the HTTP
    /// status, and `charge` is the charge row the gateway attached when it did.
    ///
    /// The one that matters most is `CHARGE_OUTCOME_UNKNOWN` (502): the provider
    /// gave no verdict and the charge MAY have happened. `charge` is present, so
    /// poll [`Client::get_status`](crate::Client::get_status) with
    /// `transaction_id` or wait for the webhook. Never retry under a new key.
    ///
    /// ```no_run
    /// # use dominaite::{charge_error_code, ChargeRequest, Client, Error, IdempotencyKey};
    /// # fn main() -> Result<(), Error> {
    /// # let client = Client::new("dmk_x", "dms_y")?;
    /// # let key = IdempotencyKey::new("order-1043")?;
    /// # let request = ChargeRequest::new(2500, "EUR", "order-1043", key);
    /// match client.charge_payment_method("pm_...", &request) {
    ///     Err(Error::Charge { code, transaction_id: Some(id), .. })
    ///         if code == charge_error_code::CHARGE_OUTCOME_UNKNOWN =>
    ///     {
    ///         let status = client.get_status(&id)?;
    ///     }
    ///     _ => {}
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// A decline is NOT this variant: HTTP 402 comes back as `Ok` with a charge
    /// whose status is `failed`. A 404 for an id that is not yours is
    /// [`Error::Api`] with code `PAYMENT_METHOD_NOT_FOUND`, and a 5xx that
    /// carries no gateway code (an HTML page from a proxy) is [`Error::Transport`].
    Charge {
        /// The HTTP status code: 409, 422, 502 or 503.
        status: u16,
        /// The machine-readable reason. See [`charge_error_code`].
        code: String,
        /// The human-readable reason from the API.
        message: String,
        /// The charge row the gateway attached to its answer, when it did.
        /// Boxed to keep the error small; deref it like any other charge.
        charge: Option<Box<PaymentMethodCharge>>,
        /// Shortcut for `charge.transaction_id`, for polling `get_status`.
        transaction_id: Option<String>,
        /// The whole envelope, for fields not modelled above.
        raw: Value,
    },

    /// The gateway refused to revoke a stored payment method. Nothing changed
    /// either way; `code` is one of the [`revoke_error_code`] constants (an
    /// unlisted code arrives here too). An id that is not yours is still
    /// [`Error::Api`] with status 404.
    Revoke {
        /// The HTTP status code: 502 or 503.
        status: u16,
        /// The machine-readable reason. See [`revoke_error_code`].
        code: String,
        /// The human-readable reason from the API.
        message: String,
        /// The whole envelope, for fields not modelled above.
        raw: Value,
    },

    /// The API is rate limiting you (HTTP 429).
    ///
    /// The platform allows 60 requests per minute per API key and 120 per minute
    /// per IP. Polling `get_status` in a tight loop is the usual way to hit
    /// this.
    ///
    /// NOT retried automatically, and [`Error::is_retryable`] is false: another
    /// request into a limiter is what got you here. Wait
    /// `retry_after_seconds` (or back off on your own schedule when the API did
    /// not name one), then send it again WITH THE SAME idempotency key, so a
    /// request that did land cannot become a second payment.
    RateLimited {
        /// The `Retry-After` value in seconds, when the API sent one.
        ///
        /// `None` when the header was absent, or in the HTTP-date form this SDK
        /// does not interpret.
        retry_after_seconds: Option<u64>,
    },

    /// A network-level failure, a timeout, or a 5xx. The request may or may not
    /// have reached the API, so retry WITH THE SAME idempotency key: a retried
    /// key never creates a second payment.
    Transport {
        /// What failed.
        message: String,
        /// The underlying cause, when there was one.
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
}

impl Error {
    /// The machine-readable code, for the variants that carry one.
    ///
    /// `Refusal`, `Auth`, `Charge` and `Revoke` always have one, `Api` has one
    /// when the API sent it; the rest return `None`.
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::Refusal { code, .. }
            | Error::Auth { code, .. }
            | Error::Charge { code, .. }
            | Error::Revoke { code, .. } => Some(code),
            Error::Api { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    /// The HTTP status, for the variants that carry one.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. }
            | Error::Charge { status, .. }
            | Error::Revoke { status, .. } => Some(*status),
            Error::RateLimited { .. } => Some(429),
            _ => None,
        }
    }

    /// True only for [`Error::Transport`], the one kind that is safe to retry
    /// blindly - and only with the SAME idempotency key.
    /// [`create_checkout_session_with_retry`](crate::Client::create_checkout_session_with_retry)
    /// does exactly that, and also retries the `PAYMENT_PROCESSING_UNAVAILABLE`
    /// refusal, which this answers false for. False for [`Error::Charge`] even on a 503: those
    /// codes each carry their own advice, and `CHARGE_OUTCOME_UNKNOWN` must be
    /// polled, never resent.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Error::Transport { .. })
    }

    pub(crate) fn validation(message: impl Into<String>) -> Error {
        Error::Validation {
            message: message.into(),
        }
    }

    pub(crate) fn refusal(
        code: impl Into<String>,
        message: impl Into<String>,
        transaction_id: Option<String>,
    ) -> Error {
        Error::Refusal {
            code: code.into(),
            message: message.into(),
            transaction_id,
        }
    }

    pub(crate) fn auth(code: impl Into<String>, message: impl Into<String>) -> Error {
        Error::Auth {
            code: code.into(),
            message: message.into(),
        }
    }

    /// An API error with no machine-readable code: the responses this crate
    /// raises itself when a 200 body is not what the contract promises.
    pub(crate) fn api(status: u16, message: impl Into<String>) -> Error {
        Error::Api {
            status,
            code: None,
            message: message.into(),
        }
    }

    pub(crate) fn api_with_code(
        status: u16,
        code: Option<String>,
        message: impl Into<String>,
    ) -> Error {
        Error::Api {
            status,
            code,
            message: message.into(),
        }
    }

    pub(crate) fn charge(
        status: u16,
        code: impl Into<String>,
        message: impl Into<String>,
        charge: Option<PaymentMethodCharge>,
        raw: Value,
    ) -> Error {
        Error::Charge {
            status,
            code: code.into(),
            message: message.into(),
            transaction_id: charge
                .as_ref()
                .map(|charge| charge.transaction_id.clone())
                .filter(|id| !id.is_empty()),
            charge: charge.map(Box::new),
            raw,
        }
    }

    pub(crate) fn revoke(
        status: u16,
        code: impl Into<String>,
        message: impl Into<String>,
        raw: Value,
    ) -> Error {
        Error::Revoke {
            status,
            code: code.into(),
            message: message.into(),
            raw,
        }
    }

    pub(crate) fn transport(
        message: impl Into<String>,
        source: Option<Box<dyn StdError + Send + Sync>>,
    ) -> Error {
        Error::Transport {
            message: message.into(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Validation { message } => write!(f, "invalid request: {message}"),
            Error::Refusal { code, message, .. } => {
                write!(f, "checkout refused ({code}): {message}")
            }
            Error::Auth { code, message } => {
                write!(f, "authentication failed ({code}): {message}")
            }
            Error::Api {
                status,
                code: Some(code),
                message,
            } => write!(f, "API error (HTTP {status}, {code}): {message}"),
            Error::Api {
                status, message, ..
            } => {
                write!(f, "API error (HTTP {status}): {message}")
            }
            Error::Charge {
                status,
                code,
                message,
                ..
            } => write!(f, "charge error (HTTP {status}, {code}): {message}"),
            Error::Revoke {
                status,
                code,
                message,
                ..
            } => write!(f, "revoke error (HTTP {status}, {code}): {message}"),
            Error::RateLimited {
                retry_after_seconds: Some(seconds),
            } => write!(f, "rate limited (HTTP 429): retry after {seconds}s"),
            Error::RateLimited { .. } => {
                write!(f, "rate limited (HTTP 429): back off and retry")
            }
            Error::Transport { message, .. } => write!(f, "transport error: {message}"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Error::Transport { source, .. } => {
                source.as_ref().map(|boxed| boxed.as_ref() as &dyn StdError)
            }
            _ => None,
        }
    }
}
