//! Request and response shapes.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::idempotency::IdempotencyKey;

/// Reads a JSON boolean, and null or absent as false. The gateway omits a flag
/// that is not set, and a null must never fail the whole response.
pub(crate) fn false_when_null<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(false))
}

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

/// A customer's acceptance of the terms for recurring charges, sent with a
/// subscription session or an agreement.
///
/// All three fields are required together. What `text_version` names depends
/// on where you send it:
///
/// - On a subscription session ([`CheckoutSessionRequest::consent`]): a
///   version of the PLATFORM consent text, never your own wording. The
///   contract lists the published versions in `recurring.consentTextVersions`
///   (today `recurring-2026-09`).
/// - On an agreement ([`AgreementRequest::consent`]): your own label for the
///   wording the customer accepted, 1 to 50 characters, e.g. `terms-2026-09`.
///   The subscription session uses the agreement's consent only when this
///   label is one of the platform versions; otherwise send consent on the
///   session too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Consent {
    /// The version of the consent text the customer accepted; the type docs
    /// say which versions each route takes.
    pub text_version: String,
    /// When the customer accepted, ISO 8601 UTC, e.g. `2026-10-11T09:30:00Z`.
    /// Not in the future.
    pub accepted_at: String,
    /// The customer's IPv4 or IPv6 address at acceptance.
    pub ip: String,
}

impl Consent {
    /// The customer accepted `text_version` at `accepted_at` (ISO 8601 UTC)
    /// from `ip`.
    pub fn new(
        text_version: impl Into<String>,
        accepted_at: impl Into<String>,
        ip: impl Into<String>,
    ) -> Consent {
        Consent {
            text_version: text_version.into(),
            accepted_at: accepted_at.into(),
            ip: ip.into(),
        }
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
///
/// // Save a card with no payment: amount 0 and verify_only.
/// let key = IdempotencyKey::new("verify-customer-77")?;
/// let verify = CheckoutSessionRequest::new(0, "EUR", "customer-77", key).verify_only(true);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutSessionRequest {
    /// The amount in MINOR units: `2500` is 25.00 EUR. Integers only. Above 0,
    /// except on a verify-only session, where it must be 0.
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
    /// `"light"`, `"dark"` or `"bright"`, at most 20 characters. `None` keeps
    /// your dashboard default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// The payment description shown on the checkout page, at most 500
    /// characters.
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
    /// How the payer enters the card: [`Integration::Widget`] (the default when
    /// `None`) or [`Integration::Fields`], card fields in your own page. Card
    /// fields are enabled per merchant on request; asking for them on an account
    /// without them is a 400 `INVALID_SELECTION`. Part of the idempotency
    /// identity: replaying a key with a different integration is
    /// `IDEMPOTENCY_KEY_REUSED`. `None` sends no integration field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration: Option<Integration>,
    /// Saves the payer's card with no payment. The checkout places a small
    /// authorization, saves the card and releases the authorization; nobody is
    /// charged. `amount` must be 0, the card is always saved, the integration
    /// must be the widget, and `agreement_id` is refused. A non-null
    /// [`CheckoutStatus::stored_payment_method`] is what says the card is
    /// verified.
    ///
    /// Off until we switch it on for your account: before that, the session is
    /// refused with [`Error::Forbidden`](crate::Error::Forbidden) and code
    /// `VERIFY_ONLY_NOT_ENABLED`. [`IdempotencyKey::for_order`] needs an amount
    /// above 0, so build the key with [`IdempotencyKey::new`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_only: Option<bool>,
    /// The `agr_` handle of a PENDING agreement from
    /// [`Client::create_agreement`](crate::Client::create_agreement). It makes
    /// this the subscription session that takes the agreement's first payment,
    /// saves the card and activates the agreement. Amount and currency must be
    /// the agreement's own (on a plan with a trial only the currency is held).
    ///
    /// A subscription session the API cannot open is refused with
    /// [`Error::Refusal`](crate::Error::Refusal) and nothing is created, for
    /// example code `AGREEMENT_NOT_PENDING`, `AGREEMENT_SESSION_OPEN` or
    /// `SUBSCRIPTIONS_NOT_ENABLED`. The contract lists every code under
    /// `recurring.subscriptionSessionOutcomes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agreement_id: Option<String>,
    /// Only with `agreement_id`: the customer's acceptance of the platform
    /// consent text. Required unless the agreement already carries consent
    /// from its create call; when sent, it replaces what the agreement held.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consent: Option<Consent>,
    /// The id of the storefront (online location) this payment belongs to.
    /// `None` uses your default storefront, or your only active one. An id
    /// that is not one of your storefronts is a 404; a key bound to one
    /// storefront refuses a different id with `STOREFRONT_MISMATCH`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,

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
            integration: None,
            verify_only: None,
            agreement_id: None,
            consent: None,
            location_id: None,
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

    /// Picks the widget or card fields. See
    /// [`CheckoutSessionRequest::integration`].
    pub fn integration(mut self, value: Integration) -> Self {
        self.integration = Some(value);
        self
    }

    /// Saves the card with no payment; send amount 0. See
    /// [`CheckoutSessionRequest::verify_only`].
    pub fn verify_only(mut self, value: bool) -> Self {
        self.verify_only = Some(value);
        self
    }

    /// Takes the first payment of a pending agreement. See
    /// [`CheckoutSessionRequest::agreement_id`].
    pub fn agreement_id(mut self, value: impl Into<String>) -> Self {
        self.agreement_id = Some(value.into());
        self
    }

    /// Records the customer's consent with the agreement. See
    /// [`CheckoutSessionRequest::consent`].
    pub fn consent(mut self, value: Consent) -> Self {
        self.consent = Some(value);
        self
    }

    /// Attributes the payment to one of your storefronts. See
    /// [`CheckoutSessionRequest::location_id`].
    pub fn location_id(mut self, value: impl Into<String>) -> Self {
        self.location_id = Some(value.into());
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
    /// What the session was created for: `"widget"` or `"fields"` (compare with
    /// [`Integration::as_str`]). Kept as a string so a value this crate does not
    /// know yet still parses. For fields, `cashier_key` and `cashier_token` are
    /// the card fields key and session token.
    #[serde(default)]
    pub integration: String,
    /// Set only for [`Integration::Fields`]: the browser credential the
    /// checkout.js drop-in sends for this session. Opaque, at most 128
    /// characters, the same on every replay. Hand it to the payer's page with
    /// the other session values; never log it or keep it past the session.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// The `agr_` handle of the agreement this session takes the first payment
    /// for. `None` on a one-off checkout.
    #[serde(default)]
    pub agreement_id: Option<String>,
    /// True for a verify-only session: `amount` is the platform's verification
    /// hold, released once the card is saved. False on a payment session.
    #[serde(default, deserialize_with = "false_when_null")]
    pub verify_only: bool,

    /// The unparsed payload, for fields this struct does not model yet.
    #[serde(skip)]
    pub raw: Value,
}

/// How the payer enters the card on a checkout session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Integration {
    /// The hosted cashier widget. The default.
    Widget,
    /// Card fields rendered in your own page by the checkout.js drop-in.
    /// Enabled per merchant on request.
    Fields,
}

impl Integration {
    /// The whole vocabulary, in the order the canonical contract lists it.
    pub const ALL: [Integration; 2] = [Integration::Widget, Integration::Fields];

    /// The wire value: `"widget"` or `"fields"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Integration::Widget => "widget",
            Integration::Fields => "fields",
        }
    }
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

/// Payment method category wire values carried by `paymentMethod` in
/// [`Client::get_status`](crate::Client::get_status) responses and `payment.*`
/// webhook events.
///
/// Reporting data, not a money-flow switch: a wallet payment refunds, captures
/// and disputes exactly like a plain card payment.
pub mod payment_method {
    /// A plain card payment.
    pub const CARD: &str = "card";
    /// A wallet payment; `walletType` says which wallet.
    pub const WALLET: &str = "wallet";
    /// A bank transfer.
    pub const BANK_TRANSFER: &str = "bank_transfer";
    /// A SEPA payment.
    pub const SEPA: &str = "sepa";

    /// Every payment method category the merchant API reports, in the gateway's
    /// own order.
    pub const ALL: [&str; 4] = [CARD, WALLET, BANK_TRANSFER, SEPA];
}

/// Wallet wire values carried by `walletType` when `paymentMethod` is `"wallet"`.
///
/// The field can carry a lower-cased identifier not in this list yet - treat
/// unknown values as a valid wallet, not an error.
pub mod wallet_type {
    /// Apple Pay.
    pub const APPLE_PAY: &str = "apple_pay";
    /// Google Pay.
    pub const GOOGLE_PAY: &str = "google_pay";
    /// Samsung Pay.
    pub const SAMSUNG_PAY: &str = "samsung_pay";

    /// The wallets the gateway currently names in `walletType`, in the order the
    /// published contract lists them; `tests/wire_contract.rs` pins the list.
    /// Values outside it are wallets the gateway learned about after this crate
    /// released, still valid.
    pub const ALL: [&str; 3] = [APPLE_PAY, GOOGLE_PAY, SAMSUNG_PAY];
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
    /// The payment processor's reference for the transaction. `None` until it
    /// is known, and on a sale settled by reconciliation without a processor
    /// webhook. Older gateways omit it.
    #[serde(default)]
    pub psp_reference: Option<String>,
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
    /// How the payer paid, one of the [`payment_method`] constants. `None` while
    /// the payment is still open (no method chosen yet) and on older
    /// transactions. Reporting data: do not branch payment handling on it.
    #[serde(default)]
    pub payment_method: Option<String>,
    /// Which wallet, when `payment_method` is `"wallet"`, usually one of the
    /// [`wallet_type`] constants. A value outside that list is a valid wallet
    /// the gateway learned about after this crate released, not an error.
    /// `None` for non-wallet payments.
    #[serde(default)]
    pub wallet_type: Option<String>,
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
    /// Not to be confused with [`CheckoutStatus::payment_method`], the string
    /// category of how the payer paid (`card`, `wallet`, ...).
    #[serde(default)]
    pub stored_payment_method: Option<StoredPaymentMethod>,
    /// What to do with a `failed` payment, one of the [`decline_class`]
    /// constants, or a class this SDK does not know yet. It can be `None`, for
    /// example when nobody declined the payment. Never read `None` as `hard`;
    /// if you retry, retry as you would for `soft_other`. It is `None` on every
    /// status other than `failed`.
    #[serde(default)]
    pub decline_class: Option<String>,
    /// True when this is a merchant-initiated charge on a stored card (a
    /// recurring charge), false for a hosted-checkout session.
    #[serde(default, deserialize_with = "false_when_null")]
    pub is_recurring: bool,
    /// True for a verify-only session: a card check, not a payment. `status`
    /// stays the raw status (`requires_capture`, then `cancelled` once the hold
    /// is released); a non-null `stored_payment_method` is what says the card
    /// is verified. False on every other session.
    #[serde(default, deserialize_with = "false_when_null")]
    pub verify_only: bool,
    /// The agreement this session took the first payment for, or the
    /// agreement a recurring charge was placed under. `None` on a one-off
    /// checkout or charge. Still `pending` after a declined or abandoned
    /// subscription session: that is how you learn the subscription did not
    /// start.
    #[serde(default)]
    pub agreement: Option<AgreementSummary>,
    /// The `data.sequence` of the latest `charge.*` webhook about this
    /// transaction, so you can order this read against those events. 0 on a
    /// checkout payment and before the first `charge.*` event. `None` when the
    /// server does not send it.
    #[serde(default)]
    pub sequence: Option<i64>,

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

/// What [`Client::get_integration_contract`](crate::Client::get_integration_contract)
/// returns: the machine-readable contract the API serves, with the routes,
/// fields, vocabularies and error codes of this API version.
///
/// The fields below are the ones code usually checks. Every other section
/// (`errorCodes`, `recurring`, `transactions`, ...) is in `raw`, as sent.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationContract {
    /// The contract version, a date such as `2026-09-16`.
    #[serde(default)]
    pub version: String,
    /// The payment status vocabulary: every value [`CheckoutStatus::status`]
    /// can take.
    #[serde(default)]
    pub statuses: Vec<String>,
    /// The `payment.*` event types, e.g. `payment.succeeded`. The
    /// `agreement.*`, `charge.*` and `verification.*` types are not in this
    /// list: the contract lists them under `recurring.webhookEvents` and
    /// `verification.webhookEvents`, which `raw` keeps. The
    /// [`event_type`] constants name all of them.
    #[serde(default)]
    pub webhook_event_catalog: Vec<String>,

    /// The whole contract as sent.
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
/// is these fields in this order, then the `extra` fields - it is what gets
/// signed.
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
    /// Free text stored on the transaction, at most 500 characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The `agr_` handle of the agreement this charge is made under. Without
    /// it, the charge is yours to back with the customer's agreement, and only
    /// the per-charge cap and the volume limits apply. With it, the agreement
    /// must be yours, linked to this payment method and carrying consent, and
    /// the amount must fit the agreement; the charge records it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agreement_id: Option<String>,

    /// Required. It travels in the header and in the signature, never in the
    /// body. Retrying with the same key never charges the card twice, so on a
    /// timeout retry with the same key. Derive it from the billing period,
    /// never mint one per attempt.
    #[serde(skip)]
    pub idempotency_key: IdempotencyKey,

    /// Any additional field the API accepts that this struct does not model yet.
    /// These are merged into the JSON body, after the fields above.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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
            agreement_id: None,
            idempotency_key,
            extra: Map::new(),
        }
    }

    /// Sets the description.
    pub fn description(mut self, value: impl Into<String>) -> Self {
        self.description = Some(value.into());
        self
    }

    /// Makes the charge under this agreement. See
    /// [`ChargeRequest::agreement_id`].
    pub fn agreement_id(mut self, value: impl Into<String>) -> Self {
        self.agreement_id = Some(value.into());
        self
    }

    /// Adds a body field this struct does not model yet.
    pub fn extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

/// Charge status wire values, on [`PaymentMethodCharge::status`].
pub mod charge_status {
    /// The card was charged.
    pub const SUCCEEDED: &str = "succeeded";
    /// The issuer declined (HTTP 402); see
    /// [`PaymentMethodCharge::decline_class`](crate::PaymentMethodCharge::decline_class),
    /// which can be `None`.
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

/// Decline class wire values, on [`PaymentMethodCharge::decline_class`](crate::PaymentMethodCharge::decline_class)
/// and [`CheckoutStatus::decline_class`]. Coarse enough to act on without
/// reading the issuer's code. A value outside this list is a class added after
/// this release.
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
/// `status` `failed`) alike. A decline is a result, not an error: `decline_class`,
/// when set, says what to do next. Also carried on [`Error::Charge`](crate::Error::Charge)
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
    /// One of the [`decline_class`] constants, or a class this SDK does not
    /// know yet. It can be `None`, for example when nobody declined the
    /// payment. [`Client::get_status`](crate::Client::get_status) carries the
    /// latest class. Never read `None` as `hard`. It is `None` on every status
    /// other than `failed`.
    #[serde(default)]
    pub decline_class: Option<String>,
    /// The issuer's decline code, verbatim, for your logs; branch on
    /// `decline_class` instead. Set on a failed charge when the issuer sent one.
    #[serde(default)]
    pub decline_code: Option<String>,
    /// The transaction the charge created; readable with
    /// [`Client::get_status`](crate::Client::get_status).
    #[serde(default)]
    pub transaction_id: String,
    /// The `agr_` handle of the agreement the charge was made under, when the
    /// request named one.
    #[serde(default)]
    pub agreement_id: Option<String>,
    /// The charge's current sequence, the counter that orders the `charge.*`
    /// webhooks ([`WebhookEvent::sequence`](crate::WebhookEvent::sequence)).
    /// 0 while pending. A replay returns the first answer as it was, so it can
    /// be lower than a later webhook's; the status read carries the live
    /// value. `None` when the server does not send it.
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
/// null. The body is these fields in this order, then the `extra` fields - it
/// is what gets signed.
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

    /// Any additional field the API accepts that this struct does not model yet.
    /// These are merged into the JSON body, after the fields above.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl RefundRequest {
    /// A refund of everything still refundable on the payment. Set
    /// [`RefundRequest::amount`] for a partial one.
    pub fn new(idempotency_key: IdempotencyKey) -> Self {
        RefundRequest {
            amount: None,
            reason: None,
            idempotency_key,
            extra: Map::new(),
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

    /// Adds a body field this struct does not model yet.
    pub fn extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
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
    /// MINOR units. On `pending`, the amount requested (`None` for a refund of
    /// everything still refundable); on `processing`, the amount being refunded
    /// (`None` until such a refund has been sized); on `succeeded`, the amount
    /// actually refunded; always `None` on `failed`.
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

/// What [`Client::list_refunds`](crate::Client::list_refunds) returns: the
/// refunds of one payment and its refund totals.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundList {
    /// The payment whose refunds these are.
    #[serde(default)]
    pub transaction_id: String,
    /// ISO 4217 code of the payment and of every amount here.
    #[serde(default)]
    pub currency: String,
    /// MINOR units refunded so far, from every source: the API, the dashboard
    /// and the payment processor.
    #[serde(default)]
    pub refunded_amount: i64,
    /// MINOR units a new refund can take now: what is not refunded yet, minus
    /// the refunds still in progress. 0 when the payment cannot be refunded.
    #[serde(default)]
    pub refundable_amount: i64,
    /// The refunds you created through the API, newest first, each in the
    /// shape [`Client::get_refund`](crate::Client::get_refund) answers. A
    /// refund made in the dashboard is not in this list, but the two totals
    /// count it.
    #[serde(default)]
    pub refunds: Vec<Refund>,

    /// The unwrapped list as the gateway sent it, for fields this struct does
    /// not model yet.
    #[serde(skip)]
    pub raw: Value,
}

/// The parameters for [`Client::create_plan`](crate::Client::create_plan): a
/// price and a billing interval you sell.
///
/// `name`, `amount`, `currency`, `interval_unit` and the idempotency key are
/// required and come from [`PlanRequest::new`]; the rest are builder methods.
/// The body is these fields in this order, then the `extra` fields. The
/// amount, the currency and the interval never change once the plan exists:
/// to change a price, create a new plan and archive the old one with
/// [`Client::delete_plan`](crate::Client::delete_plan).
///
/// ```
/// use dominaite::{interval_unit, IdempotencyKey, PlanRequest};
///
/// # fn main() -> Result<(), dominaite::Error> {
/// let key = IdempotencyKey::new("plan-pro-monthly")?;
/// // 1990 = 19.90 EUR every month, after a 14-day trial.
/// let request = PlanRequest::new("Pro", 1990, "EUR", interval_unit::MONTH, key)
///     .trial_days(14)
///     .cancel_url("https://shop.example/account/cancel");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRequest {
    /// The display name, 1 to 100 characters.
    pub name: String,
    /// The charge per period in MINOR units: `1990` is 19.90 EUR.
    pub amount: i64,
    /// ISO 4217 currency, three upper-case letters, e.g. `"EUR"`.
    pub currency: String,
    /// One of the [`interval_unit`] constants.
    pub interval_unit: String,
    /// How many units make one billing period: every 3 months is
    /// [`interval_unit::MONTH`] with 3. The period is at most one year: 365
    /// days, 52 weeks, 12 months or 1 year. `None` sends nothing, and the API
    /// uses 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_count: Option<u32>,
    /// The number of periods after which an agreement completes on its own,
    /// at least 1. `None` bills until the agreement is cancelled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_count: Option<u32>,
    /// The days between activation and the first charge, 0 to 365. `None` or
    /// 0 is no trial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trial_days: Option<u32>,
    /// Your page where the customer cancels the subscription: an absolute
    /// `https` URL, at most 2048 characters. The platform puts it as the
    /// cancel link on every receipt and decline notice it emails to your
    /// customer. Without it, those emails tell the customer to contact you.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_url: Option<String>,

    /// Required. It travels in the header and in the signature, never in the
    /// body. The same key with the same body answers the plan it created, so
    /// a create whose answer you lost is safe to send again with the same
    /// key. Derive it from your own plan record, never mint one per attempt.
    #[serde(skip)]
    pub idempotency_key: IdempotencyKey,

    /// Any additional field the API accepts that this struct does not model yet.
    /// These are merged into the JSON body, after the fields above.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PlanRequest {
    /// A plan that charges `amount` MINOR units of `currency` every
    /// `interval_unit`. The key is required: derive it from your own plan
    /// record.
    pub fn new(
        name: impl Into<String>,
        amount: i64,
        currency: impl Into<String>,
        interval_unit: impl Into<String>,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        PlanRequest {
            name: name.into(),
            amount,
            currency: currency.into(),
            interval_unit: interval_unit.into(),
            interval_count: None,
            period_count: None,
            trial_days: None,
            cancel_url: None,
            idempotency_key,
            extra: Map::new(),
        }
    }

    /// Sets how many units make one billing period.
    pub fn interval_count(mut self, value: u32) -> Self {
        self.interval_count = Some(value);
        self
    }

    /// Sets the number of periods after which agreements complete.
    pub fn period_count(mut self, value: u32) -> Self {
        self.period_count = Some(value);
        self
    }

    /// Sets the trial in days.
    pub fn trial_days(mut self, value: u32) -> Self {
        self.trial_days = Some(value);
        self
    }

    /// Sets the page where the customer cancels.
    pub fn cancel_url(mut self, value: impl Into<String>) -> Self {
        self.cancel_url = Some(value.into());
        self
    }

    /// Adds a body field this struct does not model yet.
    pub fn extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

/// The parameters for [`Client::update_plan`](crate::Client::update_plan). A
/// field you do not set keeps its value. The amount, the currency and the
/// interval cannot change.
///
/// ```
/// use dominaite::PlanUpdate;
///
/// // A new name, and no trial for the agreements created from now on.
/// let update = PlanUpdate::new().name("Pro 2027").trial_days(0);
/// ```
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanUpdate {
    /// The new display name, 1 to 100 characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The new trial in days, 0 to 365, for the agreements created from now
    /// on. 0 removes the trial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trial_days: Option<u32>,
    /// The new cancel page, an absolute `https` URL, on the emails from now
    /// on. An empty string removes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_url: Option<String>,

    /// Any additional field the API accepts that this struct does not model yet.
    /// These are merged into the JSON body, after the fields above.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PlanUpdate {
    /// An update that changes nothing yet. Set at least one field: the API
    /// refuses an empty update.
    pub fn new() -> Self {
        PlanUpdate::default()
    }

    /// Sets the new display name.
    pub fn name(mut self, value: impl Into<String>) -> Self {
        self.name = Some(value.into());
        self
    }

    /// Sets the new trial in days. 0 removes the trial.
    pub fn trial_days(mut self, value: u32) -> Self {
        self.trial_days = Some(value);
        self
    }

    /// Sets the new cancel page. An empty string removes it.
    pub fn cancel_url(mut self, value: impl Into<String>) -> Self {
        self.cancel_url = Some(value.into());
        self
    }

    /// Adds a body field this struct does not model yet.
    pub fn extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

/// A subscription plan, as [`Client::create_plan`](crate::Client::create_plan),
/// [`Client::get_plan`](crate::Client::get_plan) and the other plan methods
/// return it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// `plan_` followed by 32 hex characters. It is case-sensitive: store it
    /// and send it back exactly as it is.
    pub id: String,
    /// The display name.
    #[serde(default)]
    pub name: String,
    /// The charge per period in MINOR units of `currency`.
    #[serde(default)]
    pub amount: i64,
    /// ISO 4217 currency.
    #[serde(default)]
    pub currency: String,
    /// One of the [`interval_unit`] constants, or a unit this SDK does not
    /// know yet.
    #[serde(default)]
    pub interval_unit: String,
    /// How many units make one billing period.
    #[serde(default)]
    pub interval_count: i64,
    /// The number of periods after which agreements complete. `None` bills
    /// until cancelled.
    #[serde(default)]
    pub period_count: Option<i64>,
    /// The trial in days for the agreements created from now on. 0 is no
    /// trial.
    #[serde(default)]
    pub trial_days: i64,
    /// One of the [`plan_status`] constants, or a status this SDK does not
    /// know yet. Compare with [`Plan::is_active`].
    #[serde(default)]
    pub status: String,
    /// The page where the customer cancels. `None` when the plan has none.
    #[serde(default)]
    pub cancel_url: Option<String>,
    /// ISO 8601 UTC creation time.
    #[serde(default)]
    pub created_at: String,
    /// ISO 8601 UTC, when the plan was archived. `None` while it is active.
    #[serde(default)]
    pub archived_at: Option<String>,

    /// The unwrapped plan object as the gateway sent it, for fields this
    /// struct does not model yet.
    #[serde(skip)]
    pub raw: Value,
}

impl Plan {
    /// True only for `active`: the plan accepts new agreements. An unknown
    /// status reads as not active.
    pub fn is_active(&self) -> bool {
        self.status == plan_status::ACTIVE
    }
}

/// Interval unit wire values, on [`PlanRequest::interval_unit`] and
/// [`Plan::interval_unit`].
pub mod interval_unit {
    /// Days. At most 365 per period.
    pub const DAY: &str = "day";
    /// Weeks. At most 52 per period.
    pub const WEEK: &str = "week";
    /// Months. At most 12 per period.
    pub const MONTH: &str = "month";
    /// Years. At most 1 per period.
    pub const YEAR: &str = "year";

    /// The whole vocabulary, in the order the contract lists it.
    pub const ALL: [&str; 4] = [DAY, WEEK, MONTH, YEAR];
}

/// Plan status wire values, on [`Plan::status`]. A value outside this list is
/// a status added after this release; treat it as not active.
pub mod plan_status {
    /// The plan accepts new agreements.
    pub const ACTIVE: &str = "active";
    /// [`Client::delete_plan`](crate::Client::delete_plan) archived it: it
    /// accepts no new agreements, and the agreements it has continue.
    pub const ARCHIVED: &str = "archived";

    /// The whole vocabulary, in the order the contract lists it.
    pub const ALL: [&str; 2] = [ACTIVE, ARCHIVED];
}

/// The parameters for [`Client::create_agreement`](crate::Client::create_agreement):
/// subscribe one of your customers to a plan.
///
/// `plan_id`, `customer_reference` and the idempotency key are required and
/// come from [`AgreementRequest::new`]; the rest are builder methods. The body
/// is these fields in this order, then the `extra` fields. The agreement is
/// created `pending` and nothing is charged: the subscription session
/// ([`CheckoutSessionRequest::agreement_id`]) takes the first payment, saves
/// the card and activates it.
///
/// ```
/// use dominaite::{AgreementRequest, IdempotencyKey};
///
/// # fn main() -> Result<(), dominaite::Error> {
/// let key = IdempotencyKey::new("subscription-8817")?;
/// let request = AgreementRequest::new(
///     "plan_0123456789abcdef0123456789abcdef",
///     "customer-77",
///     key,
/// );
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgreementRequest {
    /// The `plan_` handle of one of your active plans.
    pub plan_id: String,
    /// Your own id for the customer, 1 to 100 characters, the same on every
    /// agreement of this customer.
    pub customer_reference: String,
    /// The `pm_` handle of an active card this customer already saved through
    /// checkout, to bill instead of saving a new card in the subscription
    /// session. The card must come from a payment in the plan's currency.
    /// Requires `consent`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_payment_method_id: Option<String>,
    /// The customer's acceptance, when your own page captured it. Required
    /// with `stored_payment_method_id` and with `max_amount_per_period`.
    /// Without them you can leave it out, and the subscription session
    /// captures it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consent: Option<Consent>,
    /// Only for a variable amount (a seat price plus usage): the most one
    /// charge under the agreement may be, in MINOR units of the plan's
    /// currency, at least the plan's amount. One charge per period is
    /// allowed, so this is also the cap per period. Requires `consent`. Leave
    /// it out for a fixed agreement: every charge is then exactly the plan's
    /// amount.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_amount_per_period: Option<i64>,

    /// Required. It travels in the header and in the signature, never in the
    /// body. The same key with the same body answers the agreement it
    /// created, so a create whose answer you lost is safe to send again with
    /// the same key. Derive it from your own subscription record.
    #[serde(skip)]
    pub idempotency_key: IdempotencyKey,

    /// Any additional field the API accepts that this struct does not model yet.
    /// These are merged into the JSON body, after the fields above.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl AgreementRequest {
    /// An agreement that subscribes `customer_reference` to `plan_id`. The key
    /// is required: derive it from your own subscription record.
    pub fn new(
        plan_id: impl Into<String>,
        customer_reference: impl Into<String>,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        AgreementRequest {
            plan_id: plan_id.into(),
            customer_reference: customer_reference.into(),
            stored_payment_method_id: None,
            consent: None,
            max_amount_per_period: None,
            idempotency_key,
            extra: Map::new(),
        }
    }

    /// Bills this saved card. Requires [`AgreementRequest::consent`].
    pub fn stored_payment_method_id(mut self, value: impl Into<String>) -> Self {
        self.stored_payment_method_id = Some(value.into());
        self
    }

    /// Sets the consent your own page captured.
    pub fn consent(mut self, value: Consent) -> Self {
        self.consent = Some(value);
        self
    }

    /// Makes this a variable agreement with this cap per charge. Requires
    /// [`AgreementRequest::consent`].
    pub fn max_amount_per_period(mut self, value: i64) -> Self {
        self.max_amount_per_period = Some(value);
        self
    }

    /// Adds a body field this struct does not model yet.
    pub fn extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

/// A subscription agreement, as
/// [`Client::create_agreement`](crate::Client::create_agreement) and
/// [`Client::get_agreement`](crate::Client::get_agreement) return it.
///
/// It keeps its own copy of the plan's price and interval from the time it was
/// created, so it does not change when the plan changes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agreement {
    /// `agr_` followed by 32 hex characters. It is case-sensitive: store it
    /// and send it back exactly as it is.
    pub id: String,
    /// The `plan_` handle the agreement was created from.
    #[serde(default)]
    pub plan_id: String,
    /// Your own id for the customer, as sent.
    #[serde(default)]
    pub customer_reference: String,
    /// The `pm_` handle of the card on file. `None` until a card is saved.
    #[serde(default)]
    pub stored_payment_method_id: Option<String>,
    /// One of the [`agreement_status`] constants, or a status this SDK does
    /// not know yet. Compare with [`Agreement::is_active`].
    #[serde(default)]
    pub status: String,
    /// The charge per period in MINOR units of `currency`.
    #[serde(default)]
    pub amount: i64,
    /// On a variable agreement, the most one charge may be, in MINOR units.
    /// `None` on a fixed agreement: every charge is exactly `amount`.
    #[serde(default)]
    pub max_amount_per_period: Option<i64>,
    /// ISO 4217 currency. One currency per agreement.
    #[serde(default)]
    pub currency: String,
    /// One of the [`interval_unit`] constants, or a unit this SDK does not
    /// know yet.
    #[serde(default)]
    pub interval_unit: String,
    /// How many units make one billing period.
    #[serde(default)]
    pub interval_count: i64,
    /// The number of periods. `None` bills until cancelled.
    #[serde(default)]
    pub period_count: Option<i64>,
    /// The trial in days.
    #[serde(default)]
    pub trial_days: i64,
    /// ISO 8601 UTC of the next scheduled charge. `None` while pending and
    /// once the agreement has ended.
    #[serde(default)]
    pub next_charge_at: Option<String>,
    /// Why the agreement is `past_due`: one of the [`past_due_reason`]
    /// constants, or a reason this SDK does not know yet. `None` in every
    /// other status.
    #[serde(default)]
    pub past_due_reason: Option<String>,
    /// ISO 8601 UTC, when the agreement went past due. `None` in every other
    /// status.
    #[serde(default)]
    pub past_due_at: Option<String>,
    /// The recorded consent and the platform's receipt of it. `None` until
    /// consent is captured.
    #[serde(default)]
    pub consent: Option<ConsentRecord>,
    /// Starts at 1. It goes up when the price changes for a future period.
    #[serde(default)]
    pub version: i64,
    /// The agreement's transition counter as of this read: the
    /// `data.sequence` of the latest `agreement.*` webhook about it, 0 before
    /// its first transition. Compare it with the highest sequence you
    /// processed, as you do for an event
    /// ([`WebhookEvent::sequence`](crate::WebhookEvent::sequence)).
    #[serde(default)]
    pub sequence: i64,
    /// ISO 8601 UTC creation time.
    #[serde(default)]
    pub created_at: String,
    /// ISO 8601 UTC, when the agreement went active. `None` while pending.
    #[serde(default)]
    pub activated_at: Option<String>,
    /// ISO 8601 UTC, when you cancelled it. `None` unless cancelled.
    #[serde(default)]
    pub cancelled_at: Option<String>,

    /// The unwrapped agreement object as the gateway sent it, for fields this
    /// struct does not model yet.
    #[serde(skip)]
    pub raw: Value,
}

impl Agreement {
    /// True only for `active`: the customer paid the first period. With
    /// scheduled charges on, the platform charges each period. An unknown
    /// status reads as not active.
    pub fn is_active(&self) -> bool {
        self.status == agreement_status::ACTIVE
    }
}

/// The consent recorded on an [`Agreement`]: the three fields exactly as they
/// were sent (on the agreement or on the subscription session), plus the
/// platform's own receipt, which no request can set.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentRecord {
    /// The consent text version, as sent.
    #[serde(default)]
    pub text_version: String,
    /// When the customer accepted, ISO 8601 UTC, as sent.
    #[serde(default)]
    pub accepted_at: Option<String>,
    /// The customer's IP address at acceptance, as sent.
    #[serde(default)]
    pub ip: String,
    /// When the consent reached the platform, ISO 8601 UTC, by the server
    /// clock.
    #[serde(default)]
    pub received_at: Option<String>,
    /// The address of the server that sent the consent to the platform, not
    /// the customer's. `None` unless the platform's own network edge recorded
    /// it.
    #[serde(default)]
    pub received_from_ip: Option<String>,
}

/// The agreement on a status read: enough to know whether the subscription
/// started. Read the whole agreement with
/// [`Client::get_agreement`](crate::Client::get_agreement).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgreementSummary {
    /// The `agr_` handle.
    #[serde(default)]
    pub id: String,
    /// One of the [`agreement_status`] constants, or a status this SDK does
    /// not know yet.
    #[serde(default)]
    pub status: String,
    /// ISO 8601 UTC of the next scheduled charge. `None` while pending and once
    /// the agreement has ended.
    #[serde(default)]
    pub next_charge_at: Option<String>,
}

/// Agreement status wire values, on [`Agreement::status`] and
/// [`AgreementSummary::status`]. A value outside this list is a status added
/// after this release; treat it as not active.
pub mod agreement_status {
    /// Created, nothing charged yet. The subscription session activates it.
    pub const PENDING: &str = "pending";
    /// The customer paid the first period and the card is on file. With
    /// scheduled charges on, the platform charges each period.
    pub const ACTIVE: &str = "active";
    /// The platform gave up on the current period on this card, and makes no
    /// further attempt on it; `past_due_reason` says why. It stays past due
    /// until the period is paid on another card, which makes it active again,
    /// or until you cancel it. A soft decline that is still retried leaves
    /// the agreement active.
    pub const PAST_DUE: &str = "past_due";
    /// You cancelled it. No further charge, ever. The platform never cancels
    /// an agreement on its own.
    pub const CANCELLED: &str = "cancelled";
    /// Every period of the plan was charged.
    pub const COMPLETED: &str = "completed";
    /// A terminal failure. Reserved: the platform does not set it today. An
    /// agreement whose period cannot be collected is `past_due` instead.
    pub const FAILED: &str = "failed";

    /// The whole vocabulary, in the order the contract lists it.
    pub const ALL: [&str; 6] = [PENDING, ACTIVE, PAST_DUE, CANCELLED, COMPLETED, FAILED];
}

/// Why an agreement is past due, on [`Agreement::past_due_reason`]. A value
/// outside this list is a reason added after this release.
pub mod past_due_reason {
    /// The card's expiry month is over. Ask the customer for a new card.
    pub const CARD_EXPIRED: &str = "card_expired";
    /// The card on file is no longer active, or the agreement has no card.
    /// Ask the customer for a new card.
    pub const CARD_REVOKED: &str = "card_revoked";
    /// The issuer declined, and a retry on the same card does not help.
    pub const HARD_DECLINE: &str = "hard_decline";
    /// The soft declines used up all the retries.
    pub const DUNNING_EXHAUSTED: &str = "dunning_exhausted";
    /// The issuer wants strong customer authentication. The card is good, but
    /// the next payment needs the customer present for a challenge.
    pub const SCA_REQUIRED: &str = "sca_required";
    /// The charge was refused for a reason that no retry fixes.
    pub const UNPROCESSABLE: &str = "unprocessable";

    /// The whole vocabulary, in the gateway's order.
    pub const ALL: [&str; 6] = [
        CARD_EXPIRED,
        CARD_REVOKED,
        HARD_DECLINE,
        DUNNING_EXHAUSTED,
        SCA_REQUIRED,
        UNPROCESSABLE,
    ];
}

/// What a webhook endpoint's `Debug` output prints instead of its signing
/// secret.
const REDACTED_WEBHOOK_SECRET: &str = "whsec_***redacted***";

/// The parameters for
/// [`Client::create_webhook_endpoint`](crate::Client::create_webhook_endpoint).
///
/// `url`, `events` and the idempotency key are required and come from
/// [`WebhookEndpointRequest::new`]. The body is these fields in this order,
/// then the `extra` fields.
///
/// ```
/// use dominaite::{event_type, IdempotencyKey, WebhookEndpointRequest};
///
/// # fn main() -> Result<(), dominaite::Error> {
/// let key = IdempotencyKey::new("webhook-orders-2026-10")?;
/// let request = WebhookEndpointRequest::new(
///     "https://shop.example/webhooks/dominaite",
///     [event_type::PAYMENT_SUCCEEDED, event_type::PAYMENT_REFUNDED],
///     key,
/// );
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookEndpointRequest {
    /// The URL that receives the deliveries: HTTPS on a public host, at most
    /// 500 characters.
    pub url: String,
    /// The event types to deliver, 1 to 50 entries, from the [`event_type`]
    /// constants. Duplicates count once. Matching is exact.
    pub events: Vec<String>,

    /// Required. It travels in the header and in the signature, never in the
    /// body. After a timeout or a 503, send the request again WITH THE SAME
    /// KEY: the retry replaces an endpoint that the lost attempt registered. A
    /// new key cannot do that. Use a fresh key for each new endpoint.
    #[serde(skip)]
    pub idempotency_key: IdempotencyKey,

    /// Any additional field the API accepts that this struct does not model yet.
    /// These are merged into the JSON body, after the fields above.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl WebhookEndpointRequest {
    /// An endpoint at `url` for the `events` you name. The key is required.
    pub fn new<I, S>(url: impl Into<String>, events: I, idempotency_key: IdempotencyKey) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        WebhookEndpointRequest {
            url: url.into(),
            events: events.into_iter().map(Into::into).collect(),
            idempotency_key,
            extra: Map::new(),
        }
    }

    /// Adds a body field this struct does not model yet.
    pub fn extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

/// A webhook endpoint, as
/// [`Client::create_webhook_endpoint`](crate::Client::create_webhook_endpoint)
/// and [`Client::list_webhook_endpoints`](crate::Client::list_webhook_endpoints)
/// return it.
///
/// Its `Debug` output never shows the signing secret, in `secret` or in
/// `raw`.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookEndpoint {
    /// The endpoint id, a UUID.
    /// [`Client::delete_webhook_endpoint`](crate::Client::delete_webhook_endpoint)
    /// takes it.
    pub id: String,
    /// The URL as registered.
    #[serde(default)]
    pub url: String,
    /// The event types delivered to this endpoint.
    #[serde(default)]
    pub events: Vec<String>,
    /// One of the [`webhook_endpoint_status`] constants, or a status this SDK
    /// does not know yet.
    #[serde(default)]
    pub status: String,
    /// ISO 8601 UTC, when the endpoint was registered.
    #[serde(default)]
    pub created_at: String,
    /// The signing secret, `whsec_` followed by 64 hex characters. Only the
    /// first answer of a create carries it: a replay and the list leave it
    /// out, and no route shows it again. Store it in your secret store at
    /// once, and verify deliveries with it through
    /// [`verify_webhook`](crate::verify_webhook). Lost it? Delete the endpoint
    /// and create it again. Never log it.
    #[serde(default)]
    pub secret: Option<String>,

    /// The unwrapped endpoint object as the gateway sent it, for fields this
    /// struct does not model yet. It holds the secret too, when there is one.
    #[serde(skip)]
    pub raw: Value,
}

impl fmt::Debug for WebhookEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut raw = self.raw.clone();
        if let Some(secret) = raw.get_mut("secret").filter(|secret| !secret.is_null()) {
            *secret = Value::from(REDACTED_WEBHOOK_SECRET);
        }
        f.debug_struct("WebhookEndpoint")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("events", &self.events)
            .field("status", &self.status)
            .field("created_at", &self.created_at)
            .field(
                "secret",
                &self.secret.as_ref().map(|_| REDACTED_WEBHOOK_SECRET),
            )
            .field("raw", &raw)
            .finish()
    }
}

/// Webhook endpoint status wire values, on [`WebhookEndpoint::status`].
pub mod webhook_endpoint_status {
    /// Deliveries are sent.
    pub const ACTIVE: &str = "active";
    /// Deliveries are paused: the endpoint was switched off in the dashboard,
    /// or after repeated failed deliveries. It does not count toward the limit
    /// of 25 active endpoints.
    pub const INACTIVE: &str = "inactive";

    /// The whole vocabulary, in the order the contract lists it.
    pub const ALL: [&str; 2] = [ACTIVE, INACTIVE];
}

/// The webhook event types, the `type` of a
/// [`WebhookEvent`](crate::WebhookEvent) and the values of
/// [`WebhookEndpointRequest::events`]. Three groups, as the API lists them. A
/// type outside these lists is one added after this release; ignore an event
/// you do not know.
pub mod event_type {
    /// The payment succeeded. Only this event means that the money is in
    /// hand.
    pub const PAYMENT_SUCCEEDED: &str = "payment.succeeded";
    /// The payment failed.
    pub const PAYMENT_FAILED: &str = "payment.failed";
    /// The payment is authorized and waits for a capture, for example an
    /// approved pre-authorization hold.
    pub const PAYMENT_REQUIRES_CAPTURE: &str = "payment.requires_capture";
    /// The payment was voided before it completed.
    pub const PAYMENT_CANCELLED: &str = "payment.cancelled";
    /// The checkout was never paid, and the platform closed it.
    pub const PAYMENT_ABANDONED: &str = "payment.abandoned";
    /// A refund completed. One event for each refund, partial or full.
    pub const PAYMENT_REFUNDED: &str = "payment.refunded";
    /// The payment was disputed (a chargeback).
    pub const PAYMENT_DISPUTED: &str = "payment.disputed";

    /// The subscription session's payment was approved, and the agreement
    /// went active.
    pub const AGREEMENT_ACTIVATED: &str = "agreement.activated";
    /// The platform gave up on the current period on the agreement's card.
    pub const AGREEMENT_PAST_DUE: &str = "agreement.past_due";
    /// You cancelled the agreement.
    pub const AGREEMENT_CANCELLED: &str = "agreement.cancelled";
    /// A charge on a stored card was approved: one you made, or one the
    /// platform placed for an agreement period.
    pub const CHARGE_SUCCEEDED: &str = "charge.succeeded";
    /// A charge on a stored card was declined, and nothing more is tried for
    /// it.
    pub const CHARGE_FAILED: &str = "charge.failed";
    /// An agreement period was declined softly, and the platform tries the
    /// same card again later. The agreement stays active.
    pub const CHARGE_RETRYING: &str = "charge.retrying";

    /// A verify-only session checked the card. Nobody was charged. The saved
    /// card is in `data.storedPaymentMethod`; when that is null, read the
    /// status.
    pub const VERIFICATION_SUCCEEDED: &str = "verification.succeeded";
    /// A verify-only session ended with no card saved: the issuer declined, or
    /// the payer abandoned the checkout.
    pub const VERIFICATION_FAILED: &str = "verification.failed";

    /// The `payment.*` types, in the order of the contract's
    /// `webhookEventCatalog`.
    pub const PAYMENT: [&str; 7] = [
        PAYMENT_SUCCEEDED,
        PAYMENT_FAILED,
        PAYMENT_REQUIRES_CAPTURE,
        PAYMENT_CANCELLED,
        PAYMENT_ABANDONED,
        PAYMENT_REFUNDED,
        PAYMENT_DISPUTED,
    ];

    /// The `agreement.*` and `charge.*` types, in the order of the contract's
    /// `recurring.webhookEvents`.
    pub const RECURRING: [&str; 6] = [
        AGREEMENT_ACTIVATED,
        AGREEMENT_PAST_DUE,
        AGREEMENT_CANCELLED,
        CHARGE_SUCCEEDED,
        CHARGE_FAILED,
        CHARGE_RETRYING,
    ];

    /// The `verification.*` types of a verify-only session, in the order of
    /// the contract's `verification.webhookEvents`.
    pub const VERIFICATION: [&str; 2] = [VERIFICATION_SUCCEEDED, VERIFICATION_FAILED];
}

/// The parameters for [`Client::list_transactions`](crate::Client::list_transactions):
/// one page of your payments created in a window.
///
/// `from` and `to` are required and come from [`TransactionListParams::new`];
/// the rest are builder methods. A parameter you do not set is left out of
/// the query string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionListParams {
    /// The window start, inclusive, on the payment's creation time: ISO 8601
    /// UTC, e.g. `2026-10-01T00:00:00Z`. A date alone is midnight UTC.
    pub from: String,
    /// The window end, exclusive: ISO 8601 UTC, later than `from` and at most
    /// 31 days after it. For a statement, end the window a few minutes in the
    /// past, so payments still being written are not missed.
    pub to: String,
    /// The settlement window start, inclusive: a date, e.g. `2026-10-13`.
    /// Send it with `settled_to`. With it, the list has only the payments
    /// the acquirer settled in the window. `from` and `to` still apply.
    pub settled_from: Option<String>,
    /// The settlement window end, exclusive: a date, later than
    /// `settled_from` and at most 31 days after it.
    pub settled_to: Option<String>,
    /// Only payments that are in this status now: one of the [`status`]
    /// constants.
    pub status: Option<String>,
    /// Items per page, 1 to 100. The API's default is 50.
    pub limit: Option<u32>,
    /// The [`TransactionPage::next_cursor`] of the previous page, unchanged.
    /// Leave it out for the first page.
    pub cursor: Option<String>,
}

impl TransactionListParams {
    /// The first page of the window from `from` (inclusive) to `to`
    /// (exclusive).
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        TransactionListParams {
            from: from.into(),
            to: to.into(),
            settled_from: None,
            settled_to: None,
            status: None,
            limit: None,
            cursor: None,
        }
    }

    /// Lists only the payments the acquirer settled from `from` (inclusive)
    /// to `to` (exclusive). Both are dates, e.g. `2026-10-13`. For one
    /// settlement day, send that day and the next day.
    pub fn settled(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.settled_from = Some(from.into());
        self.settled_to = Some(to.into());
        self
    }

    /// Lists only the payments in this status.
    pub fn status(mut self, value: impl Into<String>) -> Self {
        self.status = Some(value.into());
        self
    }

    /// Sets the page size, 1 to 100.
    pub fn limit(mut self, value: u32) -> Self {
        self.limit = Some(value);
        self
    }

    /// Asks for the page after the one that answered this cursor.
    pub fn cursor(mut self, value: impl Into<String>) -> Self {
        self.cursor = Some(value.into());
        self
    }
}

/// One page of [`Client::list_transactions`](crate::Client::list_transactions).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionPage {
    /// The payments of this page, oldest first by `created_at`, then by
    /// `transaction_id`. Empty when the window has none.
    #[serde(default)]
    pub items: Vec<Transaction>,
    /// The cursor of the next page. `None` on the last page of the window.
    #[serde(default)]
    pub next_cursor: Option<String>,

    /// The unwrapped page as the gateway sent it, for fields this struct does
    /// not model yet.
    #[serde(skip)]
    pub raw: Value,
}

/// One payment in a [`TransactionPage`]. The fields and values agree with the
/// status read ([`CheckoutStatus`]) of the same `transaction_id`, with the
/// status at the time of the read.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    /// The payment id, as the session or the charge returned it.
    pub transaction_id: String,
    /// Your own order id from the session create call. `None` when none was
    /// sent.
    #[serde(default)]
    pub order_reference: Option<String>,
    /// How the payment was taken: one of the [`transaction_type`] constants,
    /// or a type this SDK does not know yet.
    #[serde(default, rename = "type")]
    pub transaction_type: String,
    /// One of the [`status`] constants, or a status this SDK does not know
    /// yet. The same value the status read answers.
    #[serde(default)]
    pub status: String,
    /// The payment amount in MINOR units of `currency`.
    #[serde(default)]
    pub amount: i64,
    /// ISO 4217 currency.
    #[serde(default)]
    pub currency: String,
    /// ISO 8601 UTC, when the payment was created. The window and the order
    /// use this time.
    #[serde(default)]
    pub created_at: String,
    /// ISO 8601 UTC, when the payment first succeeded. `None` when it never
    /// succeeded, and on older payments that succeeded before this time was
    /// recorded.
    #[serde(default)]
    pub completed_at: Option<String>,
    /// On a `charge`: the `pm_` handle of the saved card that was charged.
    /// `None` on a checkout payment, and on an old charge that did not record
    /// its card.
    #[serde(default)]
    pub stored_payment_method_id: Option<String>,
    /// The total refunded so far, in MINOR units. `None` when nothing was
    /// refunded.
    #[serde(default)]
    pub refunded_amount: Option<i64>,
    /// The acquirer fee of this payment, in MINOR units of `currency`.
    /// `None` until the acquirer settles the payment. It can change, or go
    /// back to `None`, when the settlement is matched again.
    #[serde(default)]
    pub fee: Option<i64>,
    /// `amount` minus `fee`, in MINOR units of `currency`. `None` when `fee`
    /// is `None`. Deductions on the whole payout are not in it.
    #[serde(default)]
    pub net_amount: Option<i64>,
    /// ISO 8601 UTC, the date the acquirer settled the payment, at midnight
    /// (`2026-10-13T00:00:00Z`). `None` until settled.
    #[serde(default)]
    pub settled_at: Option<String>,
    /// The id of the payout that paid this money to you, the id the
    /// dashboard shows. `None` until the money is paid out.
    #[serde(default)]
    pub payout_id: Option<String>,

    /// This item as the gateway sent it, for fields this struct does not
    /// model yet.
    #[serde(skip)]
    pub raw: Value,
}

/// How a payment in the transaction list was taken, on
/// [`Transaction::transaction_type`]. A value outside this list is a type
/// added after this release.
pub mod transaction_type {
    /// The payer paid on the hosted checkout.
    pub const PAYMENT: &str = "payment";
    /// A saved card was charged: by your server through
    /// [`Client::charge_payment_method`](crate::Client::charge_payment_method),
    /// or by the platform for an agreement period.
    pub const CHARGE: &str = "charge";
    /// A verify-only session saved a card. Not a purchase: the verification
    /// amount is released, or refunded when it was captured. Leave it out of
    /// revenue.
    pub const VERIFICATION: &str = "verification";

    /// The whole vocabulary, in the order the contract lists it.
    pub const ALL: [&str; 3] = [PAYMENT, CHARGE, VERIFICATION];
}
