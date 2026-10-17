# Changelog

## 0.6.0

### Breaking

- The base URL is required. The client no longer falls back to production when you leave it
  out, so an unset environment variable cannot send sandbox credentials to production.
  Construction fails with `Error::Validation` when the base URL is missing, empty or whitespace.
  Pass `SANDBOX_BASE_URL` (`https://api.staging.dominaite.com/payments`) or
  `PRODUCTION_BASE_URL` (`https://api.dominaite.com/payments`), or your own value.
- `DEFAULT_BASE_URL` is removed. Use `PRODUCTION_BASE_URL`.
- `Client::new` takes the base URL: `Client::new(key_id, secret, base_url)`. With
  `Client::builder`, `.base_url(url)` is required before `.build()`.

### Added

- The User-Agent is `dominaite-rust/<version> (rust <rustc version>)`, then a space and your
  `.user_agent(s)` suffix. A suffix with a control character fails `.build()` with
  `Error::Validation`.
- `ClientBuilder::on_warning(f)`: a closure for the SDK's warnings. It gets the
  `Dominaite-Sdk-Notice` header (once per distinct value, on every status), a retry in
  `create_checkout_session_with_retry`, and a ping clock skew over 60 seconds. Without it,
  warnings go nowhere.
- `ClientBuilder::on_debug(f)`: the debug log, off by default. One line per request with the
  method, the path, the status, the time in milliseconds, the `Dominaite-Request-Id` and the
  idempotency key. No line holds the secret, the signature, a cashier token, a body or a query
  string.
- `Client::user_agent()` returns the User-Agent the client sends.

## 0.5.2

### Added

- Billing address on the payer: `Customer::address`, `city`, `postal_code` and `state`, all
  optional, with builder methods of the same names. Address is one line, at most 200
  characters; city at most 100; postal code at most 20. State is the state or province code, at
  most 50 characters, and only used for the United States, Canada and Australia. The widget does
  not ask the payer for a field that you send. A value that is too long fails with 400
  `VALIDATION_ERROR`, code `MAX_LENGTH`.

### Changed

- `Customer` has four new public fields, so a struct literal without `..Default::default()` no
  longer compiles. `Customer::new()` with the builder methods is unchanged.

## 0.5.1

### Added

- `Transaction` has `fee`, `net_amount`, `settled_at` and `payout_id`. All four are `None` until
  the acquirer settles the payment. `fee` and `net_amount` are in minor units, like `amount`.
  `net_amount` is `amount` minus the fee of this payment only.
- `TransactionListParams::settled(from, to)` lists only the payments settled in that window.
  Both ends are dates (`2026-10-13`), at most 31 days apart. They go out as `settledFrom` and
  `settledTo` in the query string. The signed path still has no query string.

## 0.5.0

### Added

- Verify-only sessions: `CheckoutSessionRequest::verify_only(true)` saves the payer's card with
  no payment. Send an amount of 0; any other amount fails with `Error::Validation` before
  anything is sent. The API refuses the session with 403 `VERIFY_ONLY_NOT_ENABLED` until we
  switch it on for your account. The session sends `verification.succeeded` or
  `verification.failed`, never a `payment.*` event.
- Subscription sessions: `CheckoutSessionRequest` models `agreement_id` and `consent`
  (`Consent`), and also `location_id`.
- Subscriptions: `create_plan`, `list_plans`, `get_plan`, `update_plan` (PATCH) and
  `delete_plan`, and `create_agreement`, `get_agreement` and `cancel_agreement`. `create_plan`
  and `create_agreement` need an idempotency key. `update_plan` sends none, because the API does
  not replay a plan update. New types: `Plan`, `PlanRequest`, `PlanUpdate`, `Agreement`,
  `AgreementRequest` and `ConsentRecord`, and the `plan_status`, `interval_unit`,
  `agreement_status` and `past_due_reason` constants.
- `list_refunds(transaction_id)`: the refunds of one payment that you created through the API,
  and the `refunded_amount` and `refundable_amount` totals (`RefundList`).
- Webhook endpoints from your code, with a `webhooks:manage` key: `create_webhook_endpoint`,
  `list_webhook_endpoints` and `delete_webhook_endpoint`. The create answers the `whsec_` secret
  one time only; a repeat with the same key answers `secret: None`. The `Debug` output of
  `WebhookEndpoint` never shows the secret. New: `WebhookEndpointRequest`, and the
  `webhook_endpoint_status` and `event_type` constants (`event_type::PAYMENT`, `RECURRING` and
  `VERIFICATION`).
- `list_transactions(&TransactionListParams)`, with a `transactions:read` key: one page of your
  payments, oldest first, and the `next_cursor` of the next page. The SDK signs the path without
  the query. New: `Transaction`, `TransactionPage`, `TransactionListParams` and the
  `transaction_type` constants.
- `get_integration_contract`: the contract that the API publishes about itself.
  `IntegrationContract` models `version`, `statuses` and `webhook_event_catalog`; `raw` keeps
  the other fields as sent.
- Response fields: `verify_only`, `decline_class`, `is_recurring`, `agreement`
  (`AgreementSummary`) and `sequence` on `CheckoutStatus`; `verify_only` and `agreement_id` on
  `CheckoutSession`; `agreement_id` on `PaymentMethodCharge`; `verify_only` and `decline_class`
  on `PaymentEventData`. `WebhookEvent::payment_data` also types the `verification.*` events.
  Every status, class, reason and event type stays a plain `String`, so a value the SDK does not
  know never fails a call or a parse.
- `agreement_id` on `ChargeRequest`, and `extra` on `ChargeRequest` and `RefundRequest`: the SDK
  sends every field you give, after the fields that it models, so a new API field needs no SDK
  release.
- Errors: `Error::Forbidden` for a 403 (see Changed). `Error::RateLimited` gains `code` and
  `raw`. `Error::Api` gains `raw`: the whole JSON answer, for example `error.params.webhookId` on
  409 `WEBHOOK_ALREADY_REGISTERED`.
- New constants: `auth_error_code` (the 401 and 403 codes, pinned against the wire contract) and
  `rate_limit_error_code` (`RATE_LIMIT_EXCEEDED`, `CHARGE_VELOCITY_EXCEEDED` and
  `CHARGE_DECLINE_RATE_EXCEEDED`).
- Wallet reporting: `CheckoutStatus::payment_method` and `CheckoutStatus::wallet_type`, both
  `Option<String>`. `None` while the payment is open and on older transactions; `wallet_type` is
  `None` for non-wallet payments and keeps a wallet value it does not know. Reporting data only,
  a wallet payment refunds and disputes like a card. New constants: `payment_method` (`card`,
  `wallet`, `bank_transfer`, `sepa`) and `wallet_type` (`apple_pay`, `google_pay`,
  `samsung_pay`), with `wallet_type::ALL` pinned against the gateway contract.
- Contract fixtures: `paymentMethod` and `walletType` in the status read fields and examples,
  and the `wallets` section in the wire contract.
- Contract fixture: the new status read, checkout session and charge fields, with examples. Wire
  contract fixture: the authorization error group.

### Changed

- Breaking: a 403 is now `Error::Forbidden` instead of `Error::Auth`. Code that matched
  `Error::Auth` for a 403 must match `Error::Forbidden`. A 403 no longer tells you to check your
  key id, secret and server clock: that hint is for 401 only. 403 `INSUFFICIENT_SCOPE` repeats
  the API message that names the missing scope, and 403 `VERIFY_ONLY_NOT_ENABLED` says that
  verify-only is not on for your account. `Error::http_status()` answers 401 for `Auth` and 403
  for `Forbidden`.
- Breaking for struct literals and exhaustive patterns: `Error::Api` and `Error::RateLimited`
  have new fields, and so have the request and response structs listed above. Add the fields, or
  use `..` in a pattern.
- `decline_class` can be `None`, for example when nobody declined the payment. The docs no
  longer promise a class on every decline, and `get_status` carries the latest class.
- The README installs tag `v0.5.0` and documents key scopes, verify-only sessions,
  subscriptions, the refund list, webhook endpoints, transactions and the integration contract.

## 0.4.0

- Card fields: `CheckoutSessionRequest::integration(Integration)` (`Integration::Widget` or
  `Integration::Fields`, omitted when not set), and `CheckoutSession::integration` and
  `CheckoutSession::client_secret` on the response. `client_secret` is `Some` only for fields
  sessions. Card fields are enabled per merchant on request; see the README.
- Processor reference: `CheckoutStatus::psp_reference` and `PaymentEventData::psp_reference`. The
  payment processor's reference for the transaction, `None` until known and on a sale settled by
  reconciliation without a processor webhook. Refund and cancel events carry the original sale's
  reference. Not on `charge.*` events.
- Contract fixture: `pspReference` in the status read fields and examples.
- Contract fixture: `integrationVocabulary`, the two new checkout fields and a fields session
  example.

## 0.3.1

- `WebhookEvent` parses a verified webhook body: `id`, `event_type`, `api_version` (`None` when
  absent), `created_at` and `data`. `WebhookEvent::sequence()` reads `data.sequence` on
  `agreement.*` and `charge.*` events so you can drop out-of-order deliveries; the README has the
  ordering rule. Payloads without `apiVersion` or `sequence` still parse.
- `PaymentMethodCharge::sequence`: optional, `None` until the server sends it.
- Refunds: `create_refund(transaction_id, &RefundRequest)` (POST
  `/merchant-api/payments/{transactionId}/refunds`, HTTP 202, required and signed idempotency
  key) and `get_refund(transaction_id, refund_id)`. `RefundRequest::new(key)` refunds everything
  still refundable and sends no `amount`; `.amount(minor)` makes it partial, `.reason(...)` is
  optional. `Refund` has the contract's eight fields plus `is_succeeded()`, `is_terminal()` and
  `failure()`, which reads an unknown failure code as `REFUND_FAILED`. New constants:
  `refund_status`, `refund_failure_code`, and `refund_error_code` with
  `retry_window_seconds`. Refund HTTP errors arrive as `Error::Api` with their code.
- `WebhookEvent::payment_data()` types `data` on `payment.*` events as `PaymentEventData`, with
  every field the gateway sends, including `stored_payment_method` (the existing
  `StoredPaymentMethod`, set on `payment.succeeded` and `payment.requires_capture` only).
  `WebhookEvent::stored_payment_method()` is the shortcut. It can be `None` even when a card was
  saved; the status read is the source of truth. Explicit nulls read as `None`.
- The canonical `merchant-api-contract.json` now carries the refund endpoints and vocabularies,
  and the contract test pins them.

## 0.3.0

Breaking. All six Dominaite SDKs move to 0.3.0 together with the same changes.

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
- Contract refresh (gateway contract 2026-09-16): a stored payment method can be `retired`,
  `stored_payment_method_status::RETIRED`, and carries `retired_reason: Option<String>` with the
  `retired_reason` constants `HARD_DECLINE`, `CHARGEBACK` and `SOURCE_SALE_REVERSED`. A retired
  card is not chargeable and never becomes active again.
- `session_error_code::STOREFRONT` now lists the storefront codes in the contract's order and
  is pinned against the contract's `storefrontErrorCodes`: 400 `STOREFRONT_MISMATCH`, 409
  `STOREFRONT_INACTIVE`, 409 `STOREFRONT_NOT_WHITELISTED`, none retryable, none a refusal.

### Migrating from 0.2

Pass a key to every request constructor. For a checkout, derive it from the order:

```rust
let key = IdempotencyKey::for_order("shop-a1b2c3d4", "order-1042", 2500, "EUR")?;
let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042", key);
```

For a stored-card charge, derive it from what you bill, e.g.
`IdempotencyKey::new("sub-8817-2026-10")?`. A key you built yourself with spaces or non-ASCII
characters is now refused. If you relied on `is_terminal()` to stop polling a `disputed`
payment, keep polling it now.
