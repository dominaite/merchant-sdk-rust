# Changelog

## 1.0.0

Breaking. All six Dominaite SDKs move to 1.0.0 together with the same changes.

- The idempotency key is required. `CheckoutSessionRequest::new` and `ChargeRequest::new` take
  an `IdempotencyKey` as a fourth argument, and the SDK no longer generates a random key when
  you leave it out. The `.idempotency_key(...)` builder methods are gone.
- `IdempotencyKey::for_order(scope, order_id, amount_minor, currency)` builds the recommended
  key, `{scope}-{orderId}-{amountMinor}-{CURRENCY}`. The same order at the same amount replays
  its session (reload and back button safe); a changed amount gets a new key.
  `IdempotencyKey::new(key)` wraps a key you derive yourself. Keys are 1 to 100 characters of
  visible ASCII (`!` through `~`).
- `session_error_code`: the five refusal codes plus `STOREFRONT_NOT_WHITELISTED` (409),
  `STOREFRONT_INACTIVE` (409) and `STOREFRONT_MISMATCH` (400). The storefront codes arrive as
  `Error::Api` with the code set.
- `to_minor_units(amount, currency)` and `currency_exponent(currency)`: decimal string to
  integer minor units by the gateway's exponent, without floats. HUF is 0 decimals like the
  gateway (ISO 4217 says 2). ISK, KRW, OMR, JOD and TND are refused as not supported. Extra
  decimals are rejected even when they are zeros.
- `create_checkout_session_with_retry` also retries the HTTP 200 `PAYMENT_PROCESSING_UNAVAILABLE`
  refusal with the same key, not only its 503 form.
- `CheckoutStatus::is_terminal()` is now false for `disputed`, which can still resolve either
  way.
- Docs: a clean replay of an open session returns the original session, not a refusal.

### Migrating from 0.x

Pass a key to every request constructor. For a checkout, derive it from the order:

```rust
let key = IdempotencyKey::for_order("shop-a1b2c3d4", "order-1042", 2500, "EUR")?;
let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key);
```

For a stored-card charge, derive it from what you bill, e.g.
`IdempotencyKey::new("sub-8817-2026-10")?`. A key you built yourself with spaces or non-ASCII
characters is now refused. If you relied on `is_terminal()` to stop polling a `disputed`
payment, keep polling it now.
