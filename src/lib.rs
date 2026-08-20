//! Server-side Rust client for the Dominaite merchant API.
//!
//! One call from your backend opens a hosted checkout session; a two-line script
//! tag renders the payment widget on your page. Card details go straight from
//! your customer's browser into the widget. They never touch your server or this
//! crate, which keeps your PCI scope minimal (SAQ A).
//!
//! Keep your API secret on the server. Never ship it to a browser, never commit
//! it, never log it.
//!
//! ```no_run
//! use dominaite::{CheckoutSessionRequest, Client};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::builder(
//!     std::env::var("DOMINAITE_KEY_ID")?,
//!     std::env::var("DOMINAITE_SECRET")?,
//! )
//! // Unset is fine: an empty value keeps the production default.
//! .base_url(std::env::var("DOMINAITE_BASE_URL").unwrap_or_default())
//! .build()?;
//!
//! // Verify credentials and clock before minting anything.
//! client.ping()?;
//!
//! let session = client.create_checkout_session(
//!     &CheckoutSessionRequest::new(2500, "EUR", "order-1042"), // 2500 = 25.00 EUR
//! )?;
//! // Hand session.cashier_key and session.cashier_token to the embed snippet.
//! # Ok(())
//! # }
//! ```
//!
//! Amounts are always integers in MINOR units. Errors are typed: see [`Error`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod client;
mod error;
mod signing;
mod types;

pub use client::{
    Client, ClientBuilder, RetryOptions, DEFAULT_BASE_URL, PING_PATH, SESSIONS_PATH, VERSION,
};
pub use error::{Error, Result};
pub use signing::{sha256_hex, sign_request, SignRequest};
pub use types::{status, CheckoutSession, CheckoutSessionRequest, CheckoutStatus, Customer, Ping};
