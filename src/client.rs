//! The client: signing, sending, and mapping responses onto the error taxonomy.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::error::{session_error_code, Error, Result};
use crate::signing::{sign_request, SignRequest};
use crate::types::{
    ChargeRequest, CheckoutSession, CheckoutSessionRequest, CheckoutStatus, PaymentMethodCharge,
    Ping,
};

/// The production merchant API.
pub const DEFAULT_BASE_URL: &str = "https://api.dominaite.com/payments";

/// The canonical path that gets signed. POST creates a session; GET
/// `SESSIONS_PATH/{transaction_id}` reads its status.
pub const SESSIONS_PATH: &str = "/merchant-api/checkout/sessions";

/// The canonical path of stored payment methods. POST
/// `PAYMENT_METHODS_PATH/{payment_method_id}/charges` charges one; DELETE
/// `PAYMENT_METHODS_PATH/{payment_method_id}` revokes it.
pub const PAYMENT_METHODS_PATH: &str = "/merchant-api/payment-methods";

/// The credentials-and-clock smoke test. Creates nothing.
pub const PING_PATH: &str = "/merchant-api/ping";

/// This SDK's version, reported in the User-Agent.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// Builds a [`Client`]. Start with [`Client::builder`].
// SECURITY: do not derive Serialize. The secret field would be emitted by any
// serde-based logger, which is how the Go SDK leaked it through json.Marshal.
#[derive(Clone)]
pub struct ClientBuilder {
    key_id: String,
    secret: String,
    base_url: String,
    timeout: Duration,
    user_agent: Option<String>,
    agent: Option<ureq::Agent>,
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
            .finish()
    }
}

impl ClientBuilder {
    /// Points the client at a non-production environment. Empty and
    /// whitespace-only values are ignored, so you can pass an unset environment
    /// variable straight through and still get production. Trailing slashes are
    /// trimmed.
    ///
    /// The URL has to be `https://`. [`ClientBuilder::build`] rejects anything
    /// else with [`Error::Validation`], except plain `http://` on a loopback
    /// host (`localhost`, `127.0.0.1`, `[::1]`) so a local mock still works.
    pub fn base_url(mut self, base_url: impl AsRef<str>) -> Self {
        let trimmed = base_url.as_ref().trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            self.base_url = trimmed.to_string();
        }
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
    /// Dominaite support reads the access logs for your integration.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        let value = user_agent.into();
        if !value.trim().is_empty() {
            self.user_agent = Some(value);
        }
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
    /// which catches a swapped key id and secret before anything is sent, and
    /// when the base URL is not `https://` on a non-loopback host.
    pub fn build(self) -> Result<Client> {
        if !self.key_id.starts_with("dmk_") {
            return Err(Error::validation("key_id must start with dmk_"));
        }
        if !self.secret.starts_with("dms_") {
            return Err(Error::validation("secret must start with dms_"));
        }
        if let Some(problem) = base_url_problem(&self.base_url) {
            return Err(Error::validation(problem));
        }

        let mut user_agent = format!("dominaite-rust/{VERSION}");
        if let Some(extra) = self.user_agent {
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
            base_url: self.base_url,
            user_agent,
            agent,
        })
    }
}

/// A server-side client for the Dominaite merchant API.
///
/// Cloning is cheap and shares the underlying connection pool, so one client per
/// process is the normal shape.
// SECURITY: do not derive Serialize. The secret field would be emitted by any
// serde-based logger, which is how the Go SDK leaked it through json.Marshal.
#[derive(Clone)]
pub struct Client {
    key_id: String,
    secret: String,
    base_url: String,
    user_agent: String,
    agent: ureq::Agent,
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
            .finish()
    }
}

impl Client {
    /// A client on the production base URL with the default 45s timeout.
    ///
    /// Both credentials come from the dashboard's Website integration tab.
    pub fn new(key_id: impl Into<String>, secret: impl Into<String>) -> Result<Client> {
        Client::builder(key_id, secret).build()
    }

    /// A builder, for a non-production base URL, a different timeout, or your own
    /// HTTP agent.
    pub fn builder(key_id: impl Into<String>, secret: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            key_id: key_id.into(),
            secret: secret.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            timeout: DEFAULT_TIMEOUT,
            user_agent: None,
            agent: None,
        }
    }

    /// The base URL this client is pointed at.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Verifies your credentials, your signing, and your clock without creating
    /// anything. Make this your first live call: a 401 here means the key id, the
    /// secret, or the signing, and a 503 means retry later - never both at once.
    ///
    /// Check [`Ping::clock_skew_seconds`]. Requests start failing at 300.
    pub fn ping(&self) -> Result<Ping> {
        // GET signs an EMPTY idempotency key and an EMPTY body.
        let payload = self.request("GET", PING_PATH, "", "")?;
        let mut ping: Ping = serde_json::from_value(payload.clone())
            .map_err(|_| Error::api(200, "The API returned an unexpected ping response"))?;
        ping.raw = payload;
        Ok(ping)
    }

    /// Opens a hosted checkout session for one payment.
    ///
    /// The errors it returns:
    /// - [`Error::Validation`]: bad arguments; nothing was sent.
    /// - [`Error::Auth`]: wrong credentials, bad signature, clock off, IP not
    ///   allowlisted. Fix the config, do not retry.
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
        let payload = self.request("POST", SESSIONS_PATH, &body, &idempotency_key)?;

        // Create is the nested shape: an inner `success` next to `checkout`.
        let succeeded = payload.get("success").and_then(Value::as_bool) == Some(true);
        let checkout = payload.get("checkout").filter(|value| value.is_object());

        match (succeeded, checkout) {
            (true, Some(checkout)) => {
                let mut session: CheckoutSession = serde_json::from_value(checkout.clone())
                    .map_err(|_| {
                        Error::api(200, "The API returned an unexpected checkout object")
                    })?;
                session.raw = checkout.clone();
                Ok(session)
            }
            // A replay refusal names the transaction the key collided with. Carry
            // it so the caller can reconcile with get_status instead of minting a
            // second payment for the same order.
            _ => Err(Error::refusal(
                string_field(&payload, "errorCode").unwrap_or_else(|| "UNKNOWN".to_string()),
                string_field(&payload, "errorMessage")
                    .unwrap_or_else(|| "The checkout session was refused.".to_string()),
                string_field(&payload, "transactionId"),
            )),
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
    /// What a replayed key gets back is a refusal, not the original session: the
    /// API answers HTTP 200 with `success: false` and one of the replay codes
    /// ([`Error::Refusal`] with `DUPLICATE_REQUEST`, `ALREADY_PROCESSED`,
    /// `PRIOR_ATTEMPT_FAILED` or `IDEMPOTENCY_KEY_REUSED`). The first attempt's
    /// cashier key and token are not returned again. When the refusal names a
    /// transaction id, read it back with [`Client::get_status`] to find out what
    /// the earlier attempt did.
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
                    last_error = Some(error);
                    if attempt + 1 < options.attempts {
                        std::thread::sleep(options.base_delay * 2u32.pow(attempt.min(16)));
                    }
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
        let normalized = transaction_id.trim().to_lowercase();
        if !is_uuid(&normalized) {
            return Err(Error::validation(
                "transaction_id must be the UUID returned by create_checkout_session",
            ));
        }

        // GET signs an EMPTY idempotency key and an EMPTY body, and sends no
        // Idempotency-Key header.
        let path = format!("{SESSIONS_PATH}/{normalized}");
        let payload = self.request("GET", &path, "", "")?;

        let mut status: CheckoutStatus = serde_json::from_value(payload.clone())
            .map_err(|_| Error::api(200, "The API returned an unexpected status response"))?;
        status.raw = payload;
        Ok(status)
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
    /// `status` `failed` plus a `decline_class` telling you whether to give up on
    /// the card (`hard`), wait (`soft_funds`, `soft_other`) or bring the customer
    /// back for a hosted session (`soft_sca_required`). `pending` is not
    /// terminal - poll [`Client::get_status`] with `charge.transaction_id`.
    ///
    /// Errors it returns:
    /// - [`Error::Validation`]: bad arguments; nothing was sent.
    /// - [`Error::Auth`]: wrong credentials, bad signature, clock off, IP not
    ///   allowlisted.
    /// - [`Error::Charge`]: the gateway answered with a code instead of a charge
    ///   (409, 422, 502, 503); branch on `code`. `CHARGE_OUTCOME_UNKNOWN` carries
    ///   the charge row to poll; never retry it under a new key.
    /// - [`Error::Api`]: 404 (`PAYMENT_METHOD_NOT_FOUND`) for an id that is not
    ///   yours, a 400 validation rejection, or an unexpected response.
    /// - [`Error::RateLimited`]: HTTP 429. Wait, then send it again with the
    ///   same idempotency key.
    /// - [`Error::Transport`]: network failure, or a 5xx that carries no gateway
    ///   code. Safe to retry WITH the same idempotency key.
    pub fn charge_payment_method(
        &self,
        payment_method_id: &str,
        request: &ChargeRequest,
    ) -> Result<PaymentMethodCharge> {
        let id = normalize_payment_method_id(payment_method_id)?;
        let (idempotency_key, body) = prepare_charge_request(request)?;
        let path = format!("{PAYMENT_METHODS_PATH}/{id}/charges");
        let reply = self.send("POST", &path, &body, &idempotency_key)?;

        // 201 (200 on a durable replay): the charge was placed, whatever its
        // status. 402: the provider declined; the envelope says success=false
        // but the charge is right there, status `failed` with its decline class,
        // so it is a result, not an error.
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
                .filter(|_| !is_generic_failure_status(reply.status))
            {
                let message = reply
                    .error_message()
                    .unwrap_or_else(|| "The charge was refused.".to_string());
                return Err(Error::charge(
                    reply.status,
                    code,
                    message,
                    charge,
                    reply.envelope,
                ));
            }
            return Err(reply.rejection());
        }

        Err(Error::api(
            reply.status,
            "The API answered the charge without a charge body",
        ))
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
        let reply = self.send("DELETE", &path, "", "")?;
        if reply.status < 400 {
            return Ok(());
        }

        if let Some(code) = reply
            .error_code()
            .filter(|_| !is_generic_failure_status(reply.status))
        {
            let message = reply
                .error_message()
                .unwrap_or_else(|| "The revoke was refused.".to_string());
            return Err(Error::revoke(reply.status, code, message, reply.envelope));
        }
        Err(reply.rejection())
    }

    /// Signs and sends one call, and maps the response onto the error taxonomy:
    /// [`Client::send`] plus the generic rejection for any 4xx or 5xx. The
    /// unwrapped `data` comes back on success (an empty object for a 204).
    /// `body` and `idempotency_key` are both empty for GET and DELETE.
    fn request(
        &self,
        method: &str,
        path: &str,
        body: &str,
        idempotency_key: &str,
    ) -> Result<Value> {
        let reply = self.send(method, path, body, idempotency_key)?;
        if reply.status >= 400 {
            return Err(reply.rejection());
        }
        Ok(reply.payload().clone())
    }

    /// Signs and sends one call, and parses whatever came back into a [`Reply`].
    ///
    /// Only what no route can use is an error here: transport failures, a
    /// redirect, a 429, a 401/403, and a body that is not a JSON object (a 5xx
    /// of that kind is a retryable transport error; anything else is an API
    /// error). Every other status comes back as a reply, so the payment method
    /// routes can read a 402 decline or a coded 502 as the typed answers they
    /// are, while [`Client::request`] rejects them generically.
    fn send(&self, method: &str, path: &str, body: &str, idempotency_key: &str) -> Result<Reply> {
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

        let url = format!("{}{}", self.base_url, path);
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
                return Err(classify_status(status, None, None));
            }
            Err(error) => {
                return Err(Error::transport(
                    format!("Could not reach the Dominaite API: {error}"),
                    Some(Box::new(error)),
                ));
            }
        };

        let http_status = response.status().as_u16();
        // Stop before reading the body: a redirect's body is whatever the
        // redirecting host wanted to say, and none of it is an API response.
        if is_redirect(http_status) {
            return Err(redirect_error(http_status));
        }

        // Read Retry-After off the response, which means before the body is
        // consumed. A limiter's body is whatever the edge felt like sending.
        if http_status == 429 {
            return Err(Error::RateLimited {
                retry_after_seconds: retry_after_seconds(response.headers().get("retry-after")),
            });
        }

        // 204 carries nothing to parse; the status is the whole answer.
        if http_status == 204 {
            return Ok(Reply::new(
                http_status,
                Value::Object(serde_json::Map::new()),
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
            _ if http_status >= 500 => return Err(classify_status(http_status, None, None)),
            _ => {
                return Err(Error::api(
                    http_status,
                    "The API returned a non-JSON response",
                ))
            }
        };

        let reply = Reply::new(http_status, envelope);
        // Credentials are refused the same way on every route.
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
}

impl Reply {
    fn new(status: u16, envelope: Value) -> Reply {
        Reply { status, envelope }
    }

    /// The envelope's own `success` flag, true only when it is literally true.
    fn success(&self) -> bool {
        self.envelope.get("success").and_then(Value::as_bool) == Some(true)
    }

    /// `data` when it is an object, else nothing: the gateway omits null fields
    /// on the wire, so a bodiless error has no `data` at all.
    fn data(&self) -> Option<&Value> {
        self.envelope.get("data").filter(|data| data.is_object())
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
        classify_status(self.status, self.error_code(), self.error_message())
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

/// The statuses that stay generic on every route: input validation, credentials,
/// an id that is not yours, and rate limiting. A coded answer outside this set
/// is the gateway describing a payment method outcome, which the charge and
/// revoke routes surface as [`Error::Charge`] and [`Error::Revoke`].
fn is_generic_failure_status(status: u16) -> bool {
    matches!(status, 400 | 401 | 403 | 404 | 429)
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

fn classify_status(status: u16, code: Option<String>, message: Option<String>) -> Error {
    // Defence in depth only. ureq does not surface a 3xx as a StatusCode error,
    // it follows it - what actually stops a redirect is the per-request
    // max_redirects(0) in `request`.
    if is_redirect(status) {
        return redirect_error(status);
    }
    match status {
        // Reached from the StatusCode path only, where the response - and with
        // it Retry-After - is already gone. The normal path answers 429 in
        // `request`, with the header.
        429 => Error::RateLimited {
            retry_after_seconds: None,
        },
        401 | 403 => Error::auth(
            code.unwrap_or_else(|| "UNAUTHORIZED".to_string()),
            message.unwrap_or_else(|| {
                "Authentication failed - check your key id, secret, and server clock.".to_string()
            }),
        ),
        status if status >= 500 => Error::transport(
            format!(
                "The Dominaite API is unavailable (HTTP {status}); retry with the same idempotency key."
            ),
            None,
        ),
        // The code is the whole point of a validation rejection
        // (IDEMPOTENCY_KEY_REQUIRED on a 400), so it travels with the error
        // instead of being flattened into a status the caller cannot branch on.
        status => Error::api_with_code(
            status,
            code,
            message.unwrap_or_else(|| "Request rejected".to_string()),
        ),
    }
}

/// Validates the request and returns the idempotency key plus the exact body
/// bytes that get both signed and sent. The key was validated when it was
/// built, so it goes out as is. Serializing once is the point: hashing a
/// second serialization would let key ordering or escaping drift between the
/// signature and the wire.
fn prepare_session_request(request: &CheckoutSessionRequest) -> Result<(String, String)> {
    validate_money_params(request.amount, &request.currency, &request.order_reference)?;
    let idempotency_key = request.idempotency_key.as_str().to_string();

    let body = serde_json::to_string(request).map_err(|error| {
        Error::validation(format!(
            "Request parameters are not JSON-encodable: {error}"
        ))
    })?;

    Ok((idempotency_key, body))
}

/// `prepare_session_request` for a charge: same money checks, same key
/// handling, and the body is exactly the contract's fields in declaration
/// order, which is what gets signed.
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

/// The checks shared by every request that moves money: amount, currency,
/// order reference.
fn validate_money_params(amount: i64, currency: &str, order_reference: &str) -> Result<()> {
    if amount <= 0 {
        return Err(Error::validation(
            "amount must be a positive integer in MINOR units (e.g. 2500 for 25.00 EUR)",
        ));
    }
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

/// A payment method id is opaque (`pm_...`), so this only pins what keeps it a
/// single path segment: no slash, no query, no whitespace, nothing that needs
/// percent-encoding. The id goes into the signed canonical path verbatim, so
/// anything else would sign one path and request another.
fn normalize_payment_method_id(payment_method_id: &str) -> Result<String> {
    let normalized = payment_method_id.trim();
    let well_formed = !normalized.is_empty()
        && normalized.len() <= 100
        && normalized
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if !well_formed {
        return Err(Error::validation(
            "payment_method_id must be the payment_method.id from get_status",
        ));
    }
    Ok(normalized.to_string())
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
