use crate::app::resolution::command_failure_message;
use crate::app::server::ensure_http_serve_bind_allowed;
use crate::app::{classify_local_refresh_failure_state, map_api_error};
use crate::readiness;
use codestory_contracts::api::ApiError;

#[test]
fn command_failure_message_keeps_typed_guidance_through_outer_context() {
    let error = map_api_error(ApiError::retrieval_unavailable(
        "retrieval is unavailable",
        "/tmp/project",
        vec!["codestory-cli retrieval index --project /tmp/project".to_string()],
    ))
    .context("retrieval index finalize");

    let message = command_failure_message(&error);
    assert!(message.starts_with("retrieval index finalize:"));
    assert!(message.contains("retrieval_unavailable: retrieval is unavailable"));
    assert!(message.contains("Minimum next:"));
}

#[test]
fn command_failure_message_leaves_untyped_errors_unchanged() {
    let error = anyhow::anyhow!("storage unavailable").context("open project");

    assert_eq!(command_failure_message(&error), "open project");
}

#[test]
fn http_serve_allows_loopback_bind_without_acknowledgement() {
    ensure_http_serve_bind_allowed("127.0.0.1:3917", false)
        .expect("ipv4 loopback should be allowed by default");
    ensure_http_serve_bind_allowed("localhost:3917", false)
        .expect("localhost should resolve to loopback and stay ergonomic");
    ensure_http_serve_bind_allowed("[::1]:3917", false)
        .expect("ipv6 loopback should be allowed by default");
}

#[test]
fn http_serve_rejects_non_loopback_bind_without_acknowledgement() {
    let error = ensure_http_serve_bind_allowed("0.0.0.0:3917", false)
        .expect_err("wildcard bind should require explicit acknowledgement");
    let message = error.to_string();
    assert!(
        message.contains("--allow-non-loopback")
            && message.contains("without request authentication"),
        "unsafe bind error should name the guard and auth boundary: {message}"
    );
}

#[test]
fn http_serve_allows_non_loopback_bind_with_acknowledgement() {
    ensure_http_serve_bind_allowed("0.0.0.0:3917", true)
        .expect("explicit acknowledgement should allow intentional remote binds");
}

#[test]
fn classify_local_refresh_failure_state_detects_lock_contention() {
    let locked = anyhow::anyhow!("cache_busy: database is locked");
    assert_eq!(
        classify_local_refresh_failure_state(&locked),
        readiness::LocalRefreshState::Skipped
    );

    let failed = anyhow::anyhow!("index refresh failed");
    assert_eq!(
        classify_local_refresh_failure_state(&failed),
        readiness::LocalRefreshState::Failed
    );
}
