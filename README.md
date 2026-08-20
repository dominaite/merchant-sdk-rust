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
cargo test          # includes both offline signing vectors
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
use dominaite::{CheckoutSessionRequest, Client, Customer, Error};

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

    let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042") // 2500 = 25.00 EUR
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

That's the whole integration: the session call, the script tag, and your domain bound to your
checkout by Dominaite during onboarding.

A runnable version of the above is in `examples/create_session.rs`, using the same three
environment variables:

```sh
cargo run --example create_session
```

## Verify your signing before your first live call

Run `cargo test` before you touch the live API. The SDK signs for you, but the recipe is pinned
by two offline known-answer vectors shared with the gateway and the dashboard, and the suite
reproduces both byte-for-byte. If either fails, nothing else matters - every live call will come
back `INVALID_SIGNATURE`.

`sign_request` is public so you can pin the recipe in your own suite, or debug an
`INVALID_SIGNATURE` without reading this crate's source:

```rust
let signature = dominaite::sign_request(dominaite::SignRequest {
    secret: "dms_...",
    timestamp: "1755302400",                                // unix SECONDS
    method: "POST",
    path: "/merchant-api/bridgerpay/checkout/sessions",      // path only, no host
    idempotency_key: "00000000-0000-4000-8000-000000000001", // "" for GET
    body: r#"{"amount":2500,"currency":"EUR","orderReference":"order-1042"}"#, // "" for GET
});
// "95759958a0a0a9bd3e6e37101c01e8e7fee1166406e4ac2ff488764f5f742cbf"
```

The signed payload is five lines:
`"{timestamp}\n{METHOD}\n{path}\n{idempotency_key}\n{sha256hex(body)}"`, signed as lowercase hex
HMAC-SHA256 with your secret, UTF-8 throughout. Two things to get right:

- GET signs an EMPTY idempotency key and an EMPTY body, and sends no `Idempotency-Key` header.
  The payload is still five lines.
- The signed path NEVER includes the base URL's own prefix. On dev you POST to
  `.../api/merchant-api/bridgerpay/checkout/sessions` but you sign
  `/merchant-api/bridgerpay/checkout/sessions`.

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
| `.agent(a)` | Your own `ureq::Agent`: proxy-aware transport, custom TLS, a test double. Replaces `.timeout(d)`. |
| `.user_agent(s)` | Appends your identifier to the SDK's User-Agent, which helps when support reads the access logs. |

`Client` is `Clone` and cheap to clone; one per process is the normal shape.

## Amounts are minor units

`amount` is always an integer in the currency's minor unit: `2500` is 25.00 EUR. The field is an
`i64`, so a float will not compile; non-positive values are rejected before anything reaches the
network. The amount is locked server-side - what you pass here is what gets charged, and nothing
in the browser can change it. Compute it from your own catalog, never from the request body your
page sent you.

## Retries and double-charges

Every `create_checkout_session` call carries an idempotency key (auto-generated, or set your own
with `.idempotency_key(...)`). Retrying with the same key never opens a second payment - on a
timeout, retry with the same key rather than generating a new one.

`create_checkout_session_with_retry` does that for you: it pins one key up front and reuses it
across attempts, retrying only `Error::Transport` (network failures and 5xx, including
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

## Status polling

```rust
let status = client.get_status(&session.transaction_id)?;
if status.is_paid() { /* fulfil the order */ }
```

`status.status` is one of `pending`, `processing`, `succeeded`, `failed`, `refunded`,
`partially_refunded`, `cancelled`, `disputed`, `requires_capture`, `abandoned` (the
`dominaite::status` constants). **`succeeded` is the only value that means the customer paid** -
that is what `is_paid()` answers. `is_terminal()` tells you whether to stop polling, and reports
a status it does not recognise as NOT terminal, so a value the API adds later makes you keep
polling instead of closing an open order.

There is no merchant webhook yet: confirm payment by calling this from your server until it
reads terminal. Poll after the payer returns to you, or on your order timeout - not in a tight
loop, the endpoint is rate limited per key.

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
| `Error::Transport { .. }` | Network failure, timeout, or 5xx (`MERCHANT_API_UNAVAILABLE`). The cause is reachable through `source()`. | Retry with the **same** idempotency key. `error.is_retryable()` is true only here. |
| `Error::Api { status, .. }` | Any other rejecting or unexpected response. | Inspect `status`. A 422 means an idempotency key was replayed with a different body - use a fresh key. A 404 from `get_status` is an unknown transaction id. |
| `Error::Validation { .. }` | Bad arguments (non-positive amount, missing field, malformed key id). | Fix the call; nothing was sent. |

Refusal codes on `Error::Refusal`:

- `PAYMENT_PROCESSING_UNAVAILABLE` - card payments are off right now; retry later.
- `DUPLICATE_REQUEST` - a session for this idempotency key is already open.
- `ALREADY_PROCESSED` - this idempotency key's payment already completed.
- `PRIOR_ATTEMPT_FAILED` - the earlier attempt with this key failed; use a fresh key.
- `IDEMPOTENCY_KEY_REUSED` - same key sent with a different body; use a fresh key.

All five arrive as HTTP 200 with `success: false`, not as an HTTP error status.

## The three identifiers

- `transaction_id` - Dominaite's payment id. Store it, poll status with it.
- `order_reference` - your own id, echoed back. This is what you search for in your dashboard,
  so put your order or cart id there.
- `order_id` (`dom_...`) - the provider-facing correlation id. You never need it.
