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
/// # let client = Client::new("dmk_x", "dms_y", dominaite::SANDBOX_BASE_URL)?;
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
    /// storefront and the request named a different one. When the idempotency
    /// key already opened a session, the API answers it as an HTTP 200 refusal
    /// instead; it still arrives as [`Error::Api`](crate::Error::Api), with
    /// status 200.
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
    /// HTTP 400, [`Error::Api`](crate::Error::Api): the Idempotency-Key header
    /// is missing or longer than 100 characters. It is part of the signed
    /// payload.
    pub const IDEMPOTENCY_KEY_REQUIRED: &str = "IDEMPOTENCY_KEY_REQUIRED";

    /// HTTP 200 refusal, subscription session: no agreement with this
    /// `agreement_id` under your account. The `agr_` handle is case-sensitive.
    pub const AGREEMENT_NOT_FOUND: &str = "AGREEMENT_NOT_FOUND";
    /// HTTP 200 refusal, subscription session: the agreement is already
    /// active, cancelled, completed or failed, so there is no first payment
    /// left to take.
    pub const AGREEMENT_NOT_PENDING: &str = "AGREEMENT_NOT_PENDING";
    /// HTTP 200 refusal, subscription session: the amount or the currency
    /// differs from the agreement's own.
    pub const AGREEMENT_AMOUNT_MISMATCH: &str = "AGREEMENT_AMOUNT_MISMATCH";
    /// HTTP 200 refusal, subscription session: neither the request nor the
    /// agreement carries consent. Send `consent` on the session.
    pub const AGREEMENT_CONSENT_REQUIRED: &str = "AGREEMENT_CONSENT_REQUIRED";
    /// HTTP 200 refusal, subscription session: a session for this agreement
    /// is already open. The refusal's `transaction_id` names it.
    pub const AGREEMENT_SESSION_OPEN: &str = "AGREEMENT_SESSION_OPEN";
    /// HTTP 200 refusal, subscription session: the session of this agreement
    /// was already paid and the agreement is still pending. Read the status of
    /// the refusal's `transaction_id`.
    pub const AGREEMENT_ALREADY_PAID: &str = "AGREEMENT_ALREADY_PAID";
    /// HTTP 200 refusal, subscription session: the agreement has a
    /// `max_amount_per_period`, and a session can only take a fixed amount.
    pub const AGREEMENT_VARIABLE_AMOUNT_UNSUPPORTED: &str = "AGREEMENT_VARIABLE_AMOUNT_UNSUPPORTED";
    /// HTTP 200 refusal, subscription session: subscriptions are not switched
    /// on for your account yet. Nothing was created.
    pub const SUBSCRIPTIONS_NOT_ENABLED: &str = "SUBSCRIPTIONS_NOT_ENABLED";

    /// The business refusal codes. Each arrives as
    /// [`Error::Refusal`](crate::Error::Refusal), and so does an unlisted
    /// code. The first five are the replay and availability codes; the rest
    /// belong to subscription sessions.
    pub const REFUSALS: [&str; 13] = [
        PAYMENT_PROCESSING_UNAVAILABLE,
        DUPLICATE_REQUEST,
        ALREADY_PROCESSED,
        IDEMPOTENCY_KEY_REUSED,
        PRIOR_ATTEMPT_FAILED,
        AGREEMENT_NOT_FOUND,
        AGREEMENT_NOT_PENDING,
        AGREEMENT_AMOUNT_MISMATCH,
        AGREEMENT_CONSENT_REQUIRED,
        AGREEMENT_SESSION_OPEN,
        AGREEMENT_ALREADY_PAID,
        AGREEMENT_VARIABLE_AMOUNT_UNSUPPORTED,
        SUBSCRIPTIONS_NOT_ENABLED,
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
    /// HTTP 409: the method is revoked, expired or retired; ask the customer
    /// for another card via a hosted session with `save_card`.
    pub const PAYMENT_METHOD_NOT_ACTIVE: &str = "PAYMENT_METHOD_NOT_ACTIVE";
    /// HTTP 404: no stored payment method with this id under your account.
    /// Nothing was charged.
    pub const PAYMENT_METHOD_NOT_FOUND: &str = "PAYMENT_METHOD_NOT_FOUND";
    /// HTTP 409: another charge on this payment method, sent with a different
    /// key, still waits for its verdict. Nothing was charged and the key is not
    /// burnt. `error.params` in `raw` names the other charge: read its status
    /// first, then resend with the SAME key if this charge is still owed.
    pub const CHARGE_IN_PROGRESS: &str = "CHARGE_IN_PROGRESS";
    /// HTTP 409: a request with this key is still in flight; retry with the
    /// SAME key in a moment.
    pub const DUPLICATE_REQUEST: &str = "DUPLICATE_REQUEST";
    /// HTTP 422: same key, different body or method; a bug on your side.
    pub const IDEMPOTENCY_KEY_REUSED: &str = "IDEMPOTENCY_KEY_REUSED";
    /// HTTP 422: a card is only charged in the currency of the payment it was
    /// saved from. Nothing was charged.
    pub const CHARGE_CURRENCY_MISMATCH: &str = "CHARGE_CURRENCY_MISMATCH";
    /// HTTP 404: `agreement_id` is not one of your agreements. Nothing was
    /// charged and the key is not burnt.
    pub const AGREEMENT_NOT_FOUND: &str = "AGREEMENT_NOT_FOUND";
    /// HTTP 422: the agreement is linked to a different payment method than
    /// the one in the path. Nothing was charged.
    pub const AGREEMENT_METHOD_MISMATCH: &str = "AGREEMENT_METHOD_MISMATCH";
    /// HTTP 422: the agreement has no consent on record, so it cannot back a
    /// charge. Nothing was charged.
    pub const AGREEMENT_CONSENT_MISSING: &str = "AGREEMENT_CONSENT_MISSING";
    /// HTTP 422: the agreement is in a different currency than the charge.
    /// Nothing was charged.
    pub const AGREEMENT_CURRENCY_MISMATCH: &str = "AGREEMENT_CURRENCY_MISMATCH";
    /// HTTP 422: a fixed agreement charges exactly its per-period amount, and
    /// this amount is below it. Nothing was charged.
    pub const AGREEMENT_AMOUNT_MISMATCH: &str = "AGREEMENT_AMOUNT_MISMATCH";
    /// HTTP 422: the amount is above what the customer agreed to under this
    /// agreement. Nothing was charged.
    pub const CHARGE_AMOUNT_ABOVE_AGREEMENT: &str = "CHARGE_AMOUNT_ABOVE_AGREEMENT";
    /// HTTP 422: the current billing period of the agreement already has its
    /// charge. Nothing was charged.
    pub const AGREEMENT_PERIOD_ALREADY_CHARGED: &str = "AGREEMENT_PERIOD_ALREADY_CHARGED";
    /// HTTP 422: the agreement billed its last period, or was cancelled before
    /// the current period began. Nothing was charged.
    pub const AGREEMENT_ENDED: &str = "AGREEMENT_ENDED";
    /// HTTP 422: the agreement is not active yet or is still in its trial.
    /// Nothing was charged.
    pub const AGREEMENT_NOT_STARTED: &str = "AGREEMENT_NOT_STARTED";
    /// HTTP 422: the amount is above the most one charge may be in this
    /// currency. Nothing was charged.
    pub const CHARGE_AMOUNT_ABOVE_CAP: &str = "CHARGE_AMOUNT_ABOVE_CAP";
    /// HTTP 502: the provider gave no verdict and the charge MAY have happened.
    /// The charge row is attached: poll `get_status` with its transaction id or
    /// wait for the webhook. Never retry under a new key.
    pub const CHARGE_OUTCOME_UNKNOWN: &str = "CHARGE_OUTCOME_UNKNOWN";
    /// HTTP 502: nothing was charged. The charge row is attached when one
    /// exists, absent when the provider refused before one.
    pub const CHARGE_FAILED: &str = "CHARGE_FAILED";
    /// HTTP 503: nothing was charged. Stored-card charges are not switched on
    /// for your account, or we paused charges for a short time. The code is
    /// the same in both cases. If we never switched stored-card charges on for
    /// your account, a retry does not help: ask us (Dominaite support). If
    /// they are on, retry later with the SAME idempotency key.
    pub const PAYMENT_METHOD_CHARGES_DISABLED: &str = "PAYMENT_METHOD_CHARGES_DISABLED";
    /// HTTP 503: card payments are off right now; nothing was charged. Retry
    /// later with the SAME key.
    pub const PAYMENT_PROCESSING_UNAVAILABLE: &str = "PAYMENT_PROCESSING_UNAVAILABLE";

    /// The whole vocabulary. An unlisted code still arrives as
    /// [`Error::Charge`](crate::Error::Charge).
    pub const ALL: [&str; 20] = [
        PAYMENT_METHOD_NOT_ACTIVE,
        DUPLICATE_REQUEST,
        IDEMPOTENCY_KEY_REUSED,
        CHARGE_OUTCOME_UNKNOWN,
        CHARGE_FAILED,
        PAYMENT_METHOD_CHARGES_DISABLED,
        PAYMENT_PROCESSING_UNAVAILABLE,
        PAYMENT_METHOD_NOT_FOUND,
        CHARGE_IN_PROGRESS,
        CHARGE_CURRENCY_MISMATCH,
        AGREEMENT_NOT_FOUND,
        AGREEMENT_METHOD_MISMATCH,
        AGREEMENT_CONSENT_MISSING,
        AGREEMENT_CURRENCY_MISMATCH,
        AGREEMENT_AMOUNT_MISMATCH,
        CHARGE_AMOUNT_ABOVE_AGREEMENT,
        AGREEMENT_PERIOD_ALREADY_CHARGED,
        AGREEMENT_ENDED,
        AGREEMENT_NOT_STARTED,
        CHARGE_AMOUNT_ABOVE_CAP,
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

/// The codes of the plan and agreement routes
/// ([`Client::create_plan`](crate::Client::create_plan),
/// [`Client::create_agreement`](crate::Client::create_agreement) and the
/// reads, updates and deletes of both). They arrive as
/// [`Error::Api`](crate::Error::Api) with the code and the HTTP status set.
///
/// The codes of a subscription session are refusals on the checkout route:
/// see [`session_error_code::REFUSALS`].
pub mod recurring_error_code {
    /// HTTP 404: no plan with this id under your account. The `plan_` handle is
    /// case-sensitive.
    pub const PLAN_NOT_FOUND: &str = "PLAN_NOT_FOUND";
    /// HTTP 409: the plan is archived; create the agreement from an active
    /// plan.
    pub const PLAN_NOT_ACTIVE: &str = "PLAN_NOT_ACTIVE";
    /// HTTP 404: no agreement with this id under your account.
    pub const AGREEMENT_NOT_FOUND: &str = "AGREEMENT_NOT_FOUND";
    /// HTTP 409: the agreement already completed or failed, so there is
    /// nothing left to cancel.
    pub const AGREEMENT_ENDED: &str = "AGREEMENT_ENDED";
    /// HTTP 404: no stored payment method with the `stored_payment_method_id`
    /// you sent under your account.
    pub const PAYMENT_METHOD_NOT_FOUND: &str = "PAYMENT_METHOD_NOT_FOUND";
    /// HTTP 409: the stored payment method is revoked, expired or retired and
    /// cannot back an agreement.
    pub const PAYMENT_METHOD_NOT_ACTIVE: &str = "PAYMENT_METHOD_NOT_ACTIVE";
    /// HTTP 422: the stored payment method was saved in another currency than
    /// the plan's.
    pub const PAYMENT_METHOD_CURRENCY_MISMATCH: &str = "PAYMENT_METHOD_CURRENCY_MISMATCH";
    /// HTTP 422: `max_amount_per_period` is below the plan's amount.
    pub const MAX_AMOUNT_BELOW_PLAN_AMOUNT: &str = "MAX_AMOUNT_BELOW_PLAN_AMOUNT";
    /// HTTP 422: the agreement names a stored payment method or a
    /// `max_amount_per_period` but carries no consent.
    pub const AGREEMENT_CONSENT_REQUIRED: &str = "AGREEMENT_CONSENT_REQUIRED";
    /// HTTP 422: the currency of the stored payment method is not known, so it
    /// cannot back an agreement.
    pub const PAYMENT_METHOD_CURRENCY_UNKNOWN: &str = "PAYMENT_METHOD_CURRENCY_UNKNOWN";
    /// HTTP 422: this key was first used to create a different plan or
    /// agreement. Use a fresh key.
    pub const IDEMPOTENCY_KEY_REUSED: &str = "IDEMPOTENCY_KEY_REUSED";
    /// HTTP 400: the platform does not process this currency.
    pub const INVALID_CURRENCY: &str = "INVALID_CURRENCY";

    /// The whole vocabulary, in the order the contract lists it. An unlisted
    /// code still arrives as [`Error::Api`](crate::Error::Api) with its code.
    pub const ALL: [&str; 12] = [
        PLAN_NOT_FOUND,
        PLAN_NOT_ACTIVE,
        AGREEMENT_NOT_FOUND,
        AGREEMENT_ENDED,
        PAYMENT_METHOD_NOT_FOUND,
        PAYMENT_METHOD_NOT_ACTIVE,
        PAYMENT_METHOD_CURRENCY_MISMATCH,
        MAX_AMOUNT_BELOW_PLAN_AMOUNT,
        AGREEMENT_CONSENT_REQUIRED,
        PAYMENT_METHOD_CURRENCY_UNKNOWN,
        IDEMPOTENCY_KEY_REUSED,
        INVALID_CURRENCY,
    ];
}

/// The codes of the webhook endpoint routes
/// ([`Client::create_webhook_endpoint`](crate::Client::create_webhook_endpoint),
/// [`Client::list_webhook_endpoints`](crate::Client::list_webhook_endpoints)
/// and [`Client::delete_webhook_endpoint`](crate::Client::delete_webhook_endpoint)).
/// They arrive as [`Error::Api`](crate::Error::Api) with the code and the HTTP
/// status set.
pub mod webhook_error_code {
    /// HTTP 400: the url is not HTTPS, is malformed, does not resolve, or
    /// points at a private, loopback or link-local host.
    pub const WEBHOOK_URL_REJECTED: &str = "WEBHOOK_URL_REJECTED";
    /// HTTP 409: you already have the maximum number of active endpoints.
    pub const WEBHOOK_LIMIT_REACHED: &str = "WEBHOOK_LIMIT_REACHED";
    /// HTTP 409, create only: this key registered an endpoint more than a day
    /// ago and that endpoint still exists. Its id is in `raw`, under
    /// `error.params.webhookId`.
    pub const WEBHOOK_ALREADY_REGISTERED: &str = "WEBHOOK_ALREADY_REGISTERED";
    /// HTTP 404, delete only: no endpoint with this id under your account.
    pub const WEBHOOK_NOT_FOUND: &str = "WEBHOOK_NOT_FOUND";
    /// HTTP 422: this key was first used for a different url or events.
    pub const IDEMPOTENCY_KEY_REUSED: &str = "IDEMPOTENCY_KEY_REUSED";
    /// HTTP 409: a request with this key is being processed right now. Retry
    /// with the SAME key after a second.
    pub const DUPLICATE_REQUEST: &str = "DUPLICATE_REQUEST";
    /// HTTP 400, create only: the Idempotency-Key header is missing or longer
    /// than 100 characters.
    pub const IDEMPOTENCY_KEY_REQUIRED: &str = "IDEMPOTENCY_KEY_REQUIRED";

    /// The whole vocabulary, in the order the contract lists it. An unlisted
    /// code still arrives as [`Error::Api`](crate::Error::Api) with its code.
    pub const ALL: [&str; 7] = [
        WEBHOOK_URL_REJECTED,
        WEBHOOK_LIMIT_REACHED,
        WEBHOOK_ALREADY_REGISTERED,
        WEBHOOK_NOT_FOUND,
        IDEMPOTENCY_KEY_REUSED,
        DUPLICATE_REQUEST,
        IDEMPOTENCY_KEY_REQUIRED,
    ];
}

/// The codes of a rejected field, on an HTTP 400.
///
/// The envelope's own code is [`VALIDATION_ERROR`](validation_error_code::VALIDATION_ERROR). The field codes in
/// [`ALL`](validation_error_code::ALL) name what was wrong with each field: read them in the error's `raw`,
/// under `error.validationErrors[].code`, next to the field name.
pub mod validation_error_code {
    /// The `error.code` of a 400 that rejected one or more fields. The details
    /// are in `raw`, under `error.validationErrors`.
    pub const VALIDATION_ERROR: &str = "VALIDATION_ERROR";

    /// The field was missing or empty.
    pub const REQUIRED: &str = "REQUIRED";
    /// The amount must be a positive integer in minor units.
    pub const MIN_VALUE: &str = "MIN_VALUE";
    /// The value is longer than the maximum. The limit is in `params.max`.
    pub const MAX_LENGTH: &str = "MAX_LENGTH";
    /// The value does not have the expected format.
    pub const INVALID_FORMAT: &str = "INVALID_FORMAT";
    /// The value is not one of the accepted choices.
    pub const INVALID_SELECTION: &str = "INVALID_SELECTION";
    /// `consent.acceptedAt` is later than now.
    pub const DATE_IN_FUTURE: &str = "DATE_IN_FUTURE";
    /// The email address was rejected.
    pub const INVALID_EMAIL: &str = "INVALID_EMAIL";
    /// The format is fine but the platform does not process this currency.
    pub const INVALID_CURRENCY: &str = "INVALID_CURRENCY";

    /// The field codes, in the order the contract lists them.
    pub const ALL: [&str; 8] = [
        REQUIRED,
        MIN_VALUE,
        MAX_LENGTH,
        INVALID_FORMAT,
        INVALID_SELECTION,
        DATE_IN_FUTURE,
        INVALID_EMAIL,
        INVALID_CURRENCY,
    ];
}

/// The codes of a temporary outage. Nothing was created; send the same
/// request again later WITH THE SAME idempotency key.
pub mod transient_error_code {
    /// HTTP 503 on any route: the platform could not handle the request right
    /// now. It arrives as [`Error::Transport`](crate::Error::Transport) with
    /// this code.
    pub const MERCHANT_API_UNAVAILABLE: &str = "MERCHANT_API_UNAVAILABLE";
    /// Card payments are off right now. An HTTP 200
    /// [`Error::Refusal`](crate::Error::Refusal) on the checkout route, an HTTP
    /// 503 [`Error::Charge`](crate::Error::Charge) on the charge route.
    pub const PAYMENT_PROCESSING_UNAVAILABLE: &str = "PAYMENT_PROCESSING_UNAVAILABLE";

    /// The whole vocabulary, in the order the contract lists it.
    pub const ALL: [&str; 2] = [MERCHANT_API_UNAVAILABLE, PAYMENT_PROCESSING_UNAVAILABLE];
}

/// The codes the API answers with when it refuses the key itself, on any
/// route.
///
/// HTTP 401 is a credential problem and arrives as [`Error::Auth`]: fix the key
/// id, the secret, the server clock or the IP allowlist. HTTP 403 means the
/// request was signed correctly but this key may not do this, and arrives as
/// [`Error::Forbidden`]. Neither changes on a retry.
pub mod auth_error_code {
    /// HTTP 401: wrong or revoked key id.
    pub const INVALID_API_KEY: &str = "INVALID_API_KEY";
    /// HTTP 401: the signature does not match. Run the test vector again.
    pub const INVALID_SIGNATURE: &str = "INVALID_SIGNATURE";
    /// HTTP 401: your server clock is off by more than 5 minutes. Fix NTP.
    pub const TIMESTAMP_OUT_OF_RANGE: &str = "TIMESTAMP_OUT_OF_RANGE";
    /// HTTP 401: the key is IP-locked and this address is not on its
    /// allowlist.
    pub const IP_NOT_ALLOWED: &str = "IP_NOT_ALLOWED";
    /// HTTP 403: the key does not hold the scope the route needs (`payments`,
    /// `webhooks:manage` or `transactions:read`). A key's scopes are fixed when
    /// it is issued, so use a key that holds the scope.
    pub const INSUFFICIENT_SCOPE: &str = "INSUFFICIENT_SCOPE";
    /// HTTP 403, verify-only sessions only: verify-only is not on for your
    /// account. Nothing was created and the idempotency key is not burnt. We
    /// switch it on for your account.
    pub const VERIFY_ONLY_NOT_ENABLED: &str = "VERIFY_ONLY_NOT_ENABLED";

    /// The HTTP 401 codes, in the order the contract lists them. Each arrives
    /// as [`Error::Auth`](crate::Error::Auth).
    pub const AUTHENTICATION: [&str; 4] = [
        INVALID_API_KEY,
        INVALID_SIGNATURE,
        TIMESTAMP_OUT_OF_RANGE,
        IP_NOT_ALLOWED,
    ];

    /// The contract's authorization group (HTTP 403). Each arrives as
    /// [`Error::Forbidden`](crate::Error::Forbidden), and so does
    /// [`VERIFY_ONLY_NOT_ENABLED`], which the contract lists with verify-only
    /// sessions instead.
    pub const AUTHORIZATION: [&str; 1] = [INSUFFICIENT_SCOPE];
}

/// The codes an HTTP 429 carries, on [`Error::RateLimited`]. Wait for
/// `retry_after_seconds`, then send the same request again with the SAME
/// idempotency key.
pub mod rate_limit_error_code {
    /// Your key or your IP sent more requests than it is allowed to.
    pub const RATE_LIMIT_EXCEEDED: &str = "RATE_LIMIT_EXCEEDED";
    /// Charges only: a volume limit refused the charge before anything was
    /// placed. `error.params.window` in the raw body is the limit's window in
    /// seconds and `error.params.limit` the limit. A charge that keeps hitting
    /// this is a pattern to look into, not to retry around.
    pub const CHARGE_VELOCITY_EXCEEDED: &str = "CHARGE_VELOCITY_EXCEEDED";
    /// Charges only: this key collected too many declined charges in a short
    /// window, so its charge route is locked for a while. Nothing was charged
    /// and the key is not burnt; your other routes keep working. If this is a
    /// genuine billing run with many declines, contact support.
    pub const CHARGE_DECLINE_RATE_EXCEEDED: &str = "CHARGE_DECLINE_RATE_EXCEEDED";
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
        /// # let client = Client::new("dmk_x", "dms_y", dominaite::SANDBOX_BASE_URL)?;
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

    /// The API rejected your credentials or signature (HTTP 401). Not
    /// retryable: fix the key id, the secret, the server clock, or the caller
    /// allowlist.
    Auth {
        /// One of the [`auth_error_code::AUTHENTICATION`] codes:
        /// `INVALID_API_KEY`, `INVALID_SIGNATURE`, `TIMESTAMP_OUT_OF_RANGE`,
        /// `IP_NOT_ALLOWED`.
        code: String,
        /// The human-readable reason.
        message: String,
    },

    /// The request was signed correctly, but this key may not do this (HTTP
    /// 403). Not retryable, and not a credential problem: the key id, the
    /// secret and the clock are fine.
    ///
    /// - `INSUFFICIENT_SCOPE`: the key does not hold the scope the route needs.
    ///   A key's scopes are fixed when it is issued, so use a key that holds
    ///   the scope.
    /// - `VERIFY_ONLY_NOT_ENABLED`: a verify-only session, and verify-only is
    ///   not on for your account. Nothing was created. We switch it on for
    ///   your account.
    Forbidden {
        /// One of the [`auth_error_code`] 403 codes, or a code this SDK does
        /// not know yet.
        code: String,
        /// The reason, with what to do about it.
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
        /// The whole envelope as the API sent it, for what the fields above do
        /// not carry: `error.validationErrors` on a 400, and `error.params`,
        /// e.g. the `webhookId` that a 409 `WEBHOOK_ALREADY_REGISTERED` names.
        ///
        /// `Value::Null` when the body was not JSON, and for the errors this
        /// crate raises itself.
        raw: Value,
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
    /// # let client = Client::new("dmk_x", "dms_y", dominaite::SANDBOX_BASE_URL)?;
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
    /// whose status is `failed`. A 404 for an id that is not yours IS this
    /// variant, with code `PAYMENT_METHOD_NOT_FOUND`. A 400 is [`Error::Api`].
    /// A 5xx other than 502 and 503 (an HTML page from a proxy, or a coded 500)
    /// is [`Error::Transport`].
    Charge {
        /// The HTTP status code: 404, 409, 422, 502 or 503.
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
    /// [`Error::Api`] with status 404. A 5xx other than 502 and 503 is
    /// [`Error::Transport`].
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
    /// this. The charge route has two limits of its own; `code` tells them
    /// apart (see [`rate_limit_error_code`]).
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
        /// The machine-readable reason, e.g. `RATE_LIMIT_EXCEEDED`. See
        /// [`rate_limit_error_code`]. Also on [`Error::code`].
        ///
        /// `None` when the body carried no code, e.g. a limiter in front of the
        /// API answered.
        code: Option<String>,
        /// The whole envelope, for fields not modelled above (a charge limit's
        /// `error.params`). `Value::Null` when the body was not JSON.
        raw: Value,
    },

    /// A network-level failure, a timeout, or a 5xx. The request may or may not
    /// have reached the API, so retry WITH THE SAME idempotency key: a retried
    /// key never creates a second payment.
    ///
    /// A 5xx from the API itself keeps its status, its code and its body, e.g.
    /// a 503 with
    /// [`MERCHANT_API_UNAVAILABLE`](crate::transient_error_code::MERCHANT_API_UNAVAILABLE).
    Transport {
        /// What failed.
        message: String,
        /// The HTTP status of a 5xx. `None` when no answer came back.
        status: Option<u16>,
        /// The machine-readable reason, when the 5xx carried one. Also on
        /// [`Error::code`].
        code: Option<String>,
        /// The whole envelope of a 5xx that sent JSON. `Value::Null` otherwise.
        raw: Value,
        /// The underlying cause, when there was one.
        source: Option<Box<dyn StdError + Send + Sync>>,
    },
}

impl Error {
    /// The machine-readable code, for the variants that carry one.
    ///
    /// `Refusal`, `Auth`, `Forbidden`, `Charge` and `Revoke` always have one,
    /// `Api`, `RateLimited` and `Transport` have one when the API sent it;
    /// `Validation` returns `None`.
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::Refusal { code, .. }
            | Error::Auth { code, .. }
            | Error::Forbidden { code, .. }
            | Error::Charge { code, .. }
            | Error::Revoke { code, .. } => Some(code),
            Error::Api { code, .. }
            | Error::RateLimited { code, .. }
            | Error::Transport { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    /// The HTTP status, for the variants that carry one.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. }
            | Error::Charge { status, .. }
            | Error::Revoke { status, .. } => Some(*status),
            Error::Auth { .. } => Some(401),
            Error::Forbidden { .. } => Some(403),
            Error::RateLimited { .. } => Some(429),
            Error::Transport { status, .. } => *status,
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

    /// A 403. The message says what to do for the codes this SDK knows; the
    /// API's own message is kept where it is the useful part (it names the
    /// scope the route needs).
    pub(crate) fn forbidden(code: Option<String>, api_message: Option<String>) -> Error {
        let code = code.unwrap_or_else(|| "FORBIDDEN".to_string());
        let api_message = api_message.filter(|message| !message.trim().is_empty());
        let message = match code.as_str() {
            auth_error_code::INSUFFICIENT_SCOPE => format!(
                "{}. A key's scopes are fixed when it is issued: use a key that holds this scope.",
                api_message
                    .as_deref()
                    .unwrap_or("This API key does not have the scope this route requires")
                    .trim_end_matches('.')
            ),
            auth_error_code::VERIFY_ONLY_NOT_ENABLED => {
                "Verify-only sessions are not enabled for your account. Nothing was created. \
                 We switch it on for your account: contact Dominaite support."
                    .to_string()
            }
            _ => api_message.unwrap_or_else(|| {
                "Access denied: this API key may not call this route.".to_string()
            }),
        };
        Error::Forbidden { code, message }
    }

    /// An API error with no machine-readable code: the responses this crate
    /// raises itself when a 200 body is not what the contract promises.
    pub(crate) fn api(status: u16, message: impl Into<String>) -> Error {
        Error::Api {
            status,
            code: None,
            message: message.into(),
            raw: Value::Null,
        }
    }

    pub(crate) fn api_with_code(
        status: u16,
        code: Option<String>,
        message: impl Into<String>,
        raw: Value,
    ) -> Error {
        Error::Api {
            status,
            code,
            message: message.into(),
            raw,
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

    /// A failure with no answer from the API: nothing to read a status or a
    /// code from.
    pub(crate) fn transport(
        message: impl Into<String>,
        source: Option<Box<dyn StdError + Send + Sync>>,
    ) -> Error {
        Error::Transport {
            message: message.into(),
            status: None,
            code: None,
            raw: Value::Null,
            source,
        }
    }

    /// A 5xx. The code, the API's message and the body travel with it when the
    /// API sent them.
    pub(crate) fn unavailable(
        status: u16,
        code: Option<String>,
        api_message: Option<String>,
        raw: Value,
    ) -> Error {
        let mut message = format!("The Dominaite API is unavailable (HTTP {status}");
        if let Some(code) = &code {
            message.push_str(", ");
            message.push_str(code);
        }
        message.push(')');
        if let Some(api_message) = api_message.filter(|text| !text.trim().is_empty()) {
            message.push_str(": ");
            message.push_str(api_message.trim_end_matches('.'));
        }
        message.push_str("; retry with the same idempotency key.");
        Error::Transport {
            message,
            status: Some(status),
            code,
            raw,
            source: None,
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
            Error::Forbidden { code, message } => {
                write!(f, "access denied (HTTP 403, {code}): {message}")
            }
            Error::Api {
                status,
                code: Some(code),
                message,
                ..
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
                retry_after_seconds,
                code,
                ..
            } => {
                match code {
                    Some(code) => write!(f, "rate limited (HTTP 429, {code}): ")?,
                    None => write!(f, "rate limited (HTTP 429): ")?,
                }
                match retry_after_seconds {
                    Some(seconds) => write!(f, "retry after {seconds}s"),
                    None => write!(f, "back off and retry"),
                }
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
