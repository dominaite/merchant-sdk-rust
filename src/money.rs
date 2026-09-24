//! Decimal amounts to MINOR units, by ISO 4217 exponent.

use crate::error::{Error, Result};

/// How many decimal places `currency` has in ISO 4217, or `None` for a code
/// this crate does not know. Case-insensitive.
///
/// ```
/// assert_eq!(dominaite::currency_exponent("EUR"), Some(2));
/// assert_eq!(dominaite::currency_exponent("jpy"), Some(0));
/// assert_eq!(dominaite::currency_exponent("KWD"), Some(3));
/// assert_eq!(dominaite::currency_exponent("XXX"), None);
/// ```
pub fn currency_exponent(currency: &str) -> Option<u32> {
    let upper = currency.trim().to_ascii_uppercase();
    match upper.as_str() {
        "EUR" | "USD" | "GBP" | "BGN" | "RON" | "CHF" | "PLN" | "CZK" | "HUF" | "SEK" | "DKK"
        | "NOK" | "AUD" | "CAD" | "NZD" | "TRY" | "RSD" | "MKD" | "UAH" => Some(2),
        "JPY" | "KRW" | "ISK" => Some(0),
        "BHD" | "KWD" | "OMR" | "JOD" | "TND" => Some(3),
        _ => None,
    }
}

/// Converts a decimal amount string into the integer MINOR units every amount
/// in this crate takes: `"25.00"` EUR is `2500`, `"0.30"` EUR is `30`, `"500"`
/// JPY is `500`, `"1.250"` KWD is `1250`.
///
/// The string is parsed digit by digit, never through a float, so `"0.30"`
/// cannot come out as 29. Pass the decimal your catalog or cart already holds
/// (a `rust_decimal` or database value rendered with `to_string()`), not the
/// result of float arithmetic.
///
/// Accepted: ASCII digits, optionally followed by `.` and at most as many
/// digits as the currency has decimal places. Surrounding whitespace is
/// ignored. Anything else is [`Error::Validation`]: a sign, a thousands
/// separator, a comma as the decimal mark, `"12."` or `".5"`, more fractional
/// digits than the currency allows (`"0.305"` EUR, `"100.5"` JPY; round in your
/// own code, where the rounding rule is yours to pick), an unknown currency,
/// and an amount too large for `i64`.
///
/// ```
/// # fn main() -> Result<(), dominaite::Error> {
/// use dominaite::to_minor_units;
///
/// assert_eq!(to_minor_units("0.30", "EUR")?, 30);
/// assert_eq!(to_minor_units("25", "eur")?, 2500);
/// assert_eq!(to_minor_units("500", "JPY")?, 500);
/// assert!(to_minor_units("0.305", "EUR").is_err());
/// # Ok(())
/// # }
/// ```
pub fn to_minor_units(amount: &str, currency: &str) -> Result<i64> {
    let exponent = currency_exponent(currency).ok_or_else(|| {
        Error::validation(format!(
            "unknown currency {currency:?}: no ISO 4217 exponent known for it"
        ))
    })?;

    let amount = amount.trim();
    let (whole, fraction) = match amount.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (amount, None),
    };

    let all_digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(whole) || fraction.is_some_and(|fraction| !all_digits(fraction)) {
        return Err(Error::validation(format!(
            "amount must be a plain decimal like \"25.00\": {amount:?}"
        )));
    }

    let fraction = fraction.unwrap_or("");
    if fraction.len() > exponent as usize {
        return Err(Error::validation(format!(
            "amount {amount:?} has more decimal places than {} allows ({exponent})",
            currency.trim().to_ascii_uppercase()
        )));
    }

    // "0.3" EUR is 30, not 3: pad the fraction out to the exponent.
    let digits = format!("{whole}{fraction:0<width$}", width = exponent as usize);
    digits
        .parse::<i64>()
        .map_err(|_| Error::validation(format!("amount {amount:?} is too large")))
}
