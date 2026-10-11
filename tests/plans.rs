//! Subscription plans against the loopback mock: `create_plan` and
//! `list_plans` on `/merchant-api/plans`, and `get_plan`, `update_plan` and
//! `delete_plan` on `/merchant-api/plans/{id}`. No network, no credentials.

// Only part of the mock server is needed here; the client tests use the rest.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use dominaite::{
    interval_unit, plan_status, sign_request, Client, Error, IdempotencyKey, PlanRequest,
    PlanUpdate, SignRequest, PLANS_PATH,
};
use serde_json::json;
use support::{MockServer, Recorded, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
// Mixed case on purpose: a plan id is case-sensitive and must reach the path
// exactly as given.
const PLAN_ID: &str = "plan_0123456789ABCDEF0123456789abcdef";
const PLAN_KEY: &str = "plan-pro-monthly";

const PLAN: &str = r#"{"id":"plan_0123456789ABCDEF0123456789abcdef","name":"Pro","amount":1990,"currency":"EUR","intervalUnit":"month","intervalCount":1,"trialDays":14,"status":"active","cancelUrl":"https://shop.example/account/cancel","createdAt":"2026-10-11T09:00:00Z"}"#;

fn client_for(server: &MockServer) -> Client {
    Client::builder(KEY_ID, SECRET)
        .base_url(format!("{}/api", server.base_url()))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("valid credentials")
}

fn key() -> IdempotencyKey {
    IdempotencyKey::new(PLAN_KEY).expect("a valid idempotency key")
}

fn enveloped(status: u16, data: &str) -> Reply {
    Reply::Json(
        status,
        format!(r#"{{"success":true,"data":{data},"metadata":{{"requestId":"r1"}}}}"#),
    )
}

fn plan_path() -> String {
    format!("{PLANS_PATH}/{PLAN_ID}")
}

fn assert_signature_matches(recorded: &Recorded, expected_path: &str, idempotency_key: &str) {
    let expected = sign_request(SignRequest {
        secret: SECRET,
        timestamp: recorded.header("X-Timestamp").expect("X-Timestamp sent"),
        method: &recorded.method,
        path: expected_path,
        idempotency_key,
        body: &recorded.body,
    });
    assert_eq!(
        recorded.header("X-Signature"),
        Some(expected.as_str()),
        "the signature does not cover what was sent"
    );
}

#[test]
fn create_plan_sends_every_field_in_order_and_signs_the_key() {
    let server = MockServer::start(vec![enveloped(201, PLAN)]);
    let request = PlanRequest::new("Pro", 1990, "EUR", interval_unit::MONTH, key())
        .interval_count(3)
        .period_count(12)
        .trial_days(14)
        .cancel_url("https://shop.example/account/cancel")
        .extra("futureField", json!("kept"));

    let plan = client_for(&server).create_plan(&request).expect("a plan");

    assert_eq!(plan.id, PLAN_ID);
    assert!(plan.is_active());
    let recorded = server.only_request();
    assert_eq!(recorded.method, "POST");
    assert_eq!(recorded.path, format!("/api{PLANS_PATH}"));
    assert_eq!(
        recorded.body,
        concat!(
            r#"{"name":"Pro","amount":1990,"currency":"EUR","intervalUnit":"month","#,
            r#""intervalCount":3,"periodCount":12,"trialDays":14,"#,
            r#""cancelUrl":"https://shop.example/account/cancel","futureField":"kept"}"#
        )
    );
    assert_eq!(recorded.header("Idempotency-Key"), Some(PLAN_KEY));
    assert_signature_matches(&recorded, PLANS_PATH, PLAN_KEY);
}

/// The optional fields are left out, so the API applies its own defaults: an
/// interval count of 1, no trial, billing until cancelled.
#[test]
fn a_minimal_plan_sends_only_the_required_fields() {
    let server = MockServer::start(vec![enveloped(201, PLAN)]);
    client_for(&server)
        .create_plan(&PlanRequest::new(
            "Pro",
            1990,
            "EUR",
            interval_unit::MONTH,
            key(),
        ))
        .expect("a plan");
    assert_eq!(
        server.only_request().body,
        r#"{"name":"Pro","amount":1990,"currency":"EUR","intervalUnit":"month"}"#
    );
}

/// A replay with the same key answers 200 with the plan it created.
#[test]
fn a_replayed_create_reads_the_same_plan() {
    let server = MockServer::start(vec![enveloped(200, PLAN)]);
    let plan = client_for(&server)
        .create_plan(&PlanRequest::new(
            "Pro",
            1990,
            "EUR",
            interval_unit::MONTH,
            key(),
        ))
        .expect("the same plan");
    assert_eq!(plan.id, PLAN_ID);
    assert_eq!(plan.amount, 1990);
    assert_eq!(plan.interval_unit, interval_unit::MONTH);
    assert_eq!(plan.interval_count, 1);
    assert_eq!(plan.trial_days, 14);
    assert_eq!(plan.period_count, None);
    assert_eq!(plan.archived_at, None);
    assert_eq!(plan.created_at, "2026-10-11T09:00:00Z");
    assert_eq!(plan.raw["cancelUrl"], "https://shop.example/account/cancel");
}

#[test]
fn create_plan_refuses_a_plan_the_api_would_refuse_before_sending() {
    let server = MockServer::start(vec![enveloped(201, PLAN)]);
    for request in [
        PlanRequest::new(" ", 1990, "EUR", interval_unit::MONTH, key()),
        PlanRequest::new("Pro", 0, "EUR", interval_unit::MONTH, key()),
        PlanRequest::new("Pro", -1990, "EUR", interval_unit::MONTH, key()),
        PlanRequest::new("Pro", 1990, "", interval_unit::MONTH, key()),
        PlanRequest::new("Pro", 1990, "EUR", "", key()),
    ] {
        let error = client_for(&server)
            .create_plan(&request)
            .expect_err("refused");
        assert!(matches!(error, Error::Validation { .. }), "{error:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_reused_key_is_an_api_error_with_its_code() {
    let server = MockServer::start(vec![Reply::error_envelope(
        422,
        "IDEMPOTENCY_KEY_REUSED",
        "This Idempotency-Key was used with a different body",
    )]);
    let error = client_for(&server)
        .create_plan(&PlanRequest::new(
            "Pro",
            2490,
            "EUR",
            interval_unit::MONTH,
            key(),
        ))
        .expect_err("refused");
    assert!(matches!(error, Error::Api { status: 422, .. }), "{error:?}");
    assert_eq!(error.code(), Some("IDEMPOTENCY_KEY_REUSED"));
}

/// The list route answers `data` as a JSON array, not an object.
#[test]
fn list_plans_reads_the_array_and_signs_like_a_get() {
    let archived = r#"{"id":"plan_ffffffffffffffffffffffffffffffff","name":"Old","amount":990,"currency":"EUR","intervalUnit":"year","intervalCount":1,"periodCount":2,"trialDays":0,"status":"archived","createdAt":"2026-01-01T00:00:00Z","archivedAt":"2026-10-01T00:00:00Z"}"#;
    let server = MockServer::start(vec![enveloped(200, &format!("[{PLAN},{archived}]"))]);

    let plans = client_for(&server).list_plans().expect("the plans");

    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].id, PLAN_ID);
    assert_eq!(plans[1].status, plan_status::ARCHIVED);
    assert!(!plans[1].is_active());
    assert_eq!(plans[1].period_count, Some(2));
    assert_eq!(
        plans[1].archived_at.as_deref(),
        Some("2026-10-01T00:00:00Z")
    );
    // Each plan keeps its own object.
    assert_eq!(plans[1].raw["name"], "Old");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "GET");
    assert_eq!(recorded.path, format!("/api{PLANS_PATH}"));
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_eq!(recorded.body, "");
    assert_signature_matches(&recorded, PLANS_PATH, "");
}

#[test]
fn no_plans_is_an_empty_list() {
    let server = MockServer::start(vec![enveloped(200, "[]")]);
    assert!(client_for(&server)
        .list_plans()
        .expect("the plans")
        .is_empty());
}

#[test]
fn get_plan_sends_the_id_as_given() {
    let server = MockServer::start(vec![enveloped(200, PLAN)]);
    let plan = client_for(&server).get_plan(PLAN_ID).expect("the plan");
    assert_eq!(plan.name, "Pro");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "GET");
    assert_eq!(recorded.path, format!("/api{}", plan_path()));
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_signature_matches(&recorded, &plan_path(), "");
}

#[test]
fn a_plan_that_is_not_yours_is_plan_not_found() {
    let server = MockServer::start(vec![Reply::error_envelope(
        404,
        "PLAN_NOT_FOUND",
        "No plan with this id.",
    )]);
    let error = client_for(&server)
        .get_plan(PLAN_ID)
        .expect_err("not found");
    assert!(matches!(error, Error::Api { status: 404, .. }), "{error:?}");
    assert_eq!(error.code(), Some("PLAN_NOT_FOUND"));
}

/// PATCH signs its body but no idempotency key: the route does not take one.
#[test]
fn update_plan_is_a_patch_with_a_signed_body_and_no_key() {
    let server = MockServer::start(vec![enveloped(200, PLAN)]);
    let update = PlanUpdate::new()
        .name("Pro 2027")
        .trial_days(0)
        .cancel_url("")
        .extra("futureField", json!(1));

    client_for(&server)
        .update_plan(PLAN_ID, &update)
        .expect("the plan");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "PATCH");
    assert_eq!(recorded.path, format!("/api{}", plan_path()));
    assert_eq!(
        recorded.body,
        r#"{"name":"Pro 2027","trialDays":0,"cancelUrl":"","futureField":1}"#
    );
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_signature_matches(&recorded, &plan_path(), "");
}

/// Only the fields you set are sent; the API keeps the others.
#[test]
fn an_update_sends_only_what_it_changes() {
    let server = MockServer::start(vec![enveloped(200, PLAN)]);
    client_for(&server)
        .update_plan(PLAN_ID, &PlanUpdate::new().trial_days(7))
        .expect("the plan");
    assert_eq!(server.only_request().body, r#"{"trialDays":7}"#);
}

#[test]
fn delete_plan_archives_on_a_204() {
    let server = MockServer::start(vec![Reply::Json(204, String::new())]);
    client_for(&server).delete_plan(PLAN_ID).expect("archived");

    let recorded = server.only_request();
    assert_eq!(recorded.method, "DELETE");
    assert_eq!(recorded.path, format!("/api{}", plan_path()));
    assert_eq!(recorded.header("Idempotency-Key"), None);
    assert_eq!(recorded.body, "");
    assert_signature_matches(&recorded, &plan_path(), "");
}

#[test]
fn a_plan_id_that_is_not_one_path_segment_is_refused_before_sending() {
    let server = MockServer::start(vec![enveloped(200, PLAN)]);
    let client = client_for(&server);
    for id in ["", " ", "plan_1/../x", "plan_1?x=1", "plan 1", "plan_1#x"] {
        for error in [
            client.get_plan(id).expect_err("refused"),
            client
                .update_plan(id, &PlanUpdate::new().name("x"))
                .expect_err("refused"),
            client.delete_plan(id).expect_err("refused"),
        ] {
            assert!(
                matches!(error, Error::Validation { .. }),
                "{id:?}: {error:?}"
            );
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_list_that_is_not_an_array_is_an_api_error() {
    let server = MockServer::start(vec![enveloped(200, PLAN)]);
    let error = client_for(&server).list_plans().expect_err("not a list");
    assert!(matches!(error, Error::Api { status: 200, .. }), "{error:?}");
}
