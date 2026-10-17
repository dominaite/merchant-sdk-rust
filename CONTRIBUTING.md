# Contributing

Thank you for helping. This file says how to build, test and change the crate.

## Build and test

The minimum supported Rust version (MSRV) is 1.85. It is `rust-version` in `Cargo.toml`.
CI runs the tests on 1.85 and on stable.

Run the same checks as CI before you open a pull request:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked --all-targets
```

Do not use language or library features newer than the MSRV.

## Tests

Every change gets a test. Test what a caller gets from the crate, not how the crate does it.
A bug fix gets a test that fails on the old code.

## Contract fixtures

The API publishes a machine-readable contract at `GET /merchant-api/integration/contract`.
The crate follows that contract. It does not add its own rules.

- `tests/merchant-api-contract.json` is the response contract. All Dominaite SDKs keep a
  byte-identical copy. Do not edit it here. When a test fails against it, fix the crate.
- `tests/merchant-api-wire-contract.json` is generated from the production contract by the jq
  projection in `.github/workflows/contract-drift.yml`. Do not edit it by hand. When the
  contract-drift check goes red, the API moved: regenerate the fixture with that projection,
  update the crate's constants, and release.

The API omits null properties in responses (except `nextCursor`). Webhook payloads send explicit
nulls. Enum values are snake_case strings. The parsers must accept all of these.

Amounts are in minor units, per the currency exponents in the contract.

## Versioning

The crate is 0.x on purpose. A minor release (0.6 to 0.7) can contain breaking changes. A
patch release (0.6.0 to 0.6.1) does not. Put every user-visible change in `CHANGELOG.md` under
the unreleased version. Do not change the version in `Cargo.toml`.

## Releases

The maintainer tags releases. A pull request never creates a tag, a GitHub release or a
published crate.

## Security

Do not open a public issue for a security problem. See [SECURITY.md](SECURITY.md).
