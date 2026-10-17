//! The client: signing, sending, and mapping responses onto the error taxonomy.

use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::error::{session_error_code, Error, Result};
use crate::signing::{sign_request, SignRequest};
use crate::types::{
    Agreement, AgreementRequest, ChargeRequest, CheckoutSession, CheckoutSessionRequest,
    CheckoutStatus, IntegrationContract, PaymentMethodCharge, Ping, Plan, PlanRequest, PlanUpdate,
    Refund, RefundList, RefundRequest, TransactionListParams, TransactionPage, WebhookEndpoint,
    WebhookEndpointRequest,
};

/// The sandbox merchant API. Use it with sandbox credentials.
pub const SANDBOX_BASE_URL: &str = "https://api.staging.dominaite.com/payments";

/// The production merchant API. Use it with production credentials.
pub const PRODUCTION_BASE_URL: &str = "https://api.dominaite.com/payments";

/// What [`ClientBuilder::build`] says when no base URL was set.
const BASE_URL_REQUIRED: &str = "base_url is required. Use https://api.staging.dominaite.com/payments for the sandbox or https://api.dominaite.com/payments for production.";

/// The canonical path that gets signed. POST creates a session; GET
/// `SESSIONS_PATH/{transaction_id}` reads its status.
pub const SESSIONS_PATH: &str = "/merchant-api/checkout/sessions";

/// The canonical path of stored payment methods. POST
/// `PAYMENT_METHODS_PATH/{payment_method_id}/charges` charges one; DELETE
/// `PAYMENT_METHODS_PATH/{payment_method_id}` revokes it.
pub const PAYMENT_METHODS_PATH: &str = "/merchant-api/payment-methods";

/// The canonical path of payments. POST
/// `PAYMENTS_PATH/{transaction_id}/refunds` refunds one, and GET on the same
/// path lists its refunds; GET
/// `PAYMENTS_PATH/{transaction_id}/refunds/{refund_id}` reads a refund back.
pub const PAYMENTS_PATH: &str = "/merchant-api/payments";

/// The credentials-and-clock smoke test. Creates nothing.
pub const PING_PATH: &str = "/merchant-api/ping";

/// The machine-readable integration contract. GET only.
pub const INTEGRATION_CONTRACT_PATH: &str = "/merchant-api/integration/contract";

/// The canonical path of subscription plans. POST creates one and GET lists
/// them; GET, PATCH and DELETE `PLANS_PATH/{plan_id}` read, update and archive
/// one.
pub const PLANS_PATH: &str = "/merchant-api/plans";

/// The canonical path of subscription agreements. POST creates one; GET and
/// DELETE `AGREEMENTS_PATH/{agreement_id}` read and cancel one.
pub const AGREEMENTS_PATH: &str = "/merchant-api/agreements";

/// The canonical path of webhook endpoints. POST registers one and GET lists
/// them; DELETE `WEBHOOKS_PATH/{id}` removes one.
pub const WEBHOOKS_PATH: &str = "/merchant-api/webhooks";

/// The canonical path of the transaction list. The signature covers this
/// path only, never the query string.
pub const TRANSACTIONS_PATH: &str = "/merchant-api/transactions";

/// This SDK's version, reported in the User-Agent.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The compiler version, from `build.rs`, reported in the User-Agent.
const RUSTC_VERSION: &str = env!("DOMINAITE_RUSTC_VERSION");

/// Serverless cold starts hit 10+ seconds outside prod, so 45s is the floor that
/// does not turn a cold start into a false transport failure.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(45);

/// How [`Client::create_checkout_session_with_retry`] backs off.
#[derive(Debug, Clone, Copy)]
pub struct RetryOptions {
    /// Total attempts including the first. Defaults to 3.
    pub attempts: u32,
    /// The wait before the first retry; it doubles each attempt. Defaults to 500ms.
    pub base_delay: Duration,
}

impl Default for RetryOptions {
    fn default() -> Self {
        RetryOptions {
            attempts: 3,
            base_delay: Duration::from_millis(500),
        }
    }
}

/// What every `Debug` in this crate prints instead of the API secret.
pub(crate) const REDACTED_SECRET: &str = "dms_***redacted***";

/// What every `Debug` in this crate prints for a hook you set.
const HOOK_SET: &str = "<fn>";

/// A warning or debug sink. Shared, so a clone of the client keeps it.
type Sink = Arc<dyn Fn(&str) + Send + Sync>;

/// The response header that tells an old SDK version to upgrade.
const NOTICE_HEADER: &str = "dominaite-sdk-notice";

/// The response header that names the call for Dominaite support.
const REQUEST_ID_HEADER: &str = "dominaite-request-id";

/// A notice header longer than this is cut before it is read.
const NOTICE_MAX_CHARS: usize = 512;

/// A request id longer than this is cut before it is logged.
const REQUEST_ID_MAX_CHARS: usize = 128;

/// How many distinct notice values one client remembers. A new value after
/// that is not warned about, so a misbehaving server cannot grow the set.
const NOTICE_MEMORY: usize = 32;

/// A ping clock skew above this many seconds, either way, is warned about.
const CLOCK_SKEW_WARNING_SECONDS: u64 = 60;

/// Builds a [`Client`]. Start with [`Client::builder`].
// SECURITY: do not derive Serialize. The secret field would be emitted by any
// serde-based logger, which is how the Go SDK leaked it through json.Marshal.
#[derive(Clone)]
pub struct ClientBuilder {
    key_id: String,
    secret: String,
    base_url: Option<String>,
    timeout: Duration,
    user_agent: Option<String>,
    agent: Option<ureq::Agent>,
    on_warning: Option<Sink>,
    on_debug: Option<Sink>,
}

/// Hand-written so a debug-logged builder cannot leak the secret. A derived one
/// prints it verbatim.
impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("key_id", &self.key_id)
            .field("secret", &REDACTED_SECRET)
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .field("user_agent", &self.user_agent)
            .field("agent", &self.agent)
            .field("on_warning", &self.on_warning.as_ref().map(|_| HOOK_SET))
            .field("on_debug", &self.on_debug.as_ref().map(|_| HOOK_SET))
            .finish()
    }
}

impl ClientBuilder {
    /// Sets the API the client calls: [`SANDBOX_BASE_URL`],
    /// [`PRODUCTION_BASE_URL`] or your own value. Required: there is no default,
    /// so an unset environment variable cannot send sandbox credentials to
    /// production. Whitespace and trailing slashes are trimmed.
    ///
    /// [`ClientBuilder::build`] rejects a missing, empty or whitespace-only value
    /// with [`Error::Validation`]. The URL also has to be `https://`, except
    /// plain `http://` on a loopback host (`localhost`, `127.0.0.1`, `[::1]`) so
    /// a local mock still works.
    pub fn base_url(mut self, base_url: impl AsRef<str>) -> Self {
        let trimmed = base_url.as_ref().trim().trim_end_matches('/');
        self.base_url = Some(trimmed.to_string());
        self
    }

    /// Sets the per-request timeout. Defaults to 45 seconds. Ignored when you
    /// supply your own [`ClientBuilder::agent`]; set the timeout on that agent's
    /// config instead.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Appends your own identifier to the SDK's User-Agent, which helps when
    /// Dominaite support reads the access logs for your integration. The
    /// User-Agent is `dominaite-rust/<version> (rust <rustc version>)`, then a
    /// space and this suffix. Whitespace is trimmed and a blank value is
    /// ignored. [`ClientBuilder::build`] rejects a suffix with a control
    /// character with [`Error::Validation`].
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        let value = user_agent.into();
        if !value.trim().is_empty() {
            self.user_agent = Some(value);
        }
        self
    }

    /// Receives the SDK's warnings, one line each: a `Dominaite-Sdk-Notice`
    /// from the API (your SDK version is deprecated), a retry in
    /// [`Client::create_checkout_session_with_retry`], and a ping clock skew
    /// over 60 seconds.
    ///
    /// Without it, warnings go nowhere: a library must not write to your
    /// stderr. Forward them to your logger, for example
    /// `.on_warning(|message| log::warn!("{message}"))`.
    ///
    /// A line never holds the secret, the signature, a cashier token, a
    /// request or response body, or a query string. A panic in the hook is
    /// caught and dropped, so it cannot fail the call that triggered it.
    pub fn on_warning(mut self, hook: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.on_warning = Some(Arc::new(hook));
        self
    }

    /// Turns the debug log on: one line per HTTP request, after the answer or
    /// the transport failure. Off by default.
    ///
    /// A line reads
    /// `dominaite-rust: POST /merchant-api/checkout/sessions 201 84ms request_id=<id> idempotency_key=<key>`:
    /// the method, the signed path without the query string, the HTTP status
    /// (`error` when nothing came back), the time in milliseconds, the
    /// `Dominaite-Request-Id` the API answered with, and the idempotency key
    /// when the request sent one. Give the request id to Dominaite support to
    /// find the call. The same never-logged list as [`ClientBuilder::on_warning`]
    /// applies, and a panic in the hook is caught and dropped.
    pub fn on_debug(mut self, hook: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.on_debug = Some(Arc::new(hook));
        self
    }

    /// Supplies your own [`ureq::Agent`]: a proxy-aware transport, custom TLS, or
    /// a test double. It replaces [`ClientBuilder::timeout`].
    ///
    /// Configure it with `http_status_as_error(false)` if you want the SDK to
    /// read error bodies; the SDK maps a status-as-error into the same taxonomy
    /// either way, just without the API's message.
    ///
    /// Redirects stay off whatever you pass: every request forces
    /// `max_redirects(0)` on itself, because following one would send your signed
    /// headers to a host you never authenticated. A `max_redirects` setting on
    /// your agent is ignored.
    pub fn agent(mut self, agent: ureq::Agent) -> Self {
        self.agent = Some(agent);
        self
    }

    /// Validates the credentials and the base URL, and builds the client.
    ///
    /// Returns [`Error::Validation`] when either credential has the wrong prefix,
    /// which catches a swapped key id and secret before anything is sent, when
    /// the base URL is missing, empty or whitespace-only, and when the base URL
    /// is not `https://` on a non-loopback host.
    pub fn build(self) -> Result<Client> {
        if !self.key_id.starts_with("dmk_") {
            return Err(Error::validation("key_id must start with dmk_"));
        }
        if !self.secret.starts_with("dms_") {
            return Err(Error::validation("secret must start with dms_"));
        }
        let base_url = match self.base_url {
            Some(base_url) if !base_url.is_empty() => base_url,
            _ => return Err(Error::validation(BASE_URL_REQUIRED)),
        };
        if let Some(problem) = base_url_problem(&base_url) {
            return Err(Error::validation(problem));
        }

        let mut user_agent = format!("dominaite-rust/{VERSION} (rust {RUSTC_VERSION})");
        if let Some(extra) = self.user_agent {
            // Checked before the trim, so a stray "\n" is a bug you hear about,
            // not one that is quietly cut off.
            if extra.chars().any(char::is_control) {
                return Err(Error::validation(
                    "user_agent must not contain control characters",
                ));
            }
            user_agent.push(' ');
            user_agent.push_str(extra.trim());
        }

        let agent = self.agent.unwrap_or_else(|| {
            ureq::Agent::new_with_config(
                ureq::Agent::config_builder()
                    .timeout_global(Some(self.timeout))
                    // Read the body on 4xx/5xx: the machine-readable code lives in it.
                    .http_status_as_error(false)
                    // Never follow a redirect. The signed headers would travel to a
                    // host we never authenticated, and its answer would be read as
                    // the API's. With 0, ureq hands the 3xx back as a response
                    // instead of erroring, and `request` turns it into an error.
                    .max_redirects(0)
                    .build(),
            )
        });

        Ok(Client {
            key_id: self.key_id,
            secret: self.secret,
            base_url,
            user_agent,
            agent,
            hooks: Hooks {
                on_warning: self.on_warning,
                on_debug: self.on_debug,
                seen_notices: Arc::new(Mutex::new(Vec::new())),
            },
        })
    }
}

/// A server-side client for the Dominaite merchant API.
///
/// Cloning is cheap and shares the underlying connection pool, so one client per
/// process is the normal shape. A clone also shares the hooks and the record of
/// which notices were already warned about.
// SECURITY: do not derive Serialize. The secret field would be emitted by any
// serde-based logger, which is how the Go SDK leaked it through json.Marshal.
#[derive(Clone)]
pub struct Client {
    key_id: String,
    secret: String,
    base_url: String,
    user_agent: String,
    agent: ureq::Agent,
    hooks: Hooks,
}

/// The warning and debug sinks of one client, and the notices it already
/// warned about.
#[derive(Clone)]
struct Hooks {
    on_warning: Option<Sink>,
    on_debug: Option<Sink>,
    seen_notices: Arc<Mutex<Vec<String>>>,
}

/// What [`Client::send`] learned about an exchange, for the debug line.
#[derive(Default)]
struct Exchange {
    status: Option<u16>,
    request_id: Option<String>,
}

/// Hand-written so a debug-logged client cannot leak the secret. A derived one
/// prints it verbatim.
impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("key_id", &self.key_id)
            .field("secret", &REDACTED_SECRET)
            .field("base_url", &self.base_url)
            .field("user_agent", &self.user_agent)
            .field("agent", &self.agent)
            .field(
                "on_warning",
                &self.hooks.on_warning.as_ref().map(|_| HOOK_SET),
            )
            .field("on_debug", &self.hooks.on_debug.as_ref().map(|_| HOOK_SET))
            .finish()
    }
}

impl Client {
    /// A client on `base_url` with the default 45s timeout.
    ///
    /// Both credentials come from the dashboard's Website integration tab. Pass
    /// [`SANDBOX_BASE_URL`] or [`PRODUCTION_BASE_URL`] to match them. A missing
    /// or blank base URL fails with [`Error::Validation`].
    pub fn new(
        key_id: impl Into<String>,
        secret: impl Into<String>,
        base_url: impl AsRef<str>,
    ) -> Result<Client> {
        Client::builder(key_id, secret).base_url(base_url).build()
    }

    /// A builder, for a different timeout, a User-Agent suffix, or your own HTTP
    /// agent. Set [`ClientBuilder::base_url`] before [`ClientBuilder::build`].
    pub fn builder(key_id: impl Into<String>, secret: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            key_id: key_id.into(),
            secret: secret.into(),
            base_url: None,
            timeout: DEFAULT_TIMEOUT,
            user_agent: None,
            agent: None,
            on_warning: None,
            on_debug: None,
        }
    }

    /// The base URL this client is pointed at.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The User-Agent this client sends:
    /// `dominaite-rust/<version> (rust <rustc version>)`, then your suffix.
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// Hands one warning to [`ClientBuilder::on_warning`], when set.
    fn warn(&self, message: &str) {
        if let Some(hook) = &self.hooks.on_warning {
            call_hook(hook, message);
        }
    }

    /// Warns about a `Dominaite-Sdk-Notice` value, once per distinct value.
    fn notice(&self, value: &str) {
        if value.is_empty() {
            return;
        }
        {
            // A poisoned lock only means another thread's hook panicked while it
            // was held, which it never is: the set itself is still sound.
            let mut seen = self
                .hooks
                .seen_notices
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if seen.iter().any(|known| known == value) || seen.len() >= NOTICE_MEMORY {
                return;
            }
            seen.push(value.to_string());
        }
        // Outside the lock: the hook may take as long as it likes.
        self.warn(&notice_message(value));
    }

    /// Verifies your credentials, your signing, and your clock without creating
    /// anything. Make this your first live call: a 401 here means the key id, the
    /// secret, or the signing, and a 503 means retry later - never both at once.
    ///
    /// Check [`Ping::clock_skew_seconds`]. Requests start failing at 300. A
    /// skew over 60 seconds either way also goes to
    /// [`ClientBuilder::on_warning`].
    pub fn ping(&self) -> Result<Ping> {
        // GET signs an EMPTY idempotency key and an EMPTY body.
        let ping: Ping = self
            .request_reply("GET", PING_PATH, "", "", "")?
            .object("ping")?;
        if ping.clock_skew_seconds.unsigned_abs() > CLOCK_SKEW_WARNING_SECONDS {
            self.warn(&format!(
                "dominaite-rust: clock skew is {} s; requests fail at 300 s. Sync this server's clock.",
                ping.clock_skew_seconds
            ));
        }
        Ok(ping)
    }

    /// Reads the machine-readable integration contract: the routes, fields,
    /// vocabularies and error codes of this API version. Check
    /// [`IntegrationContract::version`] in a startup or CI check to learn when
    /// the API moved on.
    ///
    /// Every key can read it, whatever its scopes. The API does not check the
    /// credentials on this route at all, so a successful read proves nothing
    /// about them: use [`Client::ping`] for that.
    pub fn get_integration_contract(&self) -> Result<IntegrationContract> {
        self.request_reply("GET", INTEGRATION_CONTRACT_PATH, "", "", "")?
            .object("integration contract")
    }

    /// Opens a hosted checkout session for one payment.
    ///
    /// The errors it returns:
    /// - [`Error::Validation`]: bad arguments; nothing was sent.
    /// - [`Error::Auth`]: wrong credentials, bad signature, clock off, IP not
    ///   allowlisted. Fix the config, do not retry.
    /// - [`Error::Forbidden`]: the key does not hold the `payments` scope, or a
    ///   verify-only session on an account where verify-only is not on.
    /// - [`Error::Refusal`]: the gateway refused the session; inspect `code`.
    /// - [`Error::RateLimited`]: HTTP 429. Wait, then send it again with the
    ///   same idempotency key. Not retried for you.
    /// - [`Error::Api`]: an unexpected or rejecting response; inspect `status`
    ///   and `code`. The storefront codes arrive here, e.g. a 409 with
    ///   [`session_error_code::STOREFRONT_NOT_WHITELISTED`](crate::session_error_code::STOREFRONT_NOT_WHITELISTED).
    /// - [`Error::Transport`]: network failure or 5xx. Safe to retry WITH the same
    ///   idempotency key, which is what [`Client::create_checkout_session_with_retry`]
    ///   does.
    pub fn create_checkout_session(
        &self,
        request: &CheckoutSessionRequest,
    ) -> Result<CheckoutSession> {
        let (idempotency_key, body) = prepare_session_request(request)?;
        let reply = self.request_reply("POST", SESSIONS_PATH, "", &body, &idempotency_key)?;
        let payload = reply.payload();

        // Create is the nested shape: an inner `success` next to `checkout`.
        let succeeded = payload.get("success").and_then(Value::as_bool) == Some(true);
        let checkout = payload.get("checkout").filter(|value| value.is_object());

        match (succeeded, checkout) {
            (true, Some(checkout)) => parse_object(
                checkout.clone(),
                reply.status,
                "checkout",
                reply.request_id(),
            ),
            // A storefront mismatch on a replay comes back as a 200 refusal, but
            // it is the same storefront error as the 400: one class for one code.
            _ if string_field(payload, "errorCode").as_deref()
                == Some(session_error_code::STOREFRONT_MISMATCH) =>
            {
                let message = string_field(payload, "errorMessage")
                    .unwrap_or_else(|| "The API key is bound to another storefront.".to_string());
                let request_id = reply.request_id.clone();
                Err(Error::api_with_code(
                    reply.status,
                    Some(session_error_code::STOREFRONT_MISMATCH.to_string()),
                    message,
                    reply.envelope,
                )
                .with_request_id(request_id.as_deref()))
            }
            // A replay refusal names the transaction the key collided with. Carry
            // it so the caller can reconcile with get_status instead of minting a
            // second payment for the same order.
            _ => Err(Error::refusal(
                string_field(payload, "errorCode").unwrap_or_else(|| "UNKNOWN".to_string()),
                string_field(payload, "errorMessage")
                    .unwrap_or_else(|| "The checkout session was refused.".to_string()),
                string_field(payload, "transactionId"),
            )
            .with_request_id(reply.request_id())),
        }
    }

    /// Creates a session, retrying [`Error::Transport`] and the
    /// `PAYMENT_PROCESSING_UNAVAILABLE` refusal, with THE SAME idempotency key
    /// across every attempt.
    ///
    /// Reusing the key is what makes the retry safe. A transport failure leaves
    /// you not knowing whether the request landed; a key the API has already seen
    /// never opens a second session. Every attempt sends the request's own key,
    /// never a fresh one: a new key per attempt would be exactly the
    /// double-charge bug this method exists to prevent.
    ///
    /// When the first attempt did land, the retry is a replay. A clean replay of
    /// a session that is still open returns the ORIGINAL session, with the same
    /// transaction id and cashier handles, so a response lost to a timeout is
    /// recovered. A replay of a paid or failed attempt, or one with a different
    /// amount, currency or `save_card`, is an [`Error::Refusal`]
    /// (`ALREADY_PROCESSED`, `PRIOR_ATTEMPT_FAILED`, `IDEMPOTENCY_KEY_REUSED`),
    /// and `DUPLICATE_REQUEST` means the open session cannot be handed back just
    /// yet. When the refusal names a transaction id, read it back with
    /// [`Client::get_status`] to find out what the earlier attempt did.
    ///
    /// `PAYMENT_PROCESSING_UNAVAILABLE` is retried in both of its forms: a 503
    /// (already a transport error) and the HTTP 200 refusal. Card payments being
    /// off is temporary, and nothing was created, so the same key is safe.
    /// Every other refusal, and every authentication failure, is returned
    /// immediately. They will not change on a retry.
    pub fn create_checkout_session_with_retry(
        &self,
        request: &CheckoutSessionRequest,
        options: RetryOptions,
    ) -> Result<CheckoutSession> {
        if options.attempts < 1 {
            return Err(Error::validation("attempts must be at least 1"));
        }

        let mut last_error = None;
        for attempt in 0..options.attempts {
            match self.create_checkout_session(request) {
                Ok(session) => return Ok(session),
                Err(error) if error.is_retryable() || is_processing_unavailable(&error) => {
                    if attempt + 1 < options.attempts {
                        self.warn(&format!(
                            "dominaite-rust: retrying create_checkout_session_with_retry after {} (attempt {} of {})",
                            retry_reason(&error),
                            attempt + 2,
                            options.attempts
                        ));
                        std::thread::sleep(options.base_delay * 2u32.pow(attempt.min(16)));
                    }
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }

        Err(last_error.unwrap_or_else(|| Error::transport("Retrying gave up", None)))
    }

    /// Reads the payment status of one of your checkout sessions.
    ///
    /// Decide "paid" with [`CheckoutStatus::is_paid`] - `succeeded` is the only
    /// value that means the customer paid. Poll after the payer returns to you,
    /// or on your order timeout. Not in a tight loop: the platform allows 60
    /// requests per minute per API key and 120 per minute per IP, and going over
    /// returns [`Error::RateLimited`]. An unknown transaction id returns
    /// [`Error::Api`] with status 404.
    ///
    /// [`CheckoutStatus::stored_payment_method`] is the card kept on file when
    /// the session asked for one with [`CheckoutSessionRequest::save_card`] and
    /// the payment was approved; `None` otherwise.
    pub fn get_status(&self, transaction_id: &str) -> Result<CheckoutStatus> {
        let normalized = normalize_transaction_id(transaction_id)?;

        // GET signs an EMPTY idempotency key and an EMPTY body, and sends no
        // Idempotency-Key header.
        let path = format!("{SESSIONS_PATH}/{normalized}");
        self.request_reply("GET", &path, "", "", "")?
            .object("status")
    }

    /// Charges a card kept on file, off-session: no widget, no payer present.
    ///
    /// `payment_method_id` is the `id` from [`CheckoutStatus::stored_payment_method`]
    /// of a session you created with [`CheckoutSessionRequest::save_card`]. The
    /// charge is signed like a session and carries the request's required
    /// `Idempotency-Key`, so retrying after a timeout WITH THE SAME KEY never
    /// charges the card twice.
    ///
    /// Returns the charge on HTTP 201 (200 on a durable replay of the same key)
    /// and on HTTP 402 alike. A decline is not an error: the 402 charge has
    /// `status` `failed`, and its `decline_class`, when set, tells you whether to
    /// give up on the card (`hard`), wait (`soft_funds`, `soft_other`) or bring
    /// the customer back for a hosted session (`soft_sca_required`). It can be
    /// `None`, for example when nobody declined the payment, and
    /// [`Client::get_status`] carries the latest class. Never read `None` as
    /// `hard`. `pending` is not terminal - poll [`Client::get_status`] with
    /// `charge.transaction_id`.
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: bad arguments; nothing was sent.
    /// - [`Error::Auth`]: wrong credentials, bad signature, clock off, IP not
    ///   allowlisted.
    /// - [`Error::Forbidden`]: the key does not hold the `payments` scope.
    /// - [`Error::Charge`]: the gateway answered with a code instead of a charge
    ///   (404, 409, 422, 502, 503); branch on `code`. A 404
    ///   `PAYMENT_METHOD_NOT_FOUND` is an id that is not yours.
    ///   `CHARGE_OUTCOME_UNKNOWN` carries the charge row to poll; never retry it
    ///   under a new key.
    /// - [`Error::Api`]: a 400 validation rejection, or an unexpected response.
    /// - [`Error::RateLimited`]: HTTP 429. Wait, then send it again with the
    ///   same idempotency key. The charge route's own limits arrive here too,
    ///   with `code` `CHARGE_VELOCITY_EXCEEDED` or `CHARGE_DECLINE_RATE_EXCEEDED`
    ///   (see [`rate_limit_error_code`](crate::rate_limit_error_code)).
    /// - [`Error::Transport`]: network failure, or a 5xx that is not a coded 502
    ///   or 503. Safe to retry WITH the same idempotency key.
    pub fn charge_payment_method(
        &self,
        payment_method_id: &str,
        request: &ChargeRequest,
    ) -> Result<PaymentMethodCharge> {
        let id = normalize_payment_method_id(payment_method_id)?;
        let (idempotency_key, body) = prepare_charge_request(request)?;
        let path = format!("{PAYMENT_METHODS_PATH}/{id}/charges");
        let reply = self.send("POST", &path, "", &body, &idempotency_key)?;

        // 201 (200 on a durable replay): the charge was placed, whatever its
        // status. 402: the provider declined; the envelope says success=false
        // but the charge is right there, status `failed` (with its decline class
        // when the gateway had one), so it is a result, not an error.
        let charge = reply.charge();
        if let Some(charge) = charge
            .as_ref()
            .filter(|_| reply.success() || reply.status == 402)
        {
            return Ok(charge.clone());
        }

        if reply.status >= 400 {
            if let Some(code) = reply
                .error_code()
                .filter(|_| is_charge_outcome_status(reply.status))
            {
                let message = reply
                    .error_message()
                    .unwrap_or_else(|| "The charge was refused.".to_string());
                let request_id = reply.request_id.clone();
                return Err(
                    Error::charge(reply.status, code, message, charge, reply.envelope)
                        .with_request_id(request_id.as_deref()),
                );
            }
            return Err(reply.rejection());
        }

        Err(Error::api(
            reply.status,
            "The API answered the charge without a charge body",
        )
        .with_request_id(reply.request_id()))
    }

    /// Revokes a card kept on file. The saved credential is deleted at the
    /// payment provider and the method's status becomes `revoked`; a later
    /// [`Client::charge_payment_method`] on it is refused with
    /// `PAYMENT_METHOD_NOT_ACTIVE`. Returns `Ok(())` on HTTP 204, and again on
    /// an already revoked method, so retrying a timed-out revoke is safe. Not a
    /// payment operation: no idempotency key is signed.
    ///
    /// Errors it returns:
    /// - [`Error::Revoke`]: the gateway refused and nothing changed (502
    ///   `UPSTREAM_CONTRACT_ERROR`, 503 `MERCHANT_API_UNAVAILABLE`); branch on
    ///   `code`.
    /// - [`Error::Api`]: 404 for an id that is not yours (code
    ///   `VALIDATION_ERROR`), or an unexpected response.
    /// - [`Error::Transport`]: network failure, or a 5xx that carries no
    ///   gateway code.
    pub fn revoke_payment_method(&self, payment_method_id: &str) -> Result<()> {
        let id = normalize_payment_method_id(payment_method_id)?;

        // DELETE signs an EMPTY idempotency key and an EMPTY body, like GET, and
        // sends no Idempotency-Key header.
        let path = format!("{PAYMENT_METHODS_PATH}/{id}");
        let reply = self.send("DELETE", &path, "", "", "")?;
        if reply.status < 400 {
            return Ok(());
        }

        if let Some(code) = reply
            .error_code()
            .filter(|_| is_revoke_outcome_status(reply.status))
        {
            let message = reply
                .error_message()
                .unwrap_or_else(|| "The revoke was refused.".to_string());
            let request_id = reply.request_id.clone();
            return Err(Error::revoke(reply.status, code, message, reply.envelope)
                .with_request_id(request_id.as_deref()));
        }
        Err(reply.rejection())
    }

    /// Refunds a payment, in full or in part. Answers HTTP 202 with the refund
    /// queued, not done: read it back with [`Client::get_refund`], or wait for
    /// the `payment.refunded` webhook, which fires once the money has moved. A
    /// failed refund sends no webhook.
    ///
    /// `transaction_id` is the id [`Client::create_checkout_session`] or
    /// [`Client::charge_payment_method`] returned. The refund carries the
    /// request's required `Idempotency-Key`, signed like a charge: the same key
    /// answers the same refund and never refunds twice, so after a timeout or a
    /// 500 resend it WITH THE SAME KEY. A replay answers 202 with the refund as
    /// it stands now.
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: bad arguments; nothing was sent.
    /// - [`Error::Auth`]: wrong credentials, bad signature, clock off, IP not
    ///   allowlisted.
    /// - [`Error::Forbidden`]: the key does not hold the `payments` scope.
    /// - [`Error::Api`]: the gateway refused with one of the
    ///   [`refund_error_code`](crate::refund_error_code) constants, e.g. 422
    ///   `REFUND_AMOUNT_EXCEEDED` or 409 `DUPLICATE_REQUEST`; branch on
    ///   [`Error::code`].
    /// - [`Error::RateLimited`]: HTTP 429. Wait, then send it again with the
    ///   same idempotency key.
    /// - [`Error::Transport`]: network failure or 5xx; nothing was queued on a
    ///   500. Safe to retry WITH the same idempotency key.
    pub fn create_refund(&self, transaction_id: &str, request: &RefundRequest) -> Result<Refund> {
        let transaction_id = normalize_transaction_id(transaction_id)?;
        let (idempotency_key, body) = prepare_refund_request(request)?;
        let path = format!("{PAYMENTS_PATH}/{transaction_id}/refunds");
        self.request_reply("POST", &path, "", &body, &idempotency_key)?
            .object("refund")
    }

    /// Reads one refund of a payment: `transaction_id` is the payment,
    /// `refund_id` the [`Refund::refund_id`] that [`Client::create_refund`]
    /// returned.
    ///
    /// Right after a 202 the refund may not be picked up yet, which answers
    /// [`Error::Api`] with status 404 and code
    /// [`REFUND_NOT_FOUND`](crate::refund_error_code::REFUND_NOT_FOUND): read it
    /// again for up to 60 seconds. `PAYMENT_NOT_FOUND` (404) is a payment that
    /// is not yours. Poll at a sane pace; the rate limits of
    /// [`Client::get_status`] apply here too.
    pub fn get_refund(&self, transaction_id: &str, refund_id: &str) -> Result<Refund> {
        let transaction_id = normalize_transaction_id(transaction_id)?;
        let refund_id = normalize_opaque_id(
            refund_id,
            "refund_id must be the refund_id returned by create_refund",
        )?;

        // GET signs an EMPTY idempotency key and an EMPTY body, and sends no
        // Idempotency-Key header.
        let path = format!("{PAYMENTS_PATH}/{transaction_id}/refunds/{refund_id}");
        self.request_reply("GET", &path, "", "", "")?
            .object("refund")
    }

    /// Lists the refunds of one payment that you created through the API,
    /// newest first, with the payment's refund totals:
    /// [`RefundList::refunded_amount`] and [`RefundList::refundable_amount`].
    ///
    /// The totals count every refund of the payment, also the ones made in the
    /// dashboard. Use `refundable_amount` to size the next refund.
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: `transaction_id` is not a payment id; nothing
    ///   was sent.
    /// - [`Error::Auth`] and [`Error::Forbidden`]: as on
    ///   [`Client::create_refund`].
    /// - [`Error::Api`]: 404 with code
    ///   [`PAYMENT_NOT_FOUND`](crate::refund_error_code::PAYMENT_NOT_FOUND), a
    ///   payment that is not yours.
    /// - [`Error::RateLimited`] and [`Error::Transport`]: wait, then read again.
    pub fn list_refunds(&self, transaction_id: &str) -> Result<RefundList> {
        let transaction_id = normalize_transaction_id(transaction_id)?;
        let path = format!("{PAYMENTS_PATH}/{transaction_id}/refunds");
        self.request_reply("GET", &path, "", "", "")?
            .object("refund list")
    }

    /// Creates a subscription plan: a price and a billing interval you sell.
    /// Nothing is charged. An agreement ([`Client::create_agreement`])
    /// subscribes one customer to the plan, and the hosted subscription
    /// session takes the first payment.
    ///
    /// The plan carries the request's required `Idempotency-Key`. The same key
    /// with the same body answers the plan it created (HTTP 200 instead of
    /// 201), also after a [`Client::update_plan`], so after a timeout send it
    /// again WITH THE SAME KEY. The same key with another body is refused with
    /// 422 `IDEMPOTENCY_KEY_REUSED`.
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: a missing name, currency or interval unit, or
    ///   an amount of 0 or less; nothing was sent.
    /// - [`Error::Auth`]: wrong credentials, bad signature, clock off, IP not
    ///   allowlisted.
    /// - [`Error::Forbidden`]: the key does not hold the `payments` scope.
    /// - [`Error::Api`]: HTTP 400 for a field the API refused (the details are
    ///   in `raw`, under `error.validationErrors`) or `INVALID_CURRENCY`, and
    ///   422 `IDEMPOTENCY_KEY_REUSED`.
    /// - [`Error::RateLimited`]: HTTP 429. Wait, then send it again with the
    ///   same idempotency key.
    /// - [`Error::Transport`]: network failure or 5xx. Nothing was created on
    ///   a 503; send it again WITH the same idempotency key.
    pub fn create_plan(&self, request: &PlanRequest) -> Result<Plan> {
        if request.name.trim().is_empty() {
            return Err(Error::validation("Missing required parameter: name"));
        }
        if request.amount <= 0 {
            return Err(Error::validation(
                "amount must be a positive integer in MINOR units (e.g. 1990 for 19.90 EUR)",
            ));
        }
        if request.currency.trim().is_empty() {
            return Err(Error::validation("Missing required parameter: currency"));
        }
        if request.interval_unit.trim().is_empty() {
            return Err(Error::validation(
                "Missing required parameter: interval_unit",
            ));
        }
        let body = json_body(request)?;
        self.request_reply(
            "POST",
            PLANS_PATH,
            "",
            &body,
            request.idempotency_key.as_str(),
        )?
        .object("plan")
    }

    /// Lists your plans: at most 200, active first, and the newest first
    /// within a status. The route has no pages yet; read a plan by id with
    /// [`Client::get_plan`] when you have more.
    ///
    /// Errors it returns: as [`Client::get_plan`], without the 404.
    pub fn list_plans(&self) -> Result<Vec<Plan>> {
        // GET signs an EMPTY idempotency key and an EMPTY body.
        self.request_reply("GET", PLANS_PATH, "", "", "")?
            .array("plan")
    }

    /// Reads one plan, archived ones too.
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: `plan_id` cannot be a plan id; nothing was
    ///   sent.
    /// - [`Error::Auth`] and [`Error::Forbidden`]: as on
    ///   [`Client::create_plan`].
    /// - [`Error::Api`]: 404 `PLAN_NOT_FOUND`, a plan that is not yours, or
    ///   400 `INVALID_FORMAT` for an id of the wrong form.
    /// - [`Error::RateLimited`] and [`Error::Transport`]: wait, then read
    ///   again.
    pub fn get_plan(&self, plan_id: &str) -> Result<Plan> {
        let path = plan_path(plan_id)?;
        self.request_reply("GET", &path, "", "", "")?.object("plan")
    }

    /// Changes the name, the trial or the cancel page of a plan, and answers
    /// the plan as it is now. A field you do not set keeps its value. A trial
    /// change reaches only the agreements created afterwards; a cancel page
    /// change shows on the next email.
    ///
    /// PATCH carries no `Idempotency-Key`: the API does not replay it, and the
    /// same update twice gives the same plan, so a retry is safe.
    ///
    /// Errors it returns: as [`Client::get_plan`]. An update that sets no
    /// field is refused with HTTP 400.
    pub fn update_plan(&self, plan_id: &str, update: &PlanUpdate) -> Result<Plan> {
        let path = plan_path(plan_id)?;
        let body = json_body(update)?;
        // PATCH signs an EMPTY idempotency key, like GET, but signs its body.
        self.request_reply("PATCH", &path, "", &body, "")?
            .object("plan")
    }

    /// Archives a plan: it accepts no new agreements, and the agreements it
    /// has continue on their own terms. An archived plan stays readable.
    /// Archiving an archived plan succeeds again, so a retry after a timeout
    /// is safe.
    ///
    /// Errors it returns: as [`Client::get_plan`].
    pub fn delete_plan(&self, plan_id: &str) -> Result<()> {
        let path = plan_path(plan_id)?;
        // DELETE signs an EMPTY idempotency key and an EMPTY body; the answer
        // is a 204 with no body.
        self.request_reply("DELETE", &path, "", "", "")?;
        Ok(())
    }

    /// Subscribes one of your customers to a plan. The agreement is created
    /// `pending` and nothing is charged. Then open the subscription session:
    /// [`Client::create_checkout_session`] with
    /// [`CheckoutSessionRequest::agreement_id`] takes the first payment, saves
    /// the card and activates the agreement. With scheduled charges on, the
    /// platform charges each period. It sends a `charge.*` webhook for each of
    /// those charges. Until we switch scheduled charges on for your account,
    /// the platform runs no scheduled charge and no retry.
    ///
    /// The agreement carries the request's required `Idempotency-Key`, with
    /// the same replay rules as [`Client::create_plan`]: the same key and body
    /// answer the agreement it created (HTTP 200 instead of 201).
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: a missing plan id or customer reference;
    ///   nothing was sent.
    /// - [`Error::Auth`] and [`Error::Forbidden`]: as on
    ///   [`Client::create_plan`].
    /// - [`Error::Api`]: HTTP 400 for a field the API refused (the details
    ///   are in `raw`, under `error.validationErrors`); 404 `PLAN_NOT_FOUND`
    ///   or `PAYMENT_METHOD_NOT_FOUND`; 409 `PLAN_NOT_ACTIVE` or
    ///   `PAYMENT_METHOD_NOT_ACTIVE`; 422 `MAX_AMOUNT_BELOW_PLAN_AMOUNT`,
    ///   `PAYMENT_METHOD_CURRENCY_MISMATCH`, `PAYMENT_METHOD_CURRENCY_UNKNOWN`,
    ///   `AGREEMENT_CONSENT_REQUIRED` or `IDEMPOTENCY_KEY_REUSED`.
    /// - [`Error::RateLimited`] and [`Error::Transport`]: as on
    ///   [`Client::create_plan`]; send it again WITH the same idempotency key.
    pub fn create_agreement(&self, request: &AgreementRequest) -> Result<Agreement> {
        if request.plan_id.trim().is_empty() {
            return Err(Error::validation("Missing required parameter: plan_id"));
        }
        if request.customer_reference.trim().is_empty() {
            return Err(Error::validation(
                "Missing required parameter: customer_reference",
            ));
        }
        let body = json_body(request)?;
        self.request_reply(
            "POST",
            AGREEMENTS_PATH,
            "",
            &body,
            request.idempotency_key.as_str(),
        )?
        .object("agreement")
    }

    /// Reads one agreement: its status, its next charge, why it is past due,
    /// and its `sequence` to order it against the `agreement.*` webhooks.
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: `agreement_id` cannot be an agreement id;
    ///   nothing was sent.
    /// - [`Error::Auth`] and [`Error::Forbidden`]: as on
    ///   [`Client::create_plan`].
    /// - [`Error::Api`]: 404 `AGREEMENT_NOT_FOUND`, an agreement that is not
    ///   yours, or 400 `INVALID_FORMAT` for an id of the wrong form.
    /// - [`Error::RateLimited`] and [`Error::Transport`]: wait, then read
    ///   again.
    pub fn get_agreement(&self, agreement_id: &str) -> Result<Agreement> {
        let path = agreement_path(agreement_id)?;
        self.request_reply("GET", &path, "", "", "")?
            .object("agreement")
    }

    /// Cancels an agreement that is pending, active or past due. No further
    /// charge is scheduled; a charge already placed is not reversed. The
    /// `agreement.cancelled` webhook follows. Cancelling a cancelled agreement
    /// succeeds again, so a retry after a timeout is safe.
    ///
    /// Errors it returns: as [`Client::get_agreement`], plus 409
    /// `AGREEMENT_ENDED` for an agreement that completed or failed.
    pub fn cancel_agreement(&self, agreement_id: &str) -> Result<()> {
        let path = agreement_path(agreement_id)?;
        // DELETE signs an EMPTY idempotency key and an EMPTY body; the answer
        // is a 204 with no body.
        self.request_reply("DELETE", &path, "", "", "")?;
        Ok(())
    }

    /// Registers a webhook endpoint: the platform POSTs the events you name
    /// to `url`. Only this answer carries the endpoint's signing secret
    /// ([`WebhookEndpoint::secret`]), so store it at once. You can have 25
    /// active endpoints.
    ///
    /// Every webhook endpoint route needs a key with the `webhooks:manage`
    /// scope. A key with the `payments` scope cannot hold it.
    ///
    /// The endpoint carries the request's required `Idempotency-Key`. The same
    /// key with the same url and events answers the same endpoint without its
    /// secret, and registers nothing new. After a timeout or a 503, send it
    /// again WITH THE SAME KEY: the retry replaces an endpoint that the lost
    /// attempt registered, and answers a new endpoint with a new secret.
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: an empty url or no events; nothing was sent.
    /// - [`Error::Auth`]: as on [`Client::create_plan`].
    /// - [`Error::Forbidden`]: the key does not hold the `webhooks:manage`
    ///   scope.
    /// - [`Error::Api`]: HTTP 400 `WEBHOOK_URL_REJECTED` (the url is not HTTPS
    ///   on a public host) or `VALIDATION_ERROR` (for example an unknown event
    ///   type; the details are in `raw`, under `error.validationErrors`). 409
    ///   `WEBHOOK_LIMIT_REACHED`, or `DUPLICATE_REQUEST`: the first request
    ///   with this key still runs, so send it again with the same key after a
    ///   second. 409 `WEBHOOK_ALREADY_REGISTERED`: this key registered an
    ///   endpoint more than a day ago and that endpoint still exists. Its id is
    ///   in `raw`, under `error.params.webhookId`; use a fresh key for a new
    ///   endpoint. 422 `IDEMPOTENCY_KEY_REUSED`.
    /// - [`Error::RateLimited`]: HTTP 429. Wait, then send it again with the
    ///   same idempotency key.
    /// - [`Error::Transport`]: network failure or 5xx. Send it again WITH the
    ///   same idempotency key.
    pub fn create_webhook_endpoint(
        &self,
        request: &WebhookEndpointRequest,
    ) -> Result<WebhookEndpoint> {
        if request.url.trim().is_empty() {
            return Err(Error::validation("Missing required parameter: url"));
        }
        if request.events.is_empty() {
            return Err(Error::validation(
                "events must name at least one event type",
            ));
        }
        let body = json_body(request)?;
        self.request_reply(
            "POST",
            WEBHOOKS_PATH,
            "",
            &body,
            request.idempotency_key.as_str(),
        )?
        .object("webhook endpoint")
    }

    /// Lists your webhook endpoints, the newest first: every active endpoint
    /// and your newest 1000 inactive ones. No item carries a secret. The route
    /// has no pages.
    ///
    /// Errors it returns: as [`Client::delete_webhook_endpoint`], without the
    /// 404.
    pub fn list_webhook_endpoints(&self) -> Result<Vec<WebhookEndpoint>> {
        // GET signs an EMPTY idempotency key and an EMPTY body.
        self.request_reply("GET", WEBHOOKS_PATH, "", "", "")?
            .array("webhook endpoint")
    }

    /// Deletes a webhook endpoint. It stops receiving deliveries.
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: `webhook_id` is not a UUID; nothing was sent.
    /// - [`Error::Auth`] and [`Error::Forbidden`]: as on
    ///   [`Client::create_webhook_endpoint`].
    /// - [`Error::Api`]: 404 `WEBHOOK_NOT_FOUND` for an id that is unknown,
    ///   not yours or already deleted. A retry after a lost answer therefore
    ///   answers 404: treat that as done.
    /// - [`Error::RateLimited`] and [`Error::Transport`]: wait, then send it
    ///   again.
    pub fn delete_webhook_endpoint(&self, webhook_id: &str) -> Result<()> {
        let path = webhook_endpoint_path(webhook_id)?;
        // DELETE signs an EMPTY idempotency key and an EMPTY body; the answer
        // is a 204 with no body.
        self.request_reply("DELETE", &path, "", "", "")?;
        Ok(())
    }

    /// Lists one page of your payments created in a window, oldest first:
    /// checkout payments, charges on saved cards (yours and the platform's
    /// for an agreement) and verify-only sessions. A refund is not a row of
    /// its own: it shows on its payment, in `refunded_amount` and `status`.
    /// `fee`, `net_amount`, `settled_at` and `payout_id` are `None` until
    /// the acquirer settles the payment.
    ///
    /// The key needs the `transactions:read` scope. A key with the `payments`
    /// scope cannot hold it. A key with this scope is for the whole account,
    /// and it cannot be bound to a storefront.
    ///
    /// To read a whole window, send [`TransactionPage::next_cursor`] back as
    /// the cursor, with the same `from`, `to`, settlement window and
    /// `status`, until it is `None`. Paging never repeats or skips a row. The
    /// parameters travel in the query string, which is not part of the
    /// signature.
    ///
    /// ```no_run
    /// use dominaite::{Client, TransactionListParams};
    ///
    /// # fn main() -> Result<(), dominaite::Error> {
    /// # let client = Client::new("dmk_0123456789abcdef0123456789abcdef", "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", dominaite::SANDBOX_BASE_URL)?;
    /// let mut params =
    ///     TransactionListParams::new("2026-10-01T00:00:00Z", "2026-10-08T00:00:00Z").limit(100);
    /// loop {
    ///     let page = client.list_transactions(&params)?;
    ///     for transaction in &page.items {
    ///         println!("{} {}", transaction.transaction_id, transaction.status);
    ///     }
    ///     match page.next_cursor {
    ///         Some(cursor) => params = params.cursor(cursor),
    ///         None => break,
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: a missing `from` or `to`; nothing was sent.
    /// - [`Error::Auth`]: as on [`Client::create_plan`].
    /// - [`Error::Forbidden`]: the key does not hold the `transactions:read`
    ///   scope.
    /// - [`Error::Api`]: HTTP 400 for a parameter the API refused, for
    ///   example a window longer than 31 days, or a cursor that is not a
    ///   `next_cursor` (the details are in `raw`, under
    ///   `error.validationErrors`).
    /// - [`Error::RateLimited`] and [`Error::Transport`]: wait, then read the
    ///   page again.
    pub fn list_transactions(&self, params: &TransactionListParams) -> Result<TransactionPage> {
        if params.from.trim().is_empty() {
            return Err(Error::validation("Missing required parameter: from"));
        }
        if params.to.trim().is_empty() {
            return Err(Error::validation("Missing required parameter: to"));
        }
        let mut query = format!(
            "from={}&to={}",
            encode_query_value(&params.from),
            encode_query_value(&params.to)
        );
        if let Some(settled_from) = &params.settled_from {
            query.push_str("&settledFrom=");
            query.push_str(&encode_query_value(settled_from));
        }
        if let Some(settled_to) = &params.settled_to {
            query.push_str("&settledTo=");
            query.push_str(&encode_query_value(settled_to));
        }
        if let Some(status) = &params.status {
            query.push_str("&status=");
            query.push_str(&encode_query_value(status));
        }
        if let Some(limit) = params.limit {
            query.push_str("&limit=");
            query.push_str(&limit.to_string());
        }
        if let Some(cursor) = &params.cursor {
            query.push_str("&cursor=");
            query.push_str(&encode_query_value(cursor));
        }
        // GET signs the path only, with an EMPTY idempotency key and an EMPTY
        // body. The query string is not signed.
        self.request_reply("GET", TRANSACTIONS_PATH, &query, "", "")?
            .object("transaction page")
    }

    /// Signs and sends one call, and maps the response onto the error taxonomy:
    /// [`Client::send`] plus the generic rejection for any 4xx or 5xx. The
    /// reply carries the unwrapped `data` (an empty object for a 204) and the
    /// request id. `query` is already encoded, without the `?`, and is never
    /// signed. `body` and `idempotency_key` are both empty for GET and DELETE.
    fn request_reply(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: &str,
        idempotency_key: &str,
    ) -> Result<Reply> {
        let reply = self.send(method, path, query, body, idempotency_key)?;
        if reply.status >= 400 {
            return Err(reply.rejection());
        }
        Ok(reply)
    }

    /// Signs and sends one call, and parses whatever came back into a [`Reply`].
    ///
    /// Only what no route can use is an error here: transport failures, a
    /// redirect, a 429, a 401/403, and a body that is not a JSON object (a 5xx
    /// of that kind is a retryable transport error; anything else is an API
    /// error). Every other status comes back as a reply, so the payment method
    /// routes can read a 402 decline or a coded 502 as the typed answers they
    /// are, while [`Client::request`] rejects them generically.
    ///
    /// Writes the debug line when [`ClientBuilder::on_debug`] is set.
    fn send(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: &str,
        idempotency_key: &str,
    ) -> Result<Reply> {
        let started = Instant::now();
        let mut exchange = Exchange::default();
        let result = self.exchange(method, path, query, body, idempotency_key, &mut exchange);
        if let Some(hook) = &self.hooks.on_debug {
            call_hook(
                hook,
                &debug_line(method, path, &exchange, started.elapsed(), idempotency_key),
            );
        }
        result
    }

    /// The body of [`Client::send`]. Records the status and the request id in
    /// `exchange`, and reads the notice header off every response.
    fn exchange(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: &str,
        idempotency_key: &str,
        exchange: &mut Exchange,
    ) -> Result<Reply> {
        let timestamp = unix_seconds().to_string();
        let signature = sign_request(SignRequest {
            secret: &self.secret,
            timestamp: &timestamp,
            method,
            // The signed path is the canonical path only. The base URL's own
            // prefix (dev's /api, prod's /payments) is NOT part of it.
            path,
            idempotency_key,
            body,
        });

        // The query string travels in the URL only: the API signs the
        // canonical path without it.
        let url = if query.is_empty() {
            format!("{}{}", self.base_url, path)
        } else {
            format!("{}{}?{}", self.base_url, path, query)
        };
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(&url)
            .header("Content-Type", "application/json")
            // Some edges block requests without a real User-Agent, so always send one.
            .header("User-Agent", &self.user_agent)
            .header("X-Api-Key-Id", &self.key_id)
            .header("X-Timestamp", &timestamp)
            .header("X-Signature", &signature);

        // No Idempotency-Key header on GET or DELETE, matching the empty key
        // they signed.
        if !idempotency_key.is_empty() {
            builder = builder.header("Idempotency-Key", idempotency_key);
        }

        let http_request = builder
            .body(body)
            .map_err(|error| Error::validation(format!("Could not build the request: {error}")))?;

        // Forced per request, so it holds for an agent supplied through
        // ClientBuilder::agent as well - ureq's own default is ten redirects, and
        // a caller's agent never saw this crate's config. Request-level config
        // overrides the agent's.
        let http_request = self
            .agent
            .configure_request(http_request)
            .max_redirects(0)
            .build();

        let mut response = match self.agent.run(http_request) {
            Ok(response) => response,
            // An agent configured with http_status_as_error(true) - a caller's own
            // agent - surfaces the status here instead of as a response. Same
            // taxonomy, minus the API's message.
            Err(ureq::Error::StatusCode(status)) => {
                exchange.status = Some(status);
                return Err(classify_status(status, None, None, Value::Null));
            }
            Err(error) => {
                return Err(Error::transport(
                    format!("Could not reach the Dominaite API: {error}"),
                    Some(Box::new(error)),
                ));
            }
        };

        let http_status = response.status().as_u16();
        exchange.status = Some(http_status);
        // The header first. Not every deployment sends it yet, so the body's
        // metadata.requestId (the same id) fills in below once it is read.
        let header_request_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .map(|value| clean_header(value.as_bytes(), REQUEST_ID_MAX_CHARS))
            .filter(|value| !value.is_empty());
        exchange.request_id = header_request_id.clone();
        // Every status, before anything below can return: a deprecated SDK
        // hears about it on its errors as well as on its successes.
        if let Some(notice) = response.headers().get(NOTICE_HEADER) {
            self.notice(&clean_header(notice.as_bytes(), NOTICE_MAX_CHARS));
        }

        // Stop before reading the body: a redirect's body is whatever the
        // redirecting host wanted to say, and none of it is an API response.
        if is_redirect(http_status) {
            return Err(redirect_error(http_status).with_request_id(header_request_id.as_deref()));
        }

        // Read Retry-After off the response, which means before the body is
        // consumed. The body is read leniently, only for the code that tells the
        // platform's limits apart: a limiter in front of the API sends whatever
        // it likes, and that is still a rate limit.
        if http_status == 429 {
            let retry_after_seconds = retry_after_seconds(response.headers().get("retry-after"));
            let raw = response
                .body_mut()
                .read_to_string()
                .ok()
                .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                .filter(Value::is_object)
                .unwrap_or(Value::Null);
            let code = raw
                .get("error")
                .and_then(|error| string_field(error, "code"));
            let request_id = header_request_id.or_else(|| body_request_id(&raw));
            exchange.request_id = request_id.clone();
            return Err(Error::RateLimited {
                retry_after_seconds,
                code,
                raw,
                request_id,
            });
        }

        // 204 carries nothing to parse; the status is the whole answer.
        if http_status == 204 {
            return Ok(Reply::new(
                http_status,
                Value::Object(serde_json::Map::new()),
                header_request_id,
            ));
        }

        // Bounded on purpose: `read_to_string` caps at ureq's default 10MB, so a
        // server that answers with an endless body cannot grow this allocation
        // until the caller's process is killed. Do NOT reach for
        // `with_config().limit(...)` here to raise it - a merchant API response
        // is a few kilobytes, and `an_oversized_response_body_stops_at_the_read_limit`
        // pins the cap. Hitting it reads as a transport failure, which is right:
        // nothing usable arrived.
        let raw = response.body_mut().read_to_string().map_err(|error| {
            Error::transport(
                format!("Could not read the Dominaite API response: {error}"),
                Some(Box::new(error)),
            )
            .with_request_id(header_request_id.as_deref())
        })?;

        // A body that is not a JSON object is classified on the STATUS. A
        // 502/503/504 of that kind comes from a load balancer or a cold function
        // host, not from the API, so the body is an HTML error page or empty;
        // reading it as "the API returned a non-JSON response" would make a
        // non-retryable Api error out of what is plainly a retryable outage,
        // and the with_retry helper would give up on the first attempt. A JSON
        // 5xx is different: that is the gateway itself talking, and the payment
        // method routes need its code.
        let envelope = match serde_json::from_str::<Value>(&raw) {
            Ok(envelope) if envelope.is_object() => envelope,
            _ if http_status >= 500 => {
                return Err(classify_status(http_status, None, None, Value::Null)
                    .with_request_id(header_request_id.as_deref()))
            }
            _ => {
                return Err(
                    Error::api(http_status, "The API returned a non-JSON response")
                        .with_request_id(header_request_id.as_deref()),
                )
            }
        };

        let reply = Reply::new(http_status, envelope, header_request_id);
        exchange.request_id = reply.request_id.clone();
        // Credentials and scopes are refused the same way on every route.
        if matches!(http_status, 401 | 403) {
            return Err(reply.rejection());
        }
        Ok(reply)
    }
}

/// One parsed gateway answer: the HTTP status and the envelope as sent.
///
/// Three shapes arrive and only one of them is nested: create answers
/// `{ success, data: { success, checkout } }`, while the status and ping reads
/// answer `{ success, data: { ...fields } }` with no inner `success` and no
/// wrapper object. So [`Reply::payload`] only ever unwraps `data`; branching on
/// the inner `success` belongs to the caller that knows which shape it asked
/// for. Treating a missing `success` as false would mark every paid order unpaid.
struct Reply {
    status: u16,
    /// The whole body: `{ success, data?, error?, metadata }`.
    envelope: Value,
    /// The `Dominaite-Request-Id` header, else `metadata.requestId`.
    request_id: Option<String>,
}

impl Reply {
    /// The header's request id wins; the body's is the fallback.
    fn new(status: u16, envelope: Value, header_request_id: Option<String>) -> Reply {
        let request_id = header_request_id.or_else(|| body_request_id(&envelope));
        Reply {
            status,
            envelope,
            request_id,
        }
    }

    fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// One `T` from the unwrapped `data`, with `raw` and `request_id` filled
    /// in. `what` names the object in the error.
    fn object<T: KeepsRaw>(&self, what: &str) -> Result<T> {
        parse_object(self.payload().clone(), self.status, what, self.request_id())
    }

    /// Every `T` of a list route, whose `data` is a JSON array.
    fn array<T: KeepsRaw>(&self, what: &str) -> Result<Vec<T>> {
        parse_array(self.payload().clone(), what, self.request_id())
    }

    /// The envelope's own `success` flag, true only when it is literally true.
    fn success(&self) -> bool {
        self.envelope.get("success").and_then(Value::as_bool) == Some(true)
    }

    /// `data` when it is an object, or an array on the list routes, else
    /// nothing: the gateway omits null fields on the wire, so a bodiless error
    /// has no `data` at all.
    fn data(&self) -> Option<&Value> {
        self.envelope
            .get("data")
            .filter(|data| data.is_object() || data.is_array())
    }

    /// The unwrapped `data`, or the envelope itself when there is none.
    fn payload(&self) -> &Value {
        self.data().unwrap_or(&self.envelope)
    }

    /// The charge row in `data`, on a 201, a 402 and the coded 502s that
    /// attach one. `None` when `data` is missing or carries no charge id.
    fn charge(&self) -> Option<PaymentMethodCharge> {
        let data = self.data()?;
        string_field(data, "chargeId")?;
        let mut charge: PaymentMethodCharge = serde_json::from_value(data.clone()).ok()?;
        charge.raw = data.clone();
        charge.request_id = self.request_id.clone();
        Some(charge)
    }

    /// The gateway's machine-readable code: `error.code` on the standard
    /// envelope, or `errorCode` inside `data` on the create route's refusals.
    fn error_code(&self) -> Option<String> {
        self.envelope
            .get("error")
            .and_then(|error| string_field(error, "code"))
            .or_else(|| string_field(self.payload(), "errorCode"))
    }

    fn error_message(&self) -> Option<String> {
        self.envelope
            .get("error")
            .and_then(|error| string_field(error, "message"))
            .or_else(|| string_field(self.payload(), "errorMessage"))
    }

    /// The generic error for a 4xx or 5xx: a 5xx is a retryable transport
    /// failure, a 4xx an [`Error::Api`] that keeps the code.
    fn rejection(self) -> Error {
        let (code, message) = (self.error_code(), self.error_message());
        classify_status(self.status, code, message, self.envelope)
            .with_request_id(self.request_id.as_deref())
    }
}

/// The one refusal the session retry helper sends again: card payments are off
/// for now and nothing was created, so the same key comes back safely later.
fn is_processing_unavailable(error: &Error) -> bool {
    matches!(
        error,
        Error::Refusal { code, .. } if code == session_error_code::PAYMENT_PROCESSING_UNAVAILABLE
    )
}

/// Whether a coded answer on the charge route is a charge outcome
/// ([`Error::Charge`]): any 4xx except input validation, credentials and rate
/// limiting, and the 502 and 503 the charge route documents. A coded 404
/// (`PAYMENT_METHOD_NOT_FOUND`, `AGREEMENT_NOT_FOUND`) is a charge outcome too.
/// Any other 5xx is an outage and stays a retryable [`Error::Transport`].
fn is_charge_outcome_status(status: u16) -> bool {
    match status {
        400 | 401 | 403 | 429 => false,
        402..=499 => true,
        502 | 503 => true,
        _ => false,
    }
}

/// Whether a coded answer on the revoke route is a revoke outcome
/// ([`Error::Revoke`]). A 404 stays [`Error::Api`]: the API answers it with a
/// generic validation code. Any 5xx other than 502 and 503 is an outage.
fn is_revoke_outcome_status(status: u16) -> bool {
    match status {
        400 | 401 | 403 | 404 | 429 => false,
        402..=499 => true,
        502 | 503 => true,
        _ => false,
    }
}

/// Why this base URL is not usable, or `None` when it is fine.
///
/// The secret itself never travels, but the signed headers and the cashier token
/// do, and over plaintext both are readable and replayable by anything on the
/// path. A loopback host is exempt because it never leaves the machine, which is
/// what makes local mocks and integration tests work.
fn base_url_problem(base_url: &str) -> Option<String> {
    // Schemes and hostnames are case-insensitive, so compare on a lowered copy
    // and keep the original for the message the integrator reads.
    let lowered = base_url.to_ascii_lowercase();
    if lowered.starts_with("https://") {
        return None;
    }
    match lowered.strip_prefix("http://") {
        Some(rest) if is_loopback_host(rest) => None,
        Some(_) => Some(format!(
            "base_url must use https://, or http:// with a loopback host: {base_url}"
        )),
        None => Some(format!("base_url must start with https://: {base_url}")),
    }
}

/// Whether the authority in `rest` (everything after `http://`) is loopback.
fn is_loopback_host(rest: &str) -> bool {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Only what is after the last `@` is the host. Without this,
    // `http://localhost@example.com/` reads as loopback and ships the signed
    // headers to example.com in plaintext.
    let authority = authority.rsplit('@').next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        // An IPv6 literal is bracketed, and its port sits after the bracket.
        Some(inside) => inside.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// Reads `Retry-After` when it is the integer-seconds form.
///
/// The HTTP-date form is valid HTTP and the platform does not send it. Reading a
/// date would mean subtracting it from a local clock that may well be the reason
/// the call is failing, so an unparseable header answers `None` and the caller
/// backs off on its own schedule.
fn retry_after_seconds(header: Option<&ureq::http::HeaderValue>) -> Option<u64> {
    header?.to_str().ok()?.trim().parse::<u64>().ok()
}

fn is_redirect(status: u16) -> bool {
    (300..400).contains(&status)
}

/// A 3xx is a hard stop, never a retry. The Dominaite API does not redirect, so
/// something else is answering for it, and whatever is at the other end would be
/// handed the signed headers and believed.
fn redirect_error(status: u16) -> Error {
    Error::api(
        status,
        "unexpected redirect response; the Dominaite API never redirects",
    )
}

fn classify_status(
    status: u16,
    code: Option<String>,
    message: Option<String>,
    raw: Value,
) -> Error {
    // Defence in depth only. ureq does not surface a 3xx as a StatusCode error,
    // it follows it - what actually stops a redirect is the per-request
    // max_redirects(0) in `request`.
    if is_redirect(status) {
        return redirect_error(status);
    }
    match status {
        // Reached from the StatusCode path only, where the response - and with
        // it Retry-After - is already gone. The normal path answers 429 in
        // `send`, with the header and the body.
        429 => Error::RateLimited {
            retry_after_seconds: None,
            code,
            raw,
            request_id: None,
        },
        // The clock hint is for 401 only. A 403 was signed correctly, so the
        // key id, the secret and the clock are fine.
        401 => Error::auth(
            code.unwrap_or_else(|| "UNAUTHORIZED".to_string()),
            message.unwrap_or_else(|| {
                "Authentication failed - check your key id, secret, and server clock.".to_string()
            }),
        ),
        403 => Error::forbidden(code, message),
        // Retryable whatever the body says, and the code the API sent, if any,
        // travels with it.
        status if status >= 500 => Error::unavailable(status, code, message, raw),
        // The code is the whole point of a validation rejection
        // (IDEMPOTENCY_KEY_REQUIRED on a 400), so it travels with the error
        // instead of being flattened into a status the caller cannot branch on.
        status => Error::api_with_code(
            status,
            code,
            message.unwrap_or_else(|| "Request rejected".to_string()),
            raw,
        ),
    }
}

/// Validates the request and returns the idempotency key plus the exact body
/// bytes that get both signed and sent. The key was validated when it was
/// built, so it goes out as is. Serializing once is the point: hashing a
/// second serialization would let key ordering or escaping drift between the
/// signature and the wire.
fn prepare_session_request(request: &CheckoutSessionRequest) -> Result<(String, String)> {
    if request.verify_only == Some(true) {
        // The platform sets the verification amount, so the gateway takes 0 only.
        if request.amount != 0 {
            return Err(Error::validation(
                "amount must be 0 with verify_only: the platform sets the verification amount",
            ));
        }
        validate_order_params(&request.currency, &request.order_reference)?;
    } else {
        validate_money_params(request.amount, &request.currency, &request.order_reference)?;
    }
    let idempotency_key = request.idempotency_key.as_str().to_string();

    let body = serde_json::to_string(request).map_err(|error| {
        Error::validation(format!(
            "Request parameters are not JSON-encodable: {error}"
        ))
    })?;

    Ok((idempotency_key, body))
}

/// `prepare_session_request` for a charge: same money checks, same key
/// handling, and the body is the contract's fields in declaration order, then
/// the caller's extra fields, which is what gets signed.
fn prepare_charge_request(request: &ChargeRequest) -> Result<(String, String)> {
    validate_money_params(request.amount, &request.currency, &request.order_reference)?;
    let idempotency_key = request.idempotency_key.as_str().to_string();

    let body = serde_json::to_string(request).map_err(|error| {
        Error::validation(format!(
            "Request parameters are not JSON-encodable: {error}"
        ))
    })?;

    Ok((idempotency_key, body))
}

/// `prepare_charge_request` for a refund: the amount is optional, but when it
/// is there it follows the same rule, and the body carries no `amount` key at
/// all for a refund of everything still refundable.
fn prepare_refund_request(request: &RefundRequest) -> Result<(String, String)> {
    if matches!(request.amount, Some(amount) if amount <= 0) {
        return Err(Error::validation(
            "amount must be a positive integer in MINOR units (e.g. 2500 for 25.00 EUR), or None to refund everything still refundable",
        ));
    }
    let idempotency_key = request.idempotency_key.as_str().to_string();

    let body = serde_json::to_string(request).map_err(|error| {
        Error::validation(format!(
            "Request parameters are not JSON-encodable: {error}"
        ))
    })?;

    Ok((idempotency_key, body))
}

/// A response type that keeps the object it was parsed from in `raw`, and the
/// id of the call that answered it in `request_id`.
trait KeepsRaw: DeserializeOwned {
    fn keep_raw(&mut self, raw: Value, request_id: Option<&str>);
}

/// [`KeepsRaw`] for a type whose `raw` and `request_id` are all there is to
/// fill in.
macro_rules! keeps_raw {
    ($($ty:ty),*) => {$(
        impl KeepsRaw for $ty {
            fn keep_raw(&mut self, raw: Value, request_id: Option<&str>) {
                self.raw = raw;
                self.request_id = request_id.map(str::to_string);
            }
        }
    )*};
}

keeps_raw!(
    Ping,
    IntegrationContract,
    CheckoutSession,
    CheckoutStatus,
    Refund,
    Plan,
    Agreement,
    WebhookEndpoint
);

impl KeepsRaw for RefundList {
    /// The list and each refund keep their own object and the request id.
    fn keep_raw(&mut self, raw: Value, request_id: Option<&str>) {
        if let Some(items) = raw.get("refunds").and_then(Value::as_array) {
            for (refund, item_raw) in self.refunds.iter_mut().zip(items) {
                refund.keep_raw(item_raw.clone(), request_id);
            }
        }
        self.raw = raw;
        self.request_id = request_id.map(str::to_string);
    }
}

impl KeepsRaw for TransactionPage {
    /// The page keeps its own object, and each item keeps its own.
    fn keep_raw(&mut self, raw: Value, request_id: Option<&str>) {
        if let Some(items) = raw.get("items").and_then(Value::as_array) {
            for (item, item_raw) in self.items.iter_mut().zip(items) {
                item.raw = item_raw.clone();
            }
        }
        self.raw = raw;
        self.request_id = request_id.map(str::to_string);
    }
}

/// One `T` from `payload`, with `raw` and `request_id` filled in. `what` names
/// the object in the error.
fn parse_object<T: KeepsRaw>(
    payload: Value,
    http_status: u16,
    what: &str,
    request_id: Option<&str>,
) -> Result<T> {
    let mut parsed: T = serde_json::from_value(payload.clone()).map_err(|_| {
        Error::api(
            http_status,
            format!("The API returned an unexpected {what} response"),
        )
        .with_request_id(request_id)
    })?;
    parsed.keep_raw(payload, request_id);
    Ok(parsed)
}

/// Every `T` of a list route, whose `data` is a JSON array. Each item keeps
/// its own object in `raw`, and the request id of the list.
fn parse_array<T: KeepsRaw>(
    payload: Value,
    what: &str,
    request_id: Option<&str>,
) -> Result<Vec<T>> {
    let Value::Array(items) = payload else {
        return Err(
            Error::api(200, format!("The API returned an unexpected {what} list"))
                .with_request_id(request_id),
        );
    };
    items
        .into_iter()
        .map(|item| parse_object(item, 200, what, request_id))
        .collect()
}

/// `metadata.requestId` in a response body.
fn body_request_id(envelope: &Value) -> Option<String> {
    envelope
        .get("metadata")
        .and_then(|metadata| string_field(metadata, "requestId"))
        .map(|id| clean_header(id.as_bytes(), REQUEST_ID_MAX_CHARS))
        .filter(|id| !id.is_empty())
}

/// The JSON body of a request, the exact text that gets signed.
fn json_body<T: Serialize>(request: &T) -> Result<String> {
    serde_json::to_string(request).map_err(|error| {
        Error::validation(format!(
            "Request parameters are not JSON-encodable: {error}"
        ))
    })
}

/// The signed path of one plan. A plan id is opaque and case-sensitive, so it
/// goes into the path exactly as given.
fn plan_path(plan_id: &str) -> Result<String> {
    let id = normalize_opaque_id(plan_id, "plan_id must be the id create_plan returned")?;
    Ok(format!("{PLANS_PATH}/{id}"))
}

/// The signed path of one agreement. An agreement id is opaque and
/// case-sensitive, like a plan id.
fn agreement_path(agreement_id: &str) -> Result<String> {
    let id = normalize_opaque_id(
        agreement_id,
        "agreement_id must be the id create_agreement returned",
    )?;
    Ok(format!("{AGREEMENTS_PATH}/{id}"))
}

/// Percent-encodes one query value: every byte except the unreserved
/// characters of RFC 3986. A `+` goes out as `%2B`, so an offset such as
/// `+02:00` is not read as a space.
fn encode_query_value(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    encoded
}

/// The signed path of one webhook endpoint, the id in lowercase hyphenated
/// form as the API signs it.
fn webhook_endpoint_path(webhook_id: &str) -> Result<String> {
    let id = webhook_id.trim().to_lowercase();
    if !is_uuid(&id) {
        return Err(Error::validation(
            "webhook_id must be the UUID create_webhook_endpoint returned",
        ));
    }
    Ok(format!("{WEBHOOKS_PATH}/{id}"))
}

/// The checks shared by every request that moves money: amount, currency,
/// order reference.
fn validate_money_params(amount: i64, currency: &str, order_reference: &str) -> Result<()> {
    if amount <= 0 {
        return Err(Error::validation(
            "amount must be a positive integer in MINOR units (e.g. 2500 for 25.00 EUR)",
        ));
    }
    validate_order_params(currency, order_reference)
}

/// Currency and order reference: the part of [`validate_money_params`] that a
/// verify-only session, which moves no money, still needs.
fn validate_order_params(currency: &str, order_reference: &str) -> Result<()> {
    if currency.trim().is_empty() {
        return Err(Error::validation("Missing required parameter: currency"));
    }
    if order_reference.trim().is_empty() {
        return Err(Error::validation(
            "Missing required parameter: order_reference",
        ));
    }
    // Characters, not bytes. `len()` counts UTF-8 bytes, so a Cyrillic or Greek
    // order reference hit the limit at 50 characters and a CJK one at 33, and
    // the caller got a validation error for a reference the API accepts.
    //
    // The server counts UTF-16 units and stays the final arbiter, so a reference
    // built from astral characters (emoji, rarer CJK) can still be rejected
    // there: each one is a single code point here and two units there.
    if order_reference.chars().count() > 100 {
        return Err(Error::validation(
            "order_reference must be at most 100 characters",
        ));
    }
    Ok(())
}

/// A payment method id (`pm_...`) must stay a single path segment: no slash,
/// no query, no whitespace, nothing that needs percent-encoding. Anything else
/// would sign one path and request another.
///
/// The id is lowercased. The API signs the lowercase id, so an id typed in
/// upper case would fail with `INVALID_SIGNATURE`. The API itself only hands
/// out lowercase ids.
fn normalize_payment_method_id(payment_method_id: &str) -> Result<String> {
    normalize_opaque_id(
        payment_method_id,
        "payment_method_id must be the payment_method.id from get_status",
    )
    .map(|id| id.to_ascii_lowercase())
}

/// The single-path-segment rule of [`normalize_payment_method_id`], for any
/// opaque id (`pm_...`, `re_...`) that goes into a signed path.
fn normalize_opaque_id(id: &str, problem: &str) -> Result<String> {
    let normalized = id.trim();
    let well_formed = !normalized.is_empty()
        && normalized.len() <= 100
        && normalized
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if !well_formed {
        return Err(Error::validation(problem));
    }
    Ok(normalized.to_string())
}

/// A transaction id goes into the signed path in lowercase hyphenated form.
fn normalize_transaction_id(transaction_id: &str) -> Result<String> {
    let normalized = transaction_id.trim().to_lowercase();
    if !is_uuid(&normalized) {
        return Err(Error::validation(
            "transaction_id must be the UUID returned by create_checkout_session",
        ));
    }
    Ok(normalized)
}

/// Calls a hook and drops a panic, so a broken log sink never fails a payment
/// call.
fn call_hook(hook: &Sink, message: &str) {
    let _ = catch_unwind(AssertUnwindSafe(|| hook(message)));
}

/// A header value as text: at most `max_chars` characters, control characters
/// removed, surrounding whitespace trimmed.
fn clean_header(bytes: &[u8], max_chars: usize) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(max_chars)
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

/// The warning for a `Dominaite-Sdk-Notice` value such as
/// `deprecated; current="0.7.0"; sunset="2027-01-31"; info="https://..."`.
fn notice_message(value: &str) -> String {
    let mut parts = value.split(';').map(str::trim);
    if !parts
        .next()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("deprecated"))
    {
        return format!("dominaite-rust {VERSION}: {value}");
    }

    let (mut current, mut sunset, mut info) = (None, None, None);
    for part in parts {
        let Some((key, raw)) = part.split_once('=') else {
            continue;
        };
        let raw = raw.trim();
        let raw = raw
            .strip_prefix('"')
            .and_then(|inner| inner.strip_suffix('"'))
            .unwrap_or(raw)
            .trim();
        if raw.is_empty() {
            continue;
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "current" => current = Some(raw),
            "sunset" => sunset = Some(raw),
            "info" => info = Some(raw),
            _ => {}
        }
    }

    let mut message = format!("dominaite-rust {VERSION} is deprecated");
    if let Some(current) = current {
        message.push_str(&format!("; upgrade to {current}"));
    }
    if let Some(sunset) = sunset {
        message.push_str(&format!(" (sunset {sunset})"));
    }
    message.push('.');
    if let Some(info) = info {
        message.push_str(&format!(" {info}"));
    }
    message
}

/// The debug line for one exchange. Never a body, a query string or a header
/// that signs or authenticates.
fn debug_line(
    method: &str,
    path: &str,
    exchange: &Exchange,
    elapsed: Duration,
    idempotency_key: &str,
) -> String {
    let status = exchange
        .status
        .map_or_else(|| "error".to_string(), |status| status.to_string());
    let mut line = format!(
        "dominaite-rust: {method} {path} {status} {}ms",
        elapsed.as_millis()
    );
    if let Some(request_id) = &exchange.request_id {
        line.push_str(&format!(" request_id={request_id}"));
    }
    if !idempotency_key.is_empty() {
        line.push_str(&format!(" idempotency_key={idempotency_key}"));
    }
    line
}

/// Why a retry happens, for the warning: the error code, else the HTTP
/// status, else the kind of error. Never the message, which may quote a body.
fn retry_reason(error: &Error) -> String {
    if let Some(code) = error.code() {
        return code.to_string();
    }
    if let Some(status) = error.http_status() {
        return format!("HTTP {status}");
    }
    match error {
        Error::Transport { .. } => "Transport".to_string(),
        _ => "Error".to_string(),
    }
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn is_uuid(candidate: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let mut parts = candidate.split('-');
    for expected in groups {
        match parts.next() {
            Some(part)
                if part.len() == expected
                    && part.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && part.bytes().all(|byte| !byte.is_ascii_uppercase()) => {}
            _ => return false,
        }
    }
    parts.next().is_none()
}

pub(crate) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idempotency::IdempotencyKey;

    #[test]
    fn uuid_check_rejects_near_misses() {
        assert!(is_uuid("00000000-0000-4000-8000-000000000001"));
        assert!(!is_uuid("00000000-0000-4000-8000-00000000000"));
        assert!(!is_uuid("00000000-0000-4000-8000-000000000001-x"));
        assert!(!is_uuid("0000000G-0000-4000-8000-000000000001"));
    }

    #[test]
    fn body_serializes_once_with_extra_fields_merged() {
        let key = IdempotencyKey::new("order-1042").expect("valid key");
        let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key)
            .extra("splitPayment", serde_json::json!(true));
        let (_, body) = prepare_session_request(&request).expect("valid request");
        assert_eq!(
            body,
            r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042","splitPayment":true}"#
        );
    }
}
