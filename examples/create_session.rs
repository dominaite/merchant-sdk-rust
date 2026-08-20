//! Mints a checkout session and reads its status back.
//!
//! ```sh
//! export DOMINAITE_KEY_ID=dmk_...
//! export DOMINAITE_SECRET=dms_...
//! export DOMINAITE_BASE_URL=https://func-dom-gw-payments-dev-gwc-01.azurewebsites.net/api
//! cargo run --example create_session
//! ```
//!
//! Leave DOMINAITE_BASE_URL unset to hit production.

use std::env;

use dominaite::{CheckoutSessionRequest, Client, Customer, Error};

fn main() {
    let key_id = env::var("DOMINAITE_KEY_ID").unwrap_or_default();
    let secret = env::var("DOMINAITE_SECRET").unwrap_or_default();
    if key_id.is_empty() || secret.is_empty() {
        eprintln!("Set DOMINAITE_KEY_ID and DOMINAITE_SECRET first.");
        std::process::exit(2);
    }

    let client = Client::builder(key_id, secret)
        // An unset variable is an empty string, which keeps the production default.
        .base_url(env::var("DOMINAITE_BASE_URL").unwrap_or_default())
        .build()
        .unwrap_or_else(|error| fail(error));

    // Ping first: it creates nothing and tells you whether the key, the secret,
    // the signing and the clock are all good.
    let ping = client.ping().unwrap_or_else(|error| fail(error));
    println!(
        "ping ok - merchant {}, clock skew {}s",
        ping.merchant_id, ping.clock_skew_seconds
    );

    let request = CheckoutSessionRequest::new(2500, "EUR", "order-1042") // 2500 = 25.00 EUR
        .customer(
            Customer::new()
                .first_name("Ana")
                .last_name("Kirova")
                .email("ana@example.com"),
        );

    let session = match client.create_checkout_session(&request) {
        Ok(session) => session,
        Err(error) => {
            match &error {
                Error::Refusal { code, .. } => eprintln!("Payment unavailable: {code}"),
                Error::Transport { .. } => eprintln!("Payment temporarily unavailable"),
                _ => {}
            }
            fail(error)
        }
    };

    println!(
        "session {} - cashier key {}, token {}, expires {}",
        session.transaction_id,
        session.cashier_key,
        session.cashier_token,
        session.expires_at.as_deref().unwrap_or("-")
    );

    // Straight after creation this reads `pending`: nobody has paid yet. Poll it
    // after the payer comes back to you, or on your order timeout.
    let status = client
        .get_status(&session.transaction_id)
        .unwrap_or_else(|error| fail(error));
    println!(
        "status {} (paid: {}, terminal: {})",
        status.status,
        status.is_paid(),
        status.is_terminal()
    );
}

fn fail(error: Error) -> ! {
    eprintln!("{error}");
    std::process::exit(1);
}
