//! The error taxonomy: what happened, and whether retrying can help.

use std::error::Error as StdError;
use std::fmt;

/// The result type every call in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

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
    /// Never blind-retry a refusal. It will not change on its own.
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
        /// # use dominaite::{Client, CheckoutSessionRequest, Error};
        /// # fn main() -> Result<(), Error> {
        /// # let client = Client::new("dmk_x", "dms_y")?;
        /// # let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042");
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
    /// unknown transaction id.
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
    /// `Refusal` and `Auth` always have one, `Api` has one when the API sent it;
    /// the rest return `None`.
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::Refusal { code, .. } | Error::Auth { code, .. } => Some(code),
            Error::Api { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    /// The HTTP status, for the variants that carry one.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// True only for [`Error::Transport`], the one kind that is safe to retry -
    /// and only with the SAME idempotency key.
    /// [`create_checkout_session_with_retry`](crate::Client::create_checkout_session_with_retry)
    /// does exactly that.
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
