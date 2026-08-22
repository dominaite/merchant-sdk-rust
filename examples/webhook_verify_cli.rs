//! Verifies one webhook delivery from a file, for cross-SDK vector checks.
//!
//! ```sh
//! export DOMINAITE_WEBHOOK_SECRET=whsec_...
//! export BODY_FILE=./delivery.json
//! export SIG='t=1755700000,v1=...'
//! cargo run --example webhook_verify_cli
//! ```
//!
//! Exits non-zero when verification fails.

fn main() {
    let body = std::fs::read_to_string(std::env::var("BODY_FILE").unwrap()).unwrap();
    let sig = std::env::var("SIG").unwrap();
    let secret = std::env::var("DOMINAITE_WEBHOOK_SECRET").unwrap();
    dominaite::verify_webhook(&body, &sig, &secret, 300, None).expect("verification failed");
}
