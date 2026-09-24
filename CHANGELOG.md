# Changelog

## 0.4.0

Breaking. This crate is still 0.x, so under Cargo's semver rules the minor bump is the breaking
one: `dominaite = "0.3"` will not pick this up on its own.

- The idempotency key is required. `CheckoutSessionRequest::new` and `ChargeRequest::new` take
  an `IdempotencyKey` as a fourth argument, and the SDK no longer generates a random key when
  you leave it out. The `.idempotency_key(...)` builder methods are gone.
- `IdempotencyKey::for_order(scope, order_id, amount_minor, currency)` builds the recommended
  key, `{scope}-{orderId}-{amountMinor}-{CURRENCY}`. The same order at the same amount replays
  its session (reload and back button safe); a changed amount gets a new key.
  `IdempotencyKey::new(key)` wraps a key you derive yourself.
- `session_error_code`: the five refusal codes plus `STOREFRONT_NOT_WHITELISTED` (409),
  `STOREFRONT_INACTIVE` (409) and `STOREFRONT_MISMATCH` (400). The storefront codes arrive as
  `Error::Api` with the code set.
- `to_minor_units(amount, currency)` and `currency_exponent(currency)`: decimal string to
  integer minor units by ISO 4217 exponent, without floats.
- `CheckoutStatus::is_terminal()` is now false for `disputed`, which can still resolve either
  way.

### Migrating from 0.3

Pass a key to every request constructor. For a checkout, derive it from the order:

```rust
let key = IdempotencyKey::for_order("shop-a1b2c3d4", "order-1042", 2500, "EUR")?;
let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key);
```

For a stored-card charge, derive it from what you bill, e.g.
`IdempotencyKey::new("sub-8817-2026-10")?`. If you relied on `is_terminal()` to stop polling a
`disputed` payment, keep polling it now.
