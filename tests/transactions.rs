//! The transaction list against the loopback mock: `list_transactions` on
//! `/merchant-api/transactions`. The query string travels in the URL and the
//! signature covers the path only. No network, no credentials.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    sign_request, status, transaction_type, Client, Error, SignRequest, TransactionListParams,
    TRANSACTIONS_PATH,
};
use support::{MockServer, Recorded, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const FROM: &str = "2026-10-01T00:00:00Z";
const TO: &str = "2026-10-08T00:00:00Z";
const FROM_ENCODED: &str = "2026-10-01T00%3A00%3A00Z";
const TO_ENCODED: &str = "2026-10-08T00%3A00%3A00Z";

const PAGE: &str = r#"{"items":[{"transactionId":"11111111-1111-4111-8111-111111111111","orderReference":"order-1042","type":"payment","status":"partially_refunded","amount":2500,"currency":"EUR","createdAt":"2026-10-01T09:00:00Z","completedAt":"2026-10-01T09:01:00Z","refundedAmount":500},{"transactionId":"22222222-2222-4222-8222-222222222222","type":"charge","status":"succeeded","amount":1990,"currency":"EUR","createdAt":"2026-10-02T09:00:00Z","completedAt":"2026-10-02T09:00:05Z","storedPaymentMethodId":"pm_0123456789abcdef0123456789abcdef"},{"transactionId":"33333333-3333-4333-8333-333333333333","type":"verification","status":"requires_capture","amount":100,"currency":"EUR","createdAt":"2026-10-03T09:00:00Z"}],"nextCursor":"eyJjIjoiMjAyNi0xMC0wMyJ9+/="}"#;

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(format!("{}/api", server.base_url()))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

fn last_page() -> Reply {
    Reply::enveloped(r#"{"items":[],"nextCursor":null}"#)
}

/// The signature covers the path without the query string, an empty
/// idempotency key and an empty body.
fn assert_signed_without_the_query(recorded: &Recorded) {
    let expected = sign_request(SignRequest {
        secret: SECRET,
        timestamp: recorded.header("X-Timestamp").expect("X-Timestamp sent"),
        method: "GET",
        path: TRANSACTIONS_PATH,
        idempotency_key: "",
        body: "",
    });
    assert_eq!(
        recorded.header("X-Signature"),
        Some(expected.as_str()),
        "the signature must cover the path only"
    );
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_eq!(recorded.body, "");
}

#[test]
fn every_parameter_goes_into_the_query_string_and_none_into_the_signature() {
    let server = MockServer::start(vec![last_page()]);
    let params = TransactionListParams::new(FROM, TO)
        .status(status::SUCCEEDED)
        .limit(100)
        .cursor("eyJjIjoiMjAyNi0xMC0wMyJ9+/=");

    client_for(&server)
        .list_transactions(&params)
        .expect("a page");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "GET");
    assert_eq!(
        recorded.path,
        format!(
            "/api{TRANSACTIONS_PATH}?from={FROM_ENCODED}&to={TO_ENCODED}&status=succeeded&limit=100&cursor=eyJjIjoiMjAyNi0xMC0wMyJ9%2B%2F%3D"
        )
    );
    assert_signed_without_the_query(&recorded);
}

#[test]
fn the_first_page_sends_only_the_window() {
    let server = MockServer::start(vec![last_page()]);
    client_for(&server)
        .list_transactions(&TransactionListParams::new(FROM, TO))
        .expect("a page");
    assert_eq!(
        server.only_request().path,
        format!("/api{TRANSACTIONS_PATH}?from={FROM_ENCODED}&to={TO_ENCODED}")
    );
}

/// A `+` in an offset would read as a space if it went out as is.
#[test]
fn an_offset_keeps_its_plus_sign() {
    let server = MockServer::start(vec![last_page()]);
    client_for(&server)
        .list_transactions(&TransactionListParams::new(
            "2026-10-01T02:00:00+02:00",
            "2026-10-08",
        ))
        .expect("a page");
    assert_eq!(
        server.only_request().path,
        format!("/api{TRANSACTIONS_PATH}?from=2026-10-01T02%3A00%3A00%2B02%3A00&to=2026-10-08")
    );
}

#[test]
fn a_page_reads_every_field_and_keeps_each_item_raw() {
    let server = MockServer::start(vec![Reply::enveloped(PAGE)]);
    let page = client_for(&server)
        .list_transactions(&TransactionListParams::new(FROM, TO))
        .expect("a page");

    assert_eq!(page.items.len(), 3);
    assert_eq!(
        page.next_cursor.as_deref(),
        Some("eyJjIjoiMjAyNi0xMC0wMyJ9+/=")
    );

    let payment = &page.items[0];
    assert_eq!(
        payment.transaction_id,
        "11111111-1111-4111-8111-111111111111"
    );
    assert_eq!(payment.order_reference.as_deref(), Some("order-1042"));
    assert_eq!(payment.transaction_type, transaction_type::PAYMENT);
    assert_eq!(payment.status, status::PARTIALLY_REFUNDED);
    assert_eq!(payment.amount, 2500);
    assert_eq!(payment.currency, "EUR");
    assert_eq!(payment.created_at, "2026-10-01T09:00:00Z");
    assert_eq!(
        payment.completed_at.as_deref(),
        Some("2026-10-01T09:01:00Z")
    );
    assert_eq!(payment.refunded_amount, Some(500));
    assert_eq!(payment.stored_payment_method_id, None);

    let charge = &page.items[1];
    assert_eq!(charge.transaction_type, transaction_type::CHARGE);
    assert_eq!(
        charge.stored_payment_method_id.as_deref(),
        Some("pm_0123456789abcdef0123456789abcdef")
    );
    assert_eq!(charge.order_reference, None);
    assert_eq!(charge.refunded_amount, None);

    let verification = &page.items[2];
    assert_eq!(
        verification.transaction_type,
        transaction_type::VERIFICATION
    );
    assert_eq!(verification.completed_at, None);
    assert_eq!(verification.raw["type"], "verification");
    assert_eq!(page.raw["items"][2]["amount"], 100);
}

/// Send `next_cursor` back until it is `None`: the loop the docs show.
#[test]
fn paging_sends_the_next_cursor_back_with_the_same_window() {
    let server = MockServer::start(vec![Reply::enveloped(PAGE), last_page()]);
    let client = client_for(&server);
    let mut params = TransactionListParams::new(FROM, TO).status(status::SUCCEEDED);
    let mut seen = 0;
    loop {
        let page = client.list_transactions(&params).expect("a page");
        seen += page.items.len();
        match page.next_cursor {
            Some(cursor) => params = params.cursor(cursor),
            None => break,
        }
    }

    assert_eq!(seen, 3);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].path,
        format!(
            "/api{TRANSACTIONS_PATH}?from={FROM_ENCODED}&to={TO_ENCODED}&status=succeeded&cursor=eyJjIjoiMjAyNi0xMC0wMyJ9%2B%2F%3D"
        )
    );
    assert_signed_without_the_query(&requests[1]);
}

#[test]
fn a_missing_window_is_refused_before_sending() {
    let server = MockServer::start(vec![last_page()]);
    for params in [
        TransactionListParams::new("", TO),
        TransactionListParams::new(FROM, " "),
    ] {
        let error = client_for(&server)
            .list_transactions(&params)
            .expect_err("refused");
        assert!(matches!(error, Error::Validation { .. }), "{error:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_key_without_the_transactions_scope_is_forbidden() {
    let server = MockServer::start(vec![Reply::error_envelope(
        403,
        "INSUFFICIENT_SCOPE",
        "This API key does not have the transactions:read scope.",
    )]);
    let error = client_for(&server)
        .list_transactions(&TransactionListParams::new(FROM, TO))
        .expect_err("forbidden");
    assert!(matches!(error, Error::Forbidden { .. }), "{error:?}");
    assert_eq!(error.code(), Some("INSUFFICIENT_SCOPE"));
}

#[test]
fn a_window_the_api_refuses_is_an_api_error_with_the_details_in_raw() {
    let server = MockServer::start(vec![Reply::Json(
        400,
        r#"{"success":false,"data":null,"error":{"code":"VALIDATION_ERROR","message":"Validation failed","validationErrors":[{"field":"to","message":"from and to may be at most 31 days apart","code":"FIELD_VALUE_OUT_OF_RANGE"}]}}"#.to_string(),
    )]);
    let error = client_for(&server)
        .list_transactions(&TransactionListParams::new(FROM, "2026-12-01T00:00:00Z"))
        .expect_err("refused");
    assert_eq!(error.code(), Some("VALIDATION_ERROR"));
    let Error::Api { status, raw, .. } = &error else {
        panic!("expected Error::Api, got {error:?}");
    };
    assert_eq!(*status, 400);
    assert_eq!(raw["error"]["validationErrors"][0]["field"], "to");
}

#[test]
fn the_settlement_window_goes_into_the_query_string_and_none_into_the_signature() {
    let server = MockServer::start(vec![last_page()]);
    let params = TransactionListParams::new(FROM, TO)
        .settled("2026-10-13", "2026-10-14")
        .status(status::SUCCEEDED);

    client_for(&server)
        .list_transactions(&params)
        .expect("a page");

    let recorded = server.only_request();
    assert_eq!(
        recorded.path,
        format!(
            "/api{TRANSACTIONS_PATH}?from={FROM_ENCODED}&to={TO_ENCODED}&settledFrom=2026-10-13&settledTo=2026-10-14&status=succeeded"
        )
    );
    assert_signed_without_the_query(&recorded);
}

#[test]
fn a_settled_payment_reads_the_settlement_fields() {
    let server = MockServer::start(vec![Reply::enveloped(
        r#"{"items":[{"transactionId":"11111111-1111-4111-8111-111111111111","type":"payment","status":"succeeded","amount":2500,"currency":"EUR","createdAt":"2026-10-12T09:00:00Z","fee":45,"netAmount":2455,"settledAt":"2026-10-13T00:00:00Z","payoutId":"5d0c6a52-8f1e-4b7a-9c3d-2e1f0a9b8c7d"}],"nextCursor":null}"#,
    )]);
    let page = client_for(&server)
        .list_transactions(&TransactionListParams::new(FROM, TO))
        .expect("a page");

    let settled = &page.items[0];
    assert_eq!(settled.fee, Some(45));
    assert_eq!(settled.net_amount, Some(2455));
    assert_eq!(settled.settled_at.as_deref(), Some("2026-10-13T00:00:00Z"));
    assert_eq!(
        settled.payout_id.as_deref(),
        Some("5d0c6a52-8f1e-4b7a-9c3d-2e1f0a9b8c7d")
    );
}

/// The gateway leaves the settlement fields out until the payment is
/// settled, and a null reads the same as a missing field.
#[test]
fn an_unsettled_payment_reads_the_settlement_fields_as_none() {
    let server = MockServer::start(vec![Reply::enveloped(
        r#"{"items":[{"transactionId":"11111111-1111-4111-8111-111111111111","type":"payment","status":"succeeded","amount":2500,"currency":"EUR","createdAt":"2026-10-12T09:00:00Z"},{"transactionId":"22222222-2222-4222-8222-222222222222","type":"payment","status":"succeeded","amount":1000,"currency":"EUR","createdAt":"2026-10-12T10:00:00Z","fee":null,"netAmount":null,"settledAt":null,"payoutId":null}],"nextCursor":null}"#,
    )]);
    let page = client_for(&server)
        .list_transactions(&TransactionListParams::new(FROM, TO))
        .expect("a page");

    for item in &page.items {
        assert_eq!(item.fee, None);
        assert_eq!(item.net_amount, None);
        assert_eq!(item.settled_at, None);
        assert_eq!(item.payout_id, None);
    }
}
