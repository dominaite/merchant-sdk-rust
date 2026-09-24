# dominaite

Server-side Rust client for the Dominaite merchant API. One call from your backend opens a
hosted checkout session; a two-line script tag renders the payment widget on your page. Card
details go straight from your customer's browser into the payment widget - they never touch
your server, which keeps your PCI scope minimal (SAQ A).

Rust 1.85 or newer, edition 2021. Five dependencies, all small and well-audited:
`hmac`, `sha2`, `hex` (RustCrypto), `serde` + `serde_json`, and `ureq` for HTTP. The client is
synchronous - no async runtime is pulled in.

## Install

Publishing to crates.io is a pending owner decision, the same as the other Dominaite SDKs went
through. A git dependency works today:

```toml
[dependencies]
dominaite = { git = "https://github.com/dominaite/merchant-sdk-rust", tag = "v0.1.0" }
```

To work on the SDK itself:

```sh
cd dominaite-rust-sdk
cargo test          # includes the offline signing and webhook vectors
cargo clippy -- -D warnings
```

## Credentials

You get two values from the Dominaite dashboard, **Website integration** tab, when you generate
an API key (shown once - store them like passwords):

- `dmk_...` - your API key id. Identifies you; not secret by itself.
- `dms_...` - your API secret. Server-side only: environment variable or a config file outside
  the web root. Never in a browser, never in git, never in logs.

Every request is signed with the secret (HMAC-SHA256) and timestamped. Keep your server clock
on NTP - signatures older than 5 minutes are rejected with `TIMESTAMP_OUT_OF_RANGE`.

If the key has an IP allowlist, calls from anywhere else fail with `IP_NOT_ALLOWED`. The
allowlist is managed on the same dashboard tab.

## Quickstart (zero to a signed session against dev)

Everything below is copy-paste. It assumes an empty directory and nothing installed.

```sh
cargo new my-checkout && cd my-checkout
cargo add --git https://github.com/dominaite/merchant-sdk-rust --tag v0.1.0 dominaite
```

Set your credentials and the environment you are pointing at:

```sh
export DOMINAITE_KEY_ID=dmk_...      # Website integration tab
export DOMINAITE_SECRET=dms_...      # shown once when you generated the key
# Dev: the payments function app, whose Azure Functions route prefix is /api.
# Confirm the host for your environment before the first call.
export DOMINAITE_BASE_URL=https://func-dom-gw-payments-dev-gwc-01.azurewebsites.net/api
# Production needs no DOMINAITE_BASE_URL - the SDK defaults to
# https://api.dominaite.com/payments
```

A dev key against production is a guaranteed `INVALID_API_KEY`: keys are issued per
environment.

`src/main.rs`:

```rust
use dominaite::{CheckoutSessionRequest, Client, Customer, Error, IdempotencyKey};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder(
        std::env::var("DOMINAITE_KEY_ID")?,
        std::env::var("DOMINAITE_SECRET")?,
    )
    // An unset variable is empty, and an empty value keeps the production default.
    .base_url(std::env::var("DOMINAITE_BASE_URL").unwrap_or_default())
    .build()?;

    // First live call: proves key, secret, signing and clock without creating anything.
    let ping = client.ping()?;
    println!("merchant {}, clock skew {}s", ping.merchant_id, ping.clock_skew_seconds);

    // The key belongs to the order, not to the request: a reload or the back button
    // replays this session instead of opening a second one, and a changed amount
    // gets a new key. The scope keeps your deployments apart (a short hash of your
    // shop's public URL works well).
    let key = IdempotencyKey::for_order("shop-a1b2c3d4", "order-1042", 2500, "EUR")?;

    let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key) // 2500 = 25.00 EUR
        .customer(
            // Pass everything you already know - prefilled fields are hidden from the
            // payer, so the checkout form stays short.
            Customer::new()
                .first_name("Ana")
                .last_name("Kirova")
                .email("ana@example.com"),
        )
        .language("bg")
        .theme("dark");

    match client.create_checkout_session(&request) {
        Ok(session) => {
            // Store session.transaction_id against your order, then hand cashier_key +
            // cashier_token to the page that renders the widget.
            println!("{} {}", session.cashier_key, session.cashier_token);
        }
        // Machine-readable: codes are listed below.
        Err(Error::Refusal { code, .. }) => println!("Payment unavailable: {code}"),
        // Network blip - safe to retry with the same idempotency key.
        Err(Error::Transport { .. }) => println!("Payment temporarily unavailable"),
        Err(error) => return Err(error.into()),
    }

    Ok(())
}
```

```sh
cargo run
```

Render the widget with the two cashier values:

```html
<div id="checkout">
  <script src="https://bp-checkout.dominaite.com/v2/launcher"
          data-cashier-key="CASHIER_KEY_FROM_SESSION"
          data-cashier-token="CASHIER_TOKEN_FROM_SESSION"></script>
</div>
```

The launcher renders the form where the script tag sits, so keep the script inside your
container. `cashier_key` and `cashier_token` are per-payment session values, not credentials -
but HTML-escape them when you template them into the page.

A runnable version of the above is in `examples/create_session.rs`, using the same three
environment variables:

```sh
cargo run --example create_session
```

### Then find out whether it got paid

Opening the session is half the integration. The widget runs in your customer's browser, so
your server does not learn the outcome from the call above - something has to tell it.

Register a webhook endpoint, verify every delivery with `verify_webhook`, and fulfil the order
when `payment.succeeded` arrives. See [Webhooks](#webhooks) for the endpoint setup, the
signature check and the delivery rules. If you cannot receive inbound requests yet, poll
`get_status` instead and move to webhooks when you can.

Either way, keep a reconciliation sweep. Webhooks are the fast path, not the guarantee.

So the whole integration is four pieces: the session call, the script tag, verified webhooks
for confirmation, and your domain bound to your checkout by Dominaite during onboarding.

## Verify your signing before your first live call

Run `cargo test` before you touch the live API. The SDK signs for you, but the recipe is pinned
by two offline known-answer vectors shared with the gateway and the dashboard, and the suite
reproduces both byte-for-byte. If either fails, nothing else matters - every live call will come
back `INVALID_SIGNATURE`.

`tests/webhooks.rs` pins the other direction the same way: the canonical cross-SDK webhook
vector, plus a tampered body, a wrong secret, a stale timestamp and a batch of malformed
headers. That vector is byte-identical across every Dominaite SDK, so a failure there means
your build disagrees with the gateway about the scheme itself.

`sign_request` is public so you can pin the recipe in your own suite, or debug an
`INVALID_SIGNATURE` without reading this crate's source:

```rust
let signature = dominaite::sign_request(dominaite::SignRequest {
    secret: "dms_...",
    timestamp: "1755302400",                                // unix SECONDS
    method: "POST",
    path: "/merchant-api/checkout/sessions",                 // path only, no host
    idempotency_key: "00000000-0000-4000-8000-000000000001", // "" for GET
    body: r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042"}"#, // "" for GET
});
// "8f5fba0b29a8eea81b76a0e6d7119e79ec68f586910f77713b045652e5ce9b74"
```

The signed payload is five lines:
`"{timestamp}\n{METHOD}\n{path}\n{idempotency_key}\n{sha256hex(body)}"`, signed as lowercase hex
HMAC-SHA256 with your secret, UTF-8 throughout. Two things to get right:

- GET signs an EMPTY idempotency key and an EMPTY body, and sends no `Idempotency-Key` header.
  The payload is still five lines.
- The signed path NEVER includes the base URL's own prefix. On dev you POST to
  `.../api/merchant-api/checkout/sessions` but you sign `/merchant-api/checkout/sessions`.

## Ping before your first mint

```rust
let ping = client.ping()?;
```

`ping` is a GET that creates nothing and reads nothing. It returns `pong`, your `merchant_id`,
`server_time`, `server_unix_time` and `clock_skew_seconds` (server time minus your
`X-Timestamp`). If the absolute skew creeps toward 300, fix NTP now - requests start failing at
300.

Only after ping returns should you mint your first session. A 401 there means key id, secret, or
signing; a 503 means retry later. Never both at once.

## Client options

`Client::new(key_id, secret)` gives you production with a 45s timeout.
`Client::builder(key_id, secret)` takes more:

| Method | What |
|---|---|
| `.base_url(url)` | Point at a non-production environment. Empty and whitespace-only values are ignored, so an unset env var still gives you production. |
| `.timeout(d)` | Per-request timeout. Defaults to 45s (serverless cold starts can take 10+s). |
| `.agent(a)` | Your own `ureq::Agent`: proxy-aware transport, custom TLS, a test double. Replaces `.timeout(d)`. Redirects stay off regardless of how the agent is configured: the SDK forces `max_redirects(0)` on every request, so signed headers never cross to another host. |
| `.user_agent(s)` | Appends your identifier to the SDK's User-Agent, which helps when support reads the access logs. |

`Client` is `Clone` and cheap to clone; one per process is the normal shape.

## Amounts are minor units

`amount` is always an integer in the currency's minor unit: `2500` is 25.00 EUR. The field is an
`i64`, so a float will not compile; non-positive values are rejected before anything reaches the
network. The amount is locked server-side - what you pass here is what gets charged, and nothing
in the browser can change it. Compute it from your own catalog, never from the request body your
page sent you.

The minor unit depends on the currency (ISO 4217): EUR, USD, GBP, BGN and most others have two
decimals, JPY, KRW and ISK none, BHD, KWD, OMR, JOD and TND three. `to_minor_units` converts a
decimal string for you:

```rust
use dominaite::to_minor_units;

let amount = to_minor_units("0.30", "EUR")?;  // 30
let amount = to_minor_units("500", "JPY")?;   // 500
let amount = to_minor_units("1.250", "KWD")?; // 1250
```

It parses the string and never goes through a float, so `"0.30"` is always 30 and never 29.
Feed it the decimal your catalog or database already holds, not the result of float
arithmetic. More decimal places than the currency allows (`"0.305"` EUR), signs, thousands
separators and unknown currencies are `Error::Validation`; do your own rounding first.
`currency_exponent` answers the exponent alone.

## Retries and double-charges

Every `create_checkout_session` and `charge_payment_method` call takes an idempotency key, and
the SDK never makes one up: `CheckoutSessionRequest::new` and `ChargeRequest::new` will not
compile without an `IdempotencyKey`. Retrying with the same key never opens a second payment -
on a timeout, retry with the same key rather than generating a new one.

Derive the key from the order with `IdempotencyKey::for_order(scope, order_id, amount_minor,
currency)`, which builds `{scope}-{orderId}-{amountMinor}-{CURRENCY}`:

- Same order, same amount: same key. A page reload or the back button replays the session
  already open for the order instead of opening a second one.
- Changed amount or currency: new key. A re-priced order never reuses the session opened for
  the old total (that replay would be refused with `IDEMPOTENCY_KEY_REUSED`).
- `scope` keeps deployments that share one merchant apart. A staging and a production shop
  that both number orders from 1001 would otherwise collide.

If you already derive keys your own way, wrap them with `IdempotencyKey::new(key)`. Both
constructors return `Error::Validation` for an empty key or one over 100 characters, before
anything is sent.

A replayed key does not hand back the original session. While the first attempt is live (or
completed, or judged failed), the API answers HTTP 200 with `success: false` and a replay code,
which arrives as `Error::Refusal`
(`DUPLICATE_REQUEST`, `ALREADY_PROCESSED`, `PRIOR_ATTEMPT_FAILED`, `IDEMPOTENCY_KEY_REUSED`) - the
first session's `cashierKey` and `cashierToken` are not returned again. When the refusal names a
transaction id, read it back with `get_status` to find out what the earlier attempt did; see
[Recovering from a replay refusal](#recovering-from-a-replay-refusal).

`create_checkout_session_with_retry` does that for you: it sends the request's key on every
attempt, retrying only `Error::Transport` (network failures and 5xx, including
`MERCHANT_API_UNAVAILABLE`). Refusals and authentication failures are not retried - they will not
change.

```rust
use dominaite::RetryOptions;

let session = client.create_checkout_session_with_retry(
    &request,
    RetryOptions::default(), // 3 attempts, 500ms base delay, doubling
)?;
```

## Sessions expire

A session is valid for about 2 hours. If the payer comes back later, create a new one. Before
re-rendering the widget for a stored session, read the status first: a completed session's
widget shows "session is closed or expired", which reads as an error to someone who just paid.

## Stored payment methods (recurring)

A session can ask the payer to save their card for later. Set `save_card(true)` on the request;
nothing else about the session changes, and a request without it sends the exact same bytes as
before (the flag is omitted, not sent as `false`).

```rust
let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key).save_card(true);
let session = client.create_checkout_session(&request)?;
```

Once that session is paid, `get_status` carries a `stored_payment_method`: an id, the brand,
the last four digits, the expiry and a status (`active`, `revoked` or `expired`). Persist the id
against your customer. The full card number never reaches the SDK, and the provider token
behind the id never leaves the gateway. `brand`, `last4` and the expiry are `Option`s: the
gateway omits them when the provider did not report them. This is not the gateway's
`paymentMethod` field (the string category of how the payer paid), which stays on `raw`.

```rust
let status = client.get_status(&session.transaction_id)?;
if let Some(method) = &status.stored_payment_method {
    if method.is_chargeable() {
        store_for_customer(customer_id, &method.id); // pm_...
    }
}
```

Charge the stored card later, off-session, with `charge_payment_method`. The call takes the
same amount, currency and order reference as a session, and an idempotency key that is required
and signed exactly like `create_checkout_session`. Derive it from what you are billing (the
subscription and its period), so a retried charge carries the same one.

```rust
use dominaite::{charge_error_code, charge_status, decline_class, ChargeRequest, Error, IdempotencyKey};

let key = IdempotencyKey::new("sub-8817-2026-10")?;
match client.charge_payment_method(
    &method_id,
    &ChargeRequest::new(2500, "EUR", "order-1043", key).description("Monthly plan"),
) {
    Ok(charge) => match charge.status.as_str() {
        charge_status::SUCCEEDED => mark_paid(&charge.transaction_id),
        charge_status::PENDING => poll_later(&charge.transaction_id), // not terminal
        charge_status::CANCELLED => nothing_moved(),
        _ => match charge.decline_class.as_deref() {
            Some(decline_class::HARD) => stop_charging(&method_id),      // never retry
            Some(decline_class::SOFT_FUNDS) => retry_in_a_few_days(),
            Some(decline_class::SOFT_SCA_REQUIRED) => bring_the_payer_back(), // needs a session
            _ => retry_later(),                                              // soft_other
        },
    },
    Err(Error::Charge { code, transaction_id, .. }) => match code.as_str() {
        // The provider gave no verdict: the charge MAY have happened. Poll the
        // transaction (or wait for the webhook); never retry under a new key.
        charge_error_code::CHARGE_OUTCOME_UNKNOWN => poll_later(&transaction_id.unwrap()),
        charge_error_code::PAYMENT_METHOD_NOT_ACTIVE => ask_for_another_card(),
        // Nothing was charged; retry later with the SAME key.
        charge_error_code::DUPLICATE_REQUEST
        | charge_error_code::PAYMENT_METHOD_CHARGES_DISABLED
        | charge_error_code::PAYMENT_PROCESSING_UNAVAILABLE => retry_later(),
        _ => give_up(), // CHARGE_FAILED, IDEMPOTENCY_KEY_REUSED
    },
    Err(other) => return Err(other.into()),
}
```

`charge_payment_method` returns `Ok` for HTTP 201 and for HTTP 402 alike. A decline is a result,
not an error: the 402 charge has `status: "failed"` and a `decline_class` (`hard`, `soft_funds`,
`soft_sca_required` or `soft_other`) plus the provider's `decline_code`. `is_paid()` and
`is_terminal()` read the same way they do on a session status; `pending` is the one status
that is not terminal.

`Error::Charge` is the gateway answering with an error code instead of a charge: HTTP 409
(`PAYMENT_METHOD_NOT_ACTIVE`, `DUPLICATE_REQUEST`), 422 (`IDEMPOTENCY_KEY_REUSED`), 502
(`CHARGE_OUTCOME_UNKNOWN`, `CHARGE_FAILED`) or 503 (`PAYMENT_METHOD_CHARGES_DISABLED`,
`PAYMENT_PROCESSING_UNAVAILABLE`). The variant keeps the status, the code, the message, and the
charge row when the gateway attached one (always for `CHARGE_OUTCOME_UNKNOWN`, whose
`transaction_id` is what you poll). A 404 is a method id that is not yours and stays the plain
`Error::Api` with code `PAYMENT_METHOD_NOT_FOUND`; a 5xx without a gateway code (an HTML page
from a proxy) stays `Error::Transport`.

`revoke_payment_method` drops the card: the gateway deletes the saved credential at the provider
and marks the method `revoked`, and any later charge on it is refused with
`PAYMENT_METHOD_NOT_ACTIVE`. It is a signed `DELETE` with an empty key and an empty body (the
same recipe as GET) and resolves on HTTP 204, again on an already revoked method. When the
gateway refuses, nothing changed and you get `Error::Revoke` with the code: `MERCHANT_API_UNAVAILABLE`
(503, retry later) or `UPSTREAM_CONTRACT_ERROR` (502, contact support with the id). A 404 is
the plain `Error::Api` with code `VALIDATION_ERROR`.

```rust
use dominaite::{revoke_error_code, Error};

match client.revoke_payment_method(&method_id) {
    Ok(()) => forget_for_customer(customer_id),
    Err(Error::Revoke { code, .. }) if code == revoke_error_code::MERCHANT_API_UNAVAILABLE => {
        retry_later()
    }
    Err(other) => return Err(other.into()),
}
```

Both routes are pinned by known-answer vectors in `tests/vectors.rs` next to the session ones,
shared byte-for-byte with the gateway: the charge vector signs
`POST /merchant-api/payment-methods/pm_0123456789abcdef0123456789abcdef/charges` with key
`00000000-0000-4000-8000-000000000003` and body
`{"amount":2500,"currency":"EUR","orderReference":"order-1043"}`, the revoke vector signs the
`DELETE` with nothing else.

## Webhooks

Webhooks are how you find out a payment succeeded without asking. Point an endpoint at your
server on the dashboard's **Webhooks** tab, pick the events you care about, and store the
`whsec_...` secret it shows you - it is shown exactly once, and regenerating it kills the old
one.

**Verify the signature before you parse the body.** An unverified webhook is an
unauthenticated stranger POSTing JSON at your server.

```rust
use dominaite::{verify_webhook, WebhookError, DEFAULT_TOLERANCE_SECS};

// `body` must be the RAW request body, byte for byte as received.
match verify_webhook(body, signature_header, &secret, DEFAULT_TOLERANCE_SECS, None) {
    Ok(()) => {
        let event: serde_json::Value = serde_json::from_str(body)?;
        // Dedupe on event["id"], enqueue the work, then answer 2xx.
    }
    Err(WebhookError::TimestampOutOfTolerance { .. }) => { /* replay, or your clock drifted */ }
    Err(_) => { /* wrong secret, or the body was modified in flight */ }
}
```

The arguments are `(payload, signature_header, secret, tolerance_secs, now)`. `now` is
`Option<u64>` unix seconds for tests and pinned vectors; pass `None` in a real handler to read
the system clock. The MAC comparison is constant-time, and it runs before the timestamp check
so an unsigned request learns nothing about your tolerance window.

The signature arrives in `X-Webhook-Signature` as `t={digits},v1={64 lowercase hex}`: an
HMAC-SHA256 over `"{t}.{raw_body}"` keyed with the UTF-8 bytes of your `whsec_` secret. The
default tolerance is 300 seconds, which matches the server.

The header grammar is closed, and anything outside it is a `MalformedSignature`: no
whitespace anywhere, exactly one `t` and one `v1` (a repeat rejects the header even when one
candidate carries a valid MAC), an element without `=` rejects, `t` is one or more raw ASCII
digits fed verbatim into the signed string, and `v1` is exactly 64 lowercase hex characters.
Unknown keys are ignored so a future `v2` can roll out alongside `v1`.

Getting the raw body is the part frameworks get wrong. If your handler hands you a parsed
struct and you re-serialize it to verify, key order or whitespace will differ and every
delivery will fail as `SignatureMismatch`. Read the bytes before any JSON layer touches them.

### The envelope

Flat JSON, no `success` wrapper - do not branch on a `success` field, there isn't one.

```json
{
  "id": "<delivery id - your dedupe key>",
  "type": "payment.succeeded",
  "createdAt": "<ISO 8601 UTC instant of the transition>",
  "data": {
    "transactionId": "...",
    "status": "succeeded",
    "previousStatus": "pending",
    "kind": "sale",
    "amount": 8440,
    "grossAmount": 8701,
    "surchargeAmount": 261,
    "currency": "EUR",
    "originalTransactionId": null,
    "idempotencyKey": "order-123"
  }
}
```

Amounts are minor units. On `payment.*` events `amount` is what you are PAID (base), while
`grossAmount` is the card movement; on `payment.refunded` the `amount` is what went back to the
customer. `surchargeAmount`, `previousStatus`, `kind` and `originalTransactionId` are nullable.

### Events

`payment.succeeded`, `payment.failed`, `payment.requires_capture`, `payment.cancelled`,
`payment.abandoned`, `payment.refunded`, `payment.disputed`. That is the whole set, exact case;
registering anything else is rejected.

`payment.succeeded` is the only signal that means money is in hand. `requires_capture` includes
approved pre-auth holds, `cancelled` is a pre-completion void only, `abandoned` is the sweep's
verdict on a checkout that was never paid, and `refunded` fires once per refund from the refund
ledger row rather than from the parent flipping status. `pending` and `processing` are not
webhooked at all - poll session status if you want in-flight UX.

### Delivery

Delivery is **at-least-once**, so the same event can arrive twice and you must dedupe on `id`.
Respond 2xx quickly and queue the work; doing it inline is how you end up timing out and
collecting retries you did not want.

Failed deliveries are retried up to your endpoint's `RetryCount` (default 3, max 10, 0 disables)
spaced 1m / 5m / 30m / 2h / 12h. An endpoint whose initial attempt and every configured retry
fail consecutively is auto-disabled; a later successful delivery re-enables it. Disabling an
endpoint yourself in the dashboard is never overridden. You get at most 25 active endpoints.

### Reconciliation is still mandatory

Webhooks complement your reconciliation sweep, they do not replace it. There are real loss
windows - there is no publish outbox, and chains parked on a disabled endpoint stay parked - so
keep a periodic sweep that reads status for orders you believe are unpaid and settles the
difference. Treat webhooks as the fast path and the sweep as the source of truth.

## Status polling (fallback)

Use this when you cannot receive webhooks - local development with no public URL, or a network
that will not accept inbound requests - and as the read side of the reconciliation sweep above.

```rust
let status = client.get_status(&session.transaction_id)?;
if status.is_paid() { /* fulfil the order */ }
```

`status.status` is one of `pending`, `processing`, `succeeded`, `failed`, `refunded`,
`partially_refunded`, `cancelled`, `disputed`, `requires_capture`, `abandoned` (the
`dominaite::status` constants). **`succeeded` is the only value that means the customer paid** -
that is what `is_paid()` answers. `is_terminal()` tells you whether to stop polling, and reports
a status it does not recognise as NOT terminal, so a value the API adds later makes you keep
polling instead of closing an open order. Keep polling on `pending`, `processing`,
`requires_capture` and `disputed` - none of them is terminal (a dispute can still go either
way).

`requires_capture` is **not** "unpaid": the payer has already paid and the funds are held
awaiting capture, which is why `is_paid()` (settled) and `is_terminal()` (finished) both answer
false for it. Never treat it as an abandoned order.

Call this from your server, never from the browser, and poll after the payer returns to you or
on your order timeout - not in a tight loop. The platform allows 60 requests per minute per API
key and 120 per minute per IP; over that you get `Error::RateLimited`.

Every response type also carries `raw` (a `serde_json::Value`) with the unparsed payload, for
fields the structs do not model yet.

## Errors

Every call returns `Result<T, dominaite::Error>`. `Error` is an enum implementing
`std::error::Error`; match on the variant, and read `error.code()` for the machine-readable
string where there is one.

| Variant | When | What to do |
|---|---|---|
| `Error::Refusal { code, .. }` | HTTP 200 with `success: false`. | Branch on `code`. Do not blind-retry. |
| `Error::Auth { code, .. }` | 401/403. `code` is `INVALID_API_KEY`, `INVALID_SIGNATURE`, `TIMESTAMP_OUT_OF_RANGE`, or `IP_NOT_ALLOWED`. | Fix the key id, secret, server clock, or allowlist. Never retry-loop. |
| `Error::Transport { .. }` | Network failure, timeout, or a 5xx without a gateway code, including one whose body is an HTML error page from a proxy. The cause is reachable through `source()`. | Retry with the **same** idempotency key. `error.is_retryable()` is true only here. |
| `Error::RateLimited { retry_after_seconds }` | 429. The platform allows 60 requests per minute per API key and 120 per minute per IP. | Wait `retry_after_seconds` (or back off yourself when it is `None`), then send the request again with the **same** idempotency key. Never auto-retried: `is_retryable()` is false. |
| `Error::Charge { status, code, charge, transaction_id, .. }` | `charge_payment_method` got an error code instead of a charge: 409, 422, 502 or 503. | Branch on `code` (see [Stored payment methods](#stored-payment-methods-recurring)). `CHARGE_OUTCOME_UNKNOWN` carries the `transaction_id` to poll; never retry it under a new key. |
| `Error::Revoke { status, code, .. }` | `revoke_payment_method` was refused: 502 `UPSTREAM_CONTRACT_ERROR` or 503 `MERCHANT_API_UNAVAILABLE`. Nothing changed. | Retry later on 503; contact support on 502. |
| `Error::Api { status, code, .. }` | Any other rejecting or unexpected response. `code` carries the API's machine-readable reason when it sent one, e.g. `IDEMPOTENCY_KEY_REQUIRED` on a 400, `STOREFRONT_NOT_WHITELISTED` on a 409, `PAYMENT_METHOD_NOT_FOUND` on a charge 404. | Inspect `status` and `code`. A 404 from `get_status` is an unknown transaction id. |
| `Error::Validation { .. }` | Bad arguments (non-positive amount, missing field, malformed key id). | Fix the call; nothing was sent. |

Refusal codes on `Error::Refusal`:

- `PAYMENT_PROCESSING_UNAVAILABLE` - card payments are off right now; retry later.
- `DUPLICATE_REQUEST` - a session for this idempotency key is already open, or expired within
  the last few minutes; re-POST the same key shortly, never a fresh one.
- `ALREADY_PROCESSED` - this idempotency key's payment already completed.
- `PRIOR_ATTEMPT_FAILED` - the earlier attempt with this key failed; use a fresh key. The
  order-derived key is spent, so derive the next one from a new attempt id (for example
  `order-1042-2` as the `order_id`).
- `IDEMPOTENCY_KEY_REUSED` - same key sent with a different body; use a fresh key.

All five arrive as HTTP 200 with `success: false`, not as an HTTP error status.

Every refusal code is a constant in `dominaite::session_error_code` (`REFUSALS` lists them).

### Storefront errors

A merchant with more than one website has a storefront (an online location) per site. When the
storefront cannot take payments, session creation fails with a real HTTP status rather than a
200 refusal, so these arrive as `Error::Api` with the code set:

| Code | Status | Meaning |
|---|---|---|
| `STOREFRONT_NOT_WHITELISTED` | 409 | The site's domain is not whitelisted with the payment provider yet. |
| `STOREFRONT_INACTIVE` | 409 | The storefront was deactivated or deleted. |
| `STOREFRONT_MISMATCH` | 400 | The API key is bound to one storefront and the request named another. |

None of them fixes itself on a retry, and none is a code bug on your side: the fix is in the
Dominaite backoffice or onboarding. Show the customer "payments are unavailable" and alert your
team.

```rust
use dominaite::session_error_code;

match client.create_checkout_session(&request) {
    Err(error) if error.code() == Some(session_error_code::STOREFRONT_NOT_WHITELISTED) => {
        alert_ops("storefront not whitelisted yet");
    }
    other => { /* ... */ }
}
```

### Recovering from a replay refusal

When your idempotency key collides with an earlier attempt, the refusal names the transaction it
collided with, so you can reconcile instead of minting a second payment:

```rust
match client.create_checkout_session(&request) {
    Err(Error::Refusal { transaction_id: Some(id), .. }) => {
        let status = client.get_status(&id)?;
        // Now you know what the earlier attempt actually did.
    }
    other => { /* ... */ }
}
```

`transaction_id` is `None` when the API did not name one (a concurrent-race `DUPLICATE_REQUEST`
knows the key is taken but not yet by which row), so match on `Some` rather than unwrapping.

One replay is not a refusal at all. A session that expired unpaid is superseded: from a few
minutes past its expiry, calling `create_checkout_session` again with the same key returns an
ordinary `Ok` with a fresh session (new `transaction_id`, same key), so a customer who comes
back late just pays. Keep the order-derived key for the life of the order to keep that path
open. The band is not endless - once the platform has independently closed the attempt (about
an hour past expiry), the replay answers `PRIOR_ATTEMPT_FAILED` and the key is spent; reconcile
and use a fresh key.

## The three identifiers

- `transaction_id` - Dominaite's payment id. Store it, poll status with it.
- `order_reference` - your own id, echoed back. This is what you search for in your dashboard,
  so put your order or cart id there.
- `order_id` (`dom_...`) - the provider-facing correlation id. You never need it.
