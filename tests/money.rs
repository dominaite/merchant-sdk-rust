//! The minor-units contract: a decimal string in, the integer the API takes
//! out, by the currency's ISO 4217 exponent. No floats anywhere.

use dominaite::{currency_exponent, to_minor_units, Error};

fn assert_rejected(amount: &str, currency: &str) {
    match to_minor_units(amount, currency) {
        Err(Error::Validation { .. }) => {}
        other => panic!("{amount:?} {currency}: expected a validation error, got {other:?}"),
    }
}

#[test]
fn exponents_follow_iso_4217() {
    for currency in [
        "EUR", "USD", "GBP", "BGN", "RON", "CHF", "PLN", "CZK", "HUF", "SEK", "DKK", "NOK",
    ] {
        assert_eq!(currency_exponent(currency), Some(2), "{currency}");
    }
    for currency in ["JPY", "KRW", "ISK"] {
        assert_eq!(currency_exponent(currency), Some(0), "{currency}");
    }
    for currency in ["BHD", "KWD", "OMR", "JOD", "TND"] {
        assert_eq!(currency_exponent(currency), Some(3), "{currency}");
    }
    assert_eq!(currency_exponent("eur"), Some(2), "case-insensitive");
    assert_eq!(currency_exponent("XXX"), None);
    assert_eq!(currency_exponent(""), None);
}

/// The float trap: 0.1 + 0.2 is 0.30000000000000004, and a float round trip
/// of "0.30" can land on 29. Parsing the string cannot.
#[test]
fn thirty_cents_is_thirty() {
    assert_eq!(to_minor_units("0.30", "EUR").expect("valid"), 30);
    assert_eq!(to_minor_units("0.3", "EUR").expect("valid"), 30);
}

#[test]
fn amounts_convert_by_the_currency_exponent() {
    for (amount, currency, expected) in [
        ("25.00", "EUR", 2500),
        ("25", "EUR", 2500),
        ("25.5", "usd", 2550),
        ("0.01", "GBP", 1),
        ("1234.56", "BGN", 123456),
        ("500", "JPY", 500),
        ("1.250", "KWD", 1250),
        ("1.25", "KWD", 1250),
        ("0", "EUR", 0),
        ("007.50", "EUR", 750),
        (" 25.00 ", "EUR", 2500),
    ] {
        assert_eq!(
            to_minor_units(amount, currency).expect("valid"),
            expected,
            "{amount:?} {currency}"
        );
    }
}

#[test]
fn more_decimal_places_than_the_currency_has_are_rejected_not_rounded() {
    assert_rejected("0.305", "EUR");
    assert_rejected("100.5", "JPY");
    assert_rejected("1.2345", "KWD");
}

#[test]
fn anything_but_a_plain_decimal_is_rejected() {
    for amount in [
        "",
        " ",
        "-25.00",
        "+25.00",
        "25,00",
        "1,000.00",
        "1 000",
        "12.",
        ".5",
        "1.2.3",
        "abc",
        "1e3",
        "٣",
        "25.00 EUR",
    ] {
        assert_rejected(amount, "EUR");
    }
}

#[test]
fn an_unknown_currency_is_rejected() {
    assert_rejected("25.00", "XXX");
    assert_rejected("25.00", "");
}

#[test]
fn an_amount_too_large_for_i64_is_rejected() {
    assert_eq!(
        to_minor_units("92233720368547758.07", "EUR").expect("i64::MAX fits"),
        i64::MAX
    );
    assert_rejected("92233720368547758.08", "EUR");
    assert_rejected("99999999999999999999999", "JPY");
}
