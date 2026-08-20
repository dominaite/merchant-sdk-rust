//! The client: signing, sending, and mapping responses onto the error taxonomy.

use std::io::Read;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::signing::{sign_request, SignRequest};
use crate::types::{CheckoutSession, CheckoutSessionRequest, CheckoutStatus, Ping};

/// The production merchant API.
pub const DEFAULT_BASE_URL: &str = "https://api.dominaite.com/payments";

/// The canonical path that gets signed. POST creates a session; GET
/// `SESSIONS_PATH/{transaction_id}` reads its status.
pub const SESSIONS_PATH: &str = "/merchant-api/bridgerpay/checkout/sessions";

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

/// Builds a [`Client`]. Start with [`Client::builder`].
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    key_id: String,
    secret: String,
    base_url: String,
    timeout: Duration,
    user_agent: Option<String>,
    agent: Option<ureq::Agent>,
}

impl ClientBuilder {
    /// Points the client at a non-production environment. Empty and
    /// whitespace-only values are ignored, so you can pass an unset environment
    /// variable straight through and still get production. Trailing slashes are
    /// trimmed.
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
    pub fn agent(mut self, agent: ureq::Agent) -> Self {
        self.agent = Some(agent);
        self
    }

    /// Validates the credentials and builds the client.
    ///
    /// Returns [`Error::Validation`] when either credential has the wrong prefix,
    /// which catches a swapped key id and secret before anything is sent.
    pub fn build(self) -> Result<Client> {
        if !self.key_id.starts_with("dmk_") {
            return Err(Error::validation("key_id must start with dmk_"));
        }
        if !self.secret.starts_with("dms_") {
            return Err(Error::validation("secret must start with dms_"));
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
#[derive(Debug, Clone)]
pub struct Client {
    key_id: String,
    secret: String,
    base_url: String,
    user_agent: String,
    agent: ureq::Agent,
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
    /// - [`Error::Api`]: an unexpected or rejecting response; inspect `status`.
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

    /// Creates a session, retrying [`Error::Transport`] only, with THE SAME
    /// idempotency key across every attempt.
    ///
    /// Reusing the key is what makes the retry safe. A transport failure leaves
    /// you not knowing whether the request landed; the API returns the original
    /// session for a key it has already seen instead of opening a second one.
    /// Generating a fresh key per attempt would be exactly the double-charge bug
    /// this method exists to prevent, so the key is pinned once before the first
    /// attempt.
    ///
    /// Refusals and authentication failures are returned immediately. They will
    /// not change on a retry.
    pub fn create_checkout_session_with_retry(
        &self,
        request: &CheckoutSessionRequest,
        options: RetryOptions,
    ) -> Result<CheckoutSession> {
        if options.attempts < 1 {
            return Err(Error::validation("attempts must be at least 1"));
        }

        let mut pinned = request.clone();
        if pinned.idempotency_key.is_none() {
            pinned.idempotency_key = Some(new_idempotency_key());
        }

        let mut last_error = None;
        for attempt in 0..options.attempts {
            match self.create_checkout_session(&pinned) {
                Ok(session) => return Ok(session),
                Err(error) if error.is_retryable() => {
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
    /// or on your order timeout. Not in a tight loop: the endpoint is rate
    /// limited per key. An unknown transaction id returns [`Error::Api`] with
    /// status 404.
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

    /// Signs and sends one call, and maps the response onto the error taxonomy.
    /// `body` and `idempotency_key` are both empty for GET.
    fn request(
        &self,
        method: &str,
        path: &str,
        body: &str,
        idempotency_key: &str,
    ) -> Result<Value> {
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

        // No Idempotency-Key header on GET, matching the empty key it signed.
        if !idempotency_key.is_empty() {
            builder = builder.header("Idempotency-Key", idempotency_key);
        }

        let http_request = builder
            .body(body)
            .map_err(|error| Error::validation(format!("Could not build the request: {error}")))?;

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
        let raw = response.body_mut().read_to_string().map_err(|error| {
            Error::transport(
                format!("Could not read the Dominaite API response: {error}"),
                Some(Box::new(error)),
            )
        })?;

        unwrap_envelope(http_status, &raw)
    }
}

/// Unwraps the gateway envelope and turns a non-2xx into the right error.
///
/// Three shapes reach this function and only one of them is nested:
/// create answers `{ success, data: { success, checkout } }`, while the status and
/// ping reads answer `{ success, data: { ...fields } }` with no inner `success`
/// and no wrapper object. So this only ever unwraps `data`; branching on the
/// inner `success` belongs to the caller that knows which shape it asked for.
/// Treating a missing `success` as false would mark every paid order unpaid.
fn unwrap_envelope(http_status: u16, raw: &str) -> Result<Value> {
    let envelope: Value = serde_json::from_str(raw)
        .map_err(|_| Error::api(http_status, "The API returned a non-JSON response"))?;
    if !envelope.is_object() {
        return Err(Error::api(
            http_status,
            "The API returned a non-JSON response",
        ));
    }

    let payload = match envelope.get("data") {
        Some(data) if data.is_object() => data.clone(),
        _ => envelope.clone(),
    };

    if http_status < 400 {
        return Ok(payload);
    }

    let code = string_field(&payload, "errorCode")
        .or_else(|| envelope.get("error").and_then(|e| string_field(e, "code")));
    let message = string_field(&payload, "errorMessage").or_else(|| {
        envelope
            .get("error")
            .and_then(|e| string_field(e, "message"))
    });

    Err(classify_status(http_status, code, message))
}

fn classify_status(status: u16, code: Option<String>, message: Option<String>) -> Error {
    match status {
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
        status => Error::api(
            status,
            message.unwrap_or_else(|| "Request rejected".to_string()),
        ),
    }
}

/// Validates the request and returns the idempotency key plus the exact body
/// bytes that get both signed and sent. Serializing once is the point: hashing a
/// second serialization would let key ordering or escaping drift between the
/// signature and the wire.
fn prepare_session_request(request: &CheckoutSessionRequest) -> Result<(String, String)> {
    if request.amount <= 0 {
        return Err(Error::validation(
            "amount must be a positive integer in MINOR units (e.g. 2500 for 25.00 EUR)",
        ));
    }
    if request.currency.trim().is_empty() {
        return Err(Error::validation("Missing required parameter: currency"));
    }
    if request.order_reference.trim().is_empty() {
        return Err(Error::validation(
            "Missing required parameter: order_reference",
        ));
    }
    if request.order_reference.len() > 100 {
        return Err(Error::validation(
            "order_reference must be at most 100 characters",
        ));
    }

    let idempotency_key = match &request.idempotency_key {
        Some(key) if key.trim().is_empty() => {
            return Err(Error::validation("idempotency_key must not be empty"))
        }
        Some(key) if key.len() > 100 => {
            return Err(Error::validation(
                "idempotency_key must be at most 100 characters",
            ))
        }
        Some(key) => key.clone(),
        None => new_idempotency_key(),
    };

    let body = serde_json::to_string(request).map_err(|error| {
        Error::validation(format!(
            "Request parameters are not JSON-encodable: {error}"
        ))
    })?;

    Ok((idempotency_key, body))
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

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Mints a random v4 UUID for use as an idempotency key. Keys are per-payment, so
/// a fresh one is generated for every call that does not supply its own.
///
/// The bytes come from the OS. If the OS random source cannot be read, the
/// fallback mixes the process's hash seed (itself OS-seeded), the current time in
/// nanoseconds, and the address of a stack local - unique in practice, and only
/// ever reached on a machine where /dev/urandom is unavailable.
fn new_idempotency_key() -> String {
    let mut bytes = [0u8; 16];
    if !fill_from_os(&mut bytes) {
        fill_from_fallback(&mut bytes);
    }

    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hexed = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hexed[0..8],
        &hexed[8..12],
        &hexed[12..16],
        &hexed[16..20],
        &hexed[20..32]
    )
}

fn fill_from_os(bytes: &mut [u8; 16]) -> bool {
    match std::fs::File::open("/dev/urandom") {
        Ok(mut file) => file.read_exact(bytes).is_ok(),
        Err(_) => false,
    }
}

fn fill_from_fallback(bytes: &mut [u8; 16]) {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0),
    );
    let marker = 0u8;
    hasher.write_usize(std::ptr::addr_of!(marker) as usize);
    let high = hasher.finish();

    hasher.write_u64(high);
    let low = hasher.finish();

    bytes[..8].copy_from_slice(&high.to_le_bytes());
    bytes[8..].copy_from_slice(&low.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_keys_are_unique_v4_uuids() {
        let first = new_idempotency_key();
        let second = new_idempotency_key();
        assert_ne!(first, second);
        assert!(is_uuid(&first), "{first} is not a lowercase UUID");
        assert_eq!(&first[14..15], "4");
    }

    #[test]
    fn uuid_check_rejects_near_misses() {
        assert!(is_uuid("00000000-0000-4000-8000-000000000001"));
        assert!(!is_uuid("00000000-0000-4000-8000-00000000000"));
        assert!(!is_uuid("00000000-0000-4000-8000-000000000001-x"));
        assert!(!is_uuid("0000000G-0000-4000-8000-000000000001"));
    }

    #[test]
    fn body_serializes_once_with_extra_fields_merged() {
        let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042")
            .extra("splitPayment", serde_json::json!(true));
        let (_, body) = prepare_session_request(&request).expect("valid request");
        assert_eq!(
            body,
            r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042","splitPayment":true}"#
        );
    }
}
