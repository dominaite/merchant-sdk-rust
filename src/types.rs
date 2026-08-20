//! Request and response shapes.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
/// `amount`, `currency` and `order_reference` are required and come from
/// [`CheckoutSessionRequest::new`]; everything else is a builder method.
///
/// ```
/// use dominaite::{CheckoutSessionRequest, Customer};
///
/// let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042")
///     .customer(Customer::new().first_name("Ana").email("ana@example.com"))
///     .language("bg");
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

    /// Auto-generated when unset. It travels in the header and in the signature,
    /// never in the body. Retrying with the same key never creates a second
    /// payment, so on a timeout retry with the same key.
    #[serde(skip)]
    pub idempotency_key: Option<String>,

    /// Any additional field the API accepts that this struct does not model yet.
    /// These are merged into the JSON body.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CheckoutSessionRequest {
    /// A request for one payment. The amount is in MINOR units.
    pub fn new(
        amount: i64,
        currency: impl Into<String>,
        order_reference: impl Into<String>,
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
            idempotency_key: None,
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

    /// Pins the idempotency key instead of letting the SDK generate one. Reuse
    /// the same key when you retry a call that failed at the transport level.
    pub fn idempotency_key(mut self, value: impl Into<String>) -> Self {
        self.idempotency_key = Some(value.into());
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
    /// The payment is disputed.
    pub const DISPUTED: &str = "disputed";
    /// Authorized, awaiting capture.
    pub const REQUIRES_CAPTURE: &str = "requires_capture";
    /// The payer never paid and the session aged out.
    pub const ABANDONED: &str = "abandoned";
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

    /// The unparsed payload, for fields this struct does not model yet.
    #[serde(skip)]
    pub raw: Value,
}

impl CheckoutStatus {
    /// True only for `succeeded`. `refunded` and `partially_refunded` mean the
    /// customer paid and was then (partly) returned, which is a different
    /// question - ask it explicitly if you need it.
    pub fn is_paid(&self) -> bool {
        self.status == status::SUCCEEDED
    }

    /// False while the payment can still change, true once it cannot.
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
                | status::DISPUTED
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
