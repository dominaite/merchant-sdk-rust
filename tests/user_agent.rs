//! The User-Agent every request carries: `dominaite-rust/<version> (rust <rustc
//! version>)`, then the caller's suffix. Support and the API's usage report read it.

#[allow(dead_code)]
mod support;

use dominaite::{Client, Error, VERSION};
use support::{MockServer, Reply};

const KEY_ID: &str = "dmk_0123456789abcdef0123456789abcdef";
const SECRET: &str = "dms_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const PING: &str = r#"{"pong":true,"merchantId":"m_1","serverTime":"2026-10-17T10:00:00Z","serverUnixTime":1792231200,"clockSkewSeconds":0}"#;

/// `(rust <version>)` with a version that has no space or parenthesis in it.
fn assert_runtime_part(rest: &str) -> &str {
    let inner = rest
        .strip_prefix("(rust ")
        .unwrap_or_else(|| panic!("no (rust ...) part: {rest}"));
    let (version, after) = inner
        .split_once(')')
        .unwrap_or_else(|| panic!("unclosed runtime part: {rest}"));
    assert!(!version.is_empty(), "empty rust version: {rest}");
    assert!(
        !version.contains([' ', '(']),
        "rust version has a space or parenthesis: {version}"
    );
    after
}

#[test]
fn the_user_agent_is_the_sdk_the_version_and_the_compiler() {
    let server = MockServer::start(vec![Reply::enveloped(PING)]);
    let client = Client::new(KEY_ID, SECRET, server.base_url()).unwrap();
    client.ping().unwrap();

    let sent = server.only_request();
    let user_agent = sent.header("user-agent").expect("a User-Agent");
    assert_eq!(user_agent, client.user_agent());
    let rest = user_agent
        .strip_prefix(&format!("dominaite-rust/{VERSION} "))
        .unwrap_or_else(|| panic!("wrong prefix: {user_agent}"));
    assert_eq!(
        assert_runtime_part(rest),
        "",
        "nothing after the runtime part"
    );
}

#[test]
fn a_suffix_follows_the_runtime_part_after_one_space() {
    let server = MockServer::start(vec![Reply::enveloped(PING)]);
    let client = Client::builder(KEY_ID, SECRET)
        .base_url(server.base_url())
        .user_agent("  shop/1.2  ")
        .build()
        .unwrap();
    client.ping().unwrap();

    let sent = server.only_request();
    let user_agent = sent.header("user-agent").expect("a User-Agent");
    let rest = user_agent
        .strip_prefix(&format!("dominaite-rust/{VERSION} "))
        .unwrap_or_else(|| panic!("wrong prefix: {user_agent}"));
    assert_eq!(assert_runtime_part(rest), " shop/1.2");
}

#[test]
fn a_blank_suffix_is_no_suffix() {
    let client = Client::builder(KEY_ID, SECRET)
        .base_url(dominaite::SANDBOX_BASE_URL)
        .user_agent("   ")
        .build()
        .unwrap();
    let rest = client
        .user_agent()
        .strip_prefix(&format!("dominaite-rust/{VERSION} "))
        .unwrap();
    assert_eq!(assert_runtime_part(rest), "");
}

#[test]
fn a_suffix_with_a_control_character_is_refused() {
    for suffix in ["shop\r\nX-Evil: 1", "shop\n", "a\tb", "a\u{7f}b"] {
        let result = Client::builder(KEY_ID, SECRET)
            .base_url(dominaite::SANDBOX_BASE_URL)
            .user_agent(suffix)
            .build();
        assert!(
            matches!(result, Err(Error::Validation { .. })),
            "{suffix:?} must be refused"
        );
    }
}
