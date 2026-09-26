//! Request and response shapes.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::idempotency::IdempotencyKey;

/// Optional payer details. Prefilled fields are hidden from the payer in the
/// widget, so the checkout form stays short.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Customer {
    /// Payer first name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    /// Payer last name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    /// Payer email.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Payer phone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

impl Customer {
    /// An empty customer to fill in with the builder methods.
    pub fn new() -> Customer {
        Customer::default()
    }

    /// Sets the first name.
    pub fn first_name(mut self, value: impl Into<String>) -> Customer {
        self.first_name = Some(value.into());
        self
    }

    /// Sets the last name.
    pub fn last_name(mut self, value: impl Into<String>) -> Customer {
        self.last_name = Some(value.into());
        self
    }

    /// Sets the email.
    pub fn email(mut self, value: impl Into<String>) -> Customer {
        self.email = Some(value.into());
        self
    }

    /// Sets the phone.
    pub fn phone(mut self, value: impl Into<String>) -> Customer {
        self.phone = Some(value.into());
        self
    }
}

/// The parameters for [`Client::create_checkout_session`](crate::Client::create_checkout_session).
///
/// `amount`, `currency`, `order_reference` and the idempotency key are required
/// and come from [`CheckoutSessionRequest::new`]; everything else is a builder
/// method.
///
/// ```
/// use dominaite::{CheckoutSessionRequest, Customer, IdempotencyKey};
///
/// # fn main() -> Result<(), dominaite::Error> {
/// let key = IdempotencyKey::for_order("shop-a1b2c3d4", "order-1042", 2500, "EUR")?;
/// let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key)
///     .customer(Customer::new().first_name("Ana").email("ana@example.com"))
///     .language("bg");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutSessionRequest {
    /// The amount in MINOR units: `2500` is 25.00 EUR. Integers only.
    pub amount: i64,
    /// ISO 4217 currency, e.g. `"EUR"`.
    pub currency: String,
    /// Your own order id, at most 100 characters. It shows up in your dashboard.
    pub order_reference: String,

    /// Optional payer details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<Customer>,
    /// ISO 3166-1 alpha-2 country.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// ISO 639-1 widget UI language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// `"light"`, `"dark"` or `"bright"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Free-text description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Ask the gateway to keep the card on file once this payment is approved, so
    /// you can charge it again later with
    /// [`Client::charge_payment_method`](crate::Client::charge_payment_method).
    /// The stored method shows up on [`CheckoutStatus::stored_payment_method`]
    /// after the payment succeeds; a declined first payment stores nothing. The
    /// card details themselves never reach you: you get an id, a brand and the
    /// last four digits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_card: Option<bool>,

    /// Required. It travels in the header and in the signature, never in the
    /// body. Retrying with the same key never creates a second payment, so on a
    /// timeout retry with the same key. See [`IdempotencyKey::for_order`] for
    /// the recommended order-derived key.
    #[serde(skip)]
    pub idempotency_key: IdempotencyKey,

    /// Any additional field the API accepts that this struct does not model yet.
    /// These are merged into the JSON body.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CheckoutSessionRequest {
    /// A request for one payment. The amount is in MINOR units. The key is
    /// required: build it with [`IdempotencyKey::for_order`] so a reload of the
    /// same order replays its session instead of opening a second one.
    pub fn new(
        amount: i64,
        currency: impl Into<String>,
        order_reference: impl Into<String>,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        CheckoutSessionRequest {
            amount,
            currency: currency.into(),
            order_reference: order_reference.into(),
            customer: None,
            country: None,
            language: None,
            theme: None,
            description: None,
            save_card: None,
            idempotency_key,
            extra: Map::new(),
        }
    }

    /// Prefills payer details.
    pub fn customer(mut self, customer: Customer) -> Self {
        self.customer = Some(customer);
        self
    }

    /// Sets the payer country (ISO 3166-1 alpha-2).
    pub fn country(mut self, value: impl Into<String>) -> Self {
        self.country = Some(value.into());
        self
    }

    /// Sets the widget UI language (ISO 639-1).
    pub fn language(mut self, value: impl Into<String>) -> Self {
        self.language = Some(value.into());
        self
    }

    /// Sets the widget theme: `"light"`, `"dark"` or `"bright"`.
    pub fn theme(mut self, value: impl Into<String>) -> Self {
        self.theme = Some(value.into());
        self
    }

    /// Sets the description.
    pub fn description(mut self, value: impl Into<String>) -> Self {
        self.description = Some(value.into());
        self
    }

    /// Keeps the card on file once this payment succeeds. See
    /// [`CheckoutSessionRequest::save_card`].
    pub fn save_card(mut self, value: bool) -> Self {
        self.save_card = Some(value);
        self
    }

    /// Adds a body field this struct does not model yet.
    pub fn extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

/// What [`Client::create_checkout_session`](crate::Client::create_checkout_session) returns.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutSession {
    /// Dominaite's payment id. Store it against your order; poll status with it.
    pub transaction_id: String,
    /// The provider-facing correlation id (`dom_...`). You never need it.
    #[serde(default)]
    pub order_id: String,
    /// Feeds the widget's `data-cashier-key`. A per-payment value, not a credential.
    #[serde(default)]
    pub cashier_key: String,
    /// Feeds the widget's `data-cashier-token`. A per-payment value, not a credential.
    #[serde(default)]
    pub cashier_token: String,
    /// The amount in MINOR units.
    #[serde(default)]
    pub amount: i64,
    /// ISO 4217 currency.
    #[serde(default)]
    pub currency: String,
    /// ISO 8601. Sessions are valid for about 2 hours.
    #[serde(default)]
    pub expires_at: Option<String>,

    /// The unparsed payload, for fields this struct does not model yet.
    #[serde(skip)]
    pub raw: Value,
}

/// Transaction status wire values returned by
/// [`Client::get_status`](crate::Client::get_status).
pub mod status {
    /// The session exists and nobody has paid yet.
    pub const PENDING: &str = "pending";
    /// The payment is in flight.
    pub const PROCESSING: &str = "processing";
    /// The customer paid. The ONLY value that means paid.
    pub const SUCCEEDED: &str = "succeeded";
    /// The payment failed.
    pub const FAILED: &str = "failed";
    /// Paid and then fully returned.
    pub const REFUNDED: &str = "refunded";
    /// Paid and then partly returned.
    pub const PARTIALLY_REFUNDED: &str = "partially_refunded";
    /// The payment was cancelled.
    pub const CANCELLED: &str = "cancelled";
    /// The payment is disputed. Not terminal: the dispute can still resolve
    /// either way, so keep polling.
    pub const DISPUTED: &str = "disputed";
    /// Authorized, awaiting capture.
    pub const REQUIRES_CAPTURE: &str = "requires_capture";
    /// The payer never paid and the session aged out.
    pub const ABANDONED: &str = "abandoned";

    /// The whole vocabulary, in the order the canonical contract lists it. This is
    /// the enumerable form of the constants above; `tests/contract.rs` pins it
    /// against the vendored `merchant-api-contract.json`, so a status the API adds
    /// cannot land in one SDK and be missed here.
    pub const ALL: [&str; 10] = [
        PENDING,
        PROCESSING,
        SUCCEEDED,
        FAILED,
        REFUNDED,
        PARTIALLY_REFUNDED,
        CANCELLED,
        DISPUTED,
        REQUIRES_CAPTURE,
        ABANDONED,
    ];
}

/// What [`Client::get_status`](crate::Client::get_status) returns.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutStatus {
    /// Dominaite's payment id.
    pub transaction_id: String,
    /// The provider-facing correlation id.
    #[serde(default)]
    pub order_id: String,
    /// Your own order id, echoed back.
    #[serde(default)]
    pub order_reference: Option<String>,
    /// One of the [`status`] constants. Compare with [`CheckoutStatus::is_paid`]
    /// rather than by hand.
    pub status: String,
    /// The amount in MINOR units.
    #[serde(default)]
    pub amount: i64,
    /// ISO 4217 currency.
    #[serde(default)]
    pub currency: String,
    /// How much of the amount has been returned, in MINOR units.
    #[serde(default)]
    pub refunded_amount: Option<i64>,
    /// ISO 8601 creation time.
    #[serde(default)]
    pub created_at: Option<String>,
    /// ISO 8601 last-change time.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// Present only while the session is still payable.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// The card kept on file for this payment. Present once a session created
    /// with [`CheckoutSessionRequest::save_card`] has been approved, and it stays
    /// present after a revoke with status `revoked`; `None` (absent on the wire)
    /// until then, for sessions without `save_card`, and for declined or
    /// abandoned ones. Store `stored_payment_method.id` against your customer -
    /// it is what [`Client::charge_payment_method`](crate::Client::charge_payment_method)
    /// takes.
    ///
    /// Not to be confused with the gateway's `paymentMethod` field, which is the
    /// string category of how the payer paid (`card`, `wallet`, ...) and stays
    /// on [`CheckoutStatus::raw`] untyped.
    #[serde(default)]
    pub stored_payment_method: Option<StoredPaymentMethod>,

    /// The unparsed payload, for fields this struct does not model yet.
    #[serde(skip)]
    pub raw: Value,
}

impl CheckoutStatus {
    /// True only for `succeeded`. `refunded` and `partially_refunded` mean the
    /// customer paid and was then (partly) returned, which is a different
    /// question - ask it explicitly if you need it.
    ///
    /// `requires_capture` is false here too, but it is NOT "unpaid": the payer
    /// has already paid and the funds are held awaiting capture. Keep polling it
    /// rather than treating it as an abandoned order.
    pub fn is_paid(&self) -> bool {
        self.status == status::SUCCEEDED
    }

    /// False while the payment can still change, true once it cannot.
    ///
    /// Terminal: `succeeded`, `failed`, `cancelled`, `abandoned`, `refunded`
    /// and `partially_refunded`. Keep polling on `pending`, `processing`,
    /// `requires_capture` and `disputed`: a dispute is still open and can go
    /// either way.
    ///
    /// An unrecognised status is reported as NOT terminal, so a status the API
    /// adds later makes you keep polling rather than silently close an order
    /// that is still open.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            status::SUCCEEDED
                | status::FAILED
                | status::REFUNDED
                | status::PARTIALLY_REFUNDED
                | status::CANCELLED
                | status::ABANDONED
        )
    }
}

/// What [`Client::ping`](crate::Client::ping) returns: proof that your key,
/// secret, signing and clock are all good, without creating anything.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ping {
    /// Always true on a 200.
    #[serde(default)]
    pub pong: bool,
    /// The merchant id your key authenticated as.
    #[serde(default)]
    pub merchant_id: String,
    /// Server time, ISO 8601.
    #[serde(default)]
    pub server_time: Option<String>,
    /// Server time in unix seconds.
    #[serde(default)]
    pub server_unix_time: Option<i64>,
    /// Server time minus your `X-Timestamp`. If its absolute value creeps toward
    /// 300, fix NTP now - requests start failing at 300.
    #[serde(default)]
    pub clock_skew_seconds: i64,

    /// The unparsed payload, for fields this struct does not model yet.
    #[serde(skip)]
    pub raw: Value,
}

/// Stored payment method status wire values, on [`StoredPaymentMethod::status`].
pub mod stored_payment_method_status {
    /// Chargeable.
    pub const ACTIVE: &str = "active";
    /// What [`Client::revoke_payment_method`](crate::Client::revoke_payment_method)
    /// leaves behind. A charge on it is refused with `PAYMENT_METHOD_NOT_ACTIVE`.
    pub const REVOKED: &str = "revoked";
    /// The card's expiry date has passed.
    pub const EXPIRED: &str = "expired";
    /// The platform stopped the card on its own; `retired_reason` says why. It
    /// never becomes active again, so ask the customer to save a card again.
    pub const RETIRED: &str = "retired";

    /// The whole vocabulary, in the order the canonical contract lists it. Treat
    /// a value outside it as not chargeable.
    pub const ALL: [&str; 4] = [ACTIVE, REVOKED, EXPIRED, RETIRED];
}

/// Why the platform retired a stored payment method, as
/// [`StoredPaymentMethod::retired_reason`] carries it. Treat a value outside
/// [`ALL`](retired_reason::ALL) as retired for an unknown reason.
pub mod retired_reason {
    /// A charge on it was declined as final.
    pub const HARD_DECLINE: &str = "hard_decline";
    /// A charge on it was disputed.
    pub const CHARGEBACK: &str = "chargeback";
    /// The payment that saved it was fully refunded or disputed.
    pub const SOURCE_SALE_REVERSED: &str = "source_sale_reversed";

    /// The whole vocabulary, in the order the canonical contract lists it.
    pub const ALL: [&str; 3] = [HARD_DECLINE, CHARGEBACK, SOURCE_SALE_REVERSED];
}

/// A card kept on file. Never the card number, never the PSP token - only what
/// you may show a customer. `brand`, `last4` and the expiry are `None` when the
/// provider did not report them (the gateway omits null fields on the wire; the
/// SDK reads absent as `None`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredPaymentMethod {
    /// Opaque id: `pm_` followed by 32 hex characters, case-sensitive. The
    /// handle you charge and revoke with.
    pub id: String,
    /// Card brand as the gateway reports it, e.g. `"visa"`, `"mastercard"`.
    #[serde(default)]
    pub brand: Option<String>,
    /// Last four digits of the card number, for display only.
    #[serde(default)]
    pub last4: Option<String>,
    /// 1 to 12.
    #[serde(default)]
    pub expiry_month: Option<u8>,
    /// Four digits, e.g. 2029.
    #[serde(default)]
    pub expiry_year: Option<u16>,
    /// One of the [`stored_payment_method_status`] constants. Compare with
    /// [`StoredPaymentMethod::is_chargeable`] rather than by hand.
    pub status: String,
    /// One of the [`retired_reason`] constants when the platform retired the
    /// card, kept if you revoke it afterwards. `None` on every other card.
    #[serde(default)]
    pub retired_reason: Option<String>,
}

impl StoredPaymentMethod {
    /// True only for `active`. An unrecognised status is reported as NOT
    /// chargeable, so a status the API adds later never charges a card the
    /// gateway would refuse anyway.
    pub fn is_chargeable(&self) -> bool {
        self.status == stored_payment_method_status::ACTIVE
    }
}

/// The parameters for [`Client::charge_payment_method`](crate::Client::charge_payment_method).
///
/// `amount`, `currency`, `order_reference` and the idempotency key are required
/// and come from [`ChargeRequest::new`]; the rest are builder methods. The body
/// is exactly these fields, in this order - it is what gets signed.
///
/// ```
/// use dominaite::{ChargeRequest, IdempotencyKey};
///
/// # fn main() -> Result<(), dominaite::Error> {
/// let key = IdempotencyKey::new("sub-8817-2026-10")?;
/// let request = ChargeRequest::new(2500, "EUR", "sub-8817-2026-10", key)
///     .description("Monthly plan, October");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChargeRequest {
    /// The amount in MINOR units: `2500` is 25.00 EUR. Integers only.
    pub amount: i64,
    /// ISO 4217 currency, e.g. `"EUR"`.
    pub currency: String,
    /// Your own order id, at most 100 characters. It shows up in your dashboard.
    pub order_reference: String,
    /// Free-text description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Required. It travels in the header and in the signature, never in the
    /// body. Retrying with the same key never charges the card twice, so on a
    /// timeout retry with the same key. Derive it from the billing period,
    /// never mint one per attempt.
    #[serde(skip)]
    pub idempotency_key: IdempotencyKey,
}

impl ChargeRequest {
    /// A charge for one payment. The amount is in MINOR units. The key is
    /// required: derive it from what is being billed (the subscription and its
    /// period, say), so a retried charge carries the same one.
    pub fn new(
        amount: i64,
        currency: impl Into<String>,
        order_reference: impl Into<String>,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        ChargeRequest {
            amount,
            currency: currency.into(),
            order_reference: order_reference.into(),
            description: None,
            idempotency_key,
        }
    }

    /// Sets the description.
    pub fn description(mut self, value: impl Into<String>) -> Self {
        self.description = Some(value.into());
        self
    }
}

/// Charge status wire values, on [`PaymentMethodCharge::status`].
pub mod charge_status {
    /// The card was charged.
    pub const SUCCEEDED: &str = "succeeded";
    /// The issuer declined (HTTP 402); see
    /// [`PaymentMethodCharge::decline_class`](crate::PaymentMethodCharge::decline_class).
    pub const FAILED: &str = "failed";
    /// Not terminal: poll [`Client::get_status`](crate::Client::get_status)
    /// with the charge's transaction id.
    pub const PENDING: &str = "pending";
    /// An authorization voided before capture; no money moved.
    pub const CANCELLED: &str = "cancelled";

    /// The whole vocabulary, in the order the canonical contract lists it. Treat
    /// a value outside it as still open.
    pub const ALL: [&str; 4] = [SUCCEEDED, FAILED, PENDING, CANCELLED];
}

/// Decline class wire values, on [`PaymentMethodCharge::decline_class`](crate::PaymentMethodCharge::decline_class). Coarse
/// enough to act on without reading the issuer's code.
pub mod decline_class {
    /// Do not retry this card; ask the customer for another one.
    pub const HARD: &str = "hard";
    /// Insufficient funds; retry later (after the customer's payday, not in a loop).
    pub const SOFT_FUNDS: &str = "soft_funds";
    /// The issuer wants the customer present; send them through a hosted
    /// checkout session with `save_card` instead of charging off-session again.
    pub const SOFT_SCA_REQUIRED: &str = "soft_sca_required";
    /// A transient issuer or network condition; one retry later is reasonable.
    pub const SOFT_OTHER: &str = "soft_other";

    /// The whole vocabulary, in the order the canonical contract lists it.
    pub const ALL: [&str; 4] = [HARD, SOFT_FUNDS, SOFT_SCA_REQUIRED, SOFT_OTHER];
}

/// What [`Client::charge_payment_method`](crate::Client::charge_payment_method)
/// returns, for a placed charge (HTTP 201) and for a provider decline (HTTP 402,
/// `status` `failed`) alike. A decline is a result, not an error: `decline_class`
/// says what to do next. Also carried on [`Error::Charge`](crate::Error::Charge)
/// when the gateway attached the charge row to its answer.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentMethodCharge {
    /// `ch_` followed by 32 hex characters. Store it against the order; it is
    /// what support asks for.
    pub charge_id: String,
    /// One of the [`charge_status`] constants. Compare with
    /// [`PaymentMethodCharge::is_paid`] and [`PaymentMethodCharge::is_terminal`]
    /// rather than by hand.
    pub status: String,
    /// One of the [`decline_class`] constants, set on a 402 decline; `None`
    /// everywhere else (the SDK reads absent as `None`).
    #[serde(default)]
    pub decline_class: Option<String>,
    /// The raw decline code, for your logs; branch on `decline_class` instead.
    /// `None` when `decline_class` is.
    #[serde(default)]
    pub decline_code: Option<String>,
    /// The transaction the charge created; readable with
    /// [`Client::get_status`](crate::Client::get_status).
    #[serde(default)]
    pub transaction_id: String,
    /// The charge's current sequence, the counter that orders the `charge.*`
    /// webhooks ([`WebhookEvent::sequence`](crate::WebhookEvent::sequence)).
    /// `None` when the server does not send it yet.
    #[serde(default)]
    pub sequence: Option<i64>,

    /// The unwrapped charge object as the gateway sent it, for fields this
    /// struct does not model yet.
    #[serde(skip)]
    pub raw: Value,
}

impl PaymentMethodCharge {
    /// True only for `succeeded`.
    pub fn is_paid(&self) -> bool {
        self.status == charge_status::SUCCEEDED
    }

    /// False while the charge can still change, true once it cannot.
    ///
    /// An unrecognised status is reported as NOT terminal, so a status the API
    /// adds later keeps you polling rather than silently closing a live charge.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            charge_status::SUCCEEDED | charge_status::FAILED | charge_status::CANCELLED
        )
    }
}

/// The parameters for [`Client::create_refund`](crate::Client::create_refund).
///
/// The idempotency key is required and comes from [`RefundRequest::new`]; the
/// amount and reason are builder methods. Leave the amount out to refund
/// everything still refundable: the body then carries no `amount` at all, not a
/// null. The body is exactly these fields, in this order - it is what gets
/// signed.
///
/// ```
/// use dominaite::{to_minor_units, IdempotencyKey, RefundRequest};
///
/// # fn main() -> Result<(), dominaite::Error> {
/// // A partial refund of 1,500 HUF (HUF has no minor unit at the gateway).
/// let key = IdempotencyKey::new("return-7731")?;
/// let partial = RefundRequest::new(key)
///     .amount(to_minor_units("1500", "HUF")?)
///     .reason("Returned one item");
///
/// // Everything still refundable on the payment.
/// let full = RefundRequest::new(IdempotencyKey::new("credit-note-2291")?);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundRequest {
    /// The amount to refund in MINOR units of the payment's currency, at least
    /// 1. `None` refunds everything still refundable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<i64>,
    /// Free text stored with the refund, at most 500 characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Required. It travels in the header and in the signature, never in the
    /// body. Derive it from YOUR refund (the return or credit-note id) and resend
    /// it as is: the same key answers the same refund and never refunds twice.
    #[serde(skip)]
    pub idempotency_key: IdempotencyKey,
}

impl RefundRequest {
    /// A refund of everything still refundable on the payment. Set
    /// [`RefundRequest::amount`] for a partial one.
    pub fn new(idempotency_key: IdempotencyKey) -> Self {
        RefundRequest {
            amount: None,
            reason: None,
            idempotency_key,
        }
    }

    /// Refunds this amount, in MINOR units, instead of everything still
    /// refundable.
    pub fn amount(mut self, amount: i64) -> Self {
        self.amount = Some(amount);
        self
    }

    /// Sets the reason stored with the refund.
    pub fn reason(mut self, value: impl Into<String>) -> Self {
        self.reason = Some(value.into());
        self
    }
}

/// Refund status wire values, on [`Refund::status`].
pub mod refund_status {
    /// Accepted and queued, behind another refund of the same payment or not yet
    /// picked up.
    pub const PENDING: &str = "pending";
    /// With the payment provider now.
    pub const PROCESSING: &str = "processing";
    /// The money was returned to the payer. Final.
    pub const SUCCEEDED: &str = "succeeded";
    /// The refund did not happen; read
    /// [`Refund::failure_code`](crate::Refund::failure_code). Final for this
    /// idempotency key: a new attempt needs a new key.
    pub const FAILED: &str = "failed";

    /// The whole vocabulary, in the order the canonical contract lists it. Treat
    /// a value outside it as still open.
    pub const ALL: [&str; 4] = [PENDING, PROCESSING, SUCCEEDED, FAILED];
}

/// The codes a failed [`Refund`] carries in [`Refund::failure_code`]. They are
/// not HTTP errors: the refund was accepted and then did not happen.
pub mod refund_failure_code {
    /// The amount was more than what was left to refund.
    pub const REFUND_AMOUNT_EXCEEDED: &str = "REFUND_AMOUNT_EXCEEDED";
    /// The payment could no longer be refunded.
    pub const PAYMENT_NOT_REFUNDABLE: &str = "PAYMENT_NOT_REFUNDABLE";
    /// The refund could not be completed. Also what an unknown code means; see
    /// [`Refund::failure`](crate::Refund::failure).
    pub const REFUND_FAILED: &str = "REFUND_FAILED";

    /// The whole vocabulary, in the order the canonical contract lists it.
    pub const ALL: [&str; 3] = [
        REFUND_AMOUNT_EXCEEDED,
        PAYMENT_NOT_REFUNDABLE,
        REFUND_FAILED,
    ];
}

/// What [`Client::create_refund`](crate::Client::create_refund) (HTTP 202) and
/// [`Client::get_refund`](crate::Client::get_refund) (HTTP 200) return. The
/// gateway omits null fields on the wire; the SDK reads absent as `None`.
///
/// A 202 means the refund is queued, not done. Read it back with `get_refund`,
/// or wait for the `payment.refunded` webhook, which fires once the money has
/// moved. A failed refund sends no webhook, so poll if you need to know about
/// failures.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Refund {
    /// `re_` followed by 32 hex characters. The same key on the same payment
    /// always names the same refund.
    pub refund_id: String,
    /// The payment being refunded.
    pub transaction_id: String,
    /// One of the [`refund_status`] constants. Compare with
    /// [`Refund::is_succeeded`] and [`Refund::is_terminal`] rather than by hand.
    pub status: String,
    /// MINOR units. Before success, the amount requested (`None` for a refund of
    /// everything still refundable); on `succeeded`, the amount actually
    /// refunded; always `None` on `failed`.
    #[serde(default)]
    pub amount: Option<i64>,
    /// ISO 4217 code of the payment. A refund is always in the payment's
    /// currency.
    #[serde(default)]
    pub currency: String,
    /// On `failed` only: one of the [`refund_failure_code`] constants. Branch on
    /// [`Refund::failure`], which folds an unknown code into `REFUND_FAILED`.
    #[serde(default)]
    pub failure_code: Option<String>,
    /// On `failed` only: a fixed English explanation of `failure_code`.
    #[serde(default)]
    pub failure_message: Option<String>,
    /// ISO 8601 UTC, when the refund reached `succeeded` or `failed`; `None`
    /// before that.
    #[serde(default)]
    pub completed_at: Option<String>,

    /// The unwrapped refund object as the gateway sent it, for fields this
    /// struct does not model yet.
    #[serde(skip)]
    pub raw: Value,
}

impl Refund {
    /// True only for `succeeded`: the money went back to the payer.
    pub fn is_succeeded(&self) -> bool {
        self.status == refund_status::SUCCEEDED
    }

    /// False while the refund can still change, true once it cannot
    /// (`succeeded` or `failed`).
    ///
    /// An unrecognised status is reported as NOT terminal, so a status the API
    /// adds later keeps you polling rather than closing a refund that is still
    /// open.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            refund_status::SUCCEEDED | refund_status::FAILED
        )
    }

    /// Why a failed refund did not happen, as one of the
    /// [`refund_failure_code`] constants. An unknown or missing code on a failed
    /// refund reads as `REFUND_FAILED`. `None` unless the status is `failed`.
    pub fn failure(&self) -> Option<&'static str> {
        if self.status != refund_status::FAILED {
            return None;
        }
        let code = self.failure_code.as_deref().unwrap_or("");
        Some(
            refund_failure_code::ALL
                .into_iter()
                .find(|known| *known == code)
                .unwrap_or(refund_failure_code::REFUND_FAILED),
        )
    }
}
