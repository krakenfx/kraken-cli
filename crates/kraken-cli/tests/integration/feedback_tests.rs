//! `kraken feedback` integration: upload to a mock ingest endpoint under an
//! isolated config home.

use super::common::kraken_in;
use predicates::prelude::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[tokio::test]
async fn feedback_input_string_uploads() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/feedback"))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
            "status": "accepted",
            "event_id": "11111111-1111-4111-8111-111111111111"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let endpoint = format!("{}/v1/feedback", server.uri());
    let out = kraken_in(&home)
        .env("KRAKEN_FEEDBACK_ENDPOINT", &endpoint)
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .args([
            "feedback",
            "bug",
            "--input-string",
            "description; steps to reproduce; versions",
            "--llm-model",
            "claude-opus-4",
            "-o",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["status"], "submitted");
    assert_eq!(v["type"], "bug");
    assert_eq!(v["llm_model"], "claude-opus-4");
    assert!(v.get("id").is_none());
    assert!(v.get("path").is_none());
    let event_id = v["event_id"].as_str().unwrap();
    assert!(
        event_id.len() >= 36,
        "expected a UUID-shaped event_id, got {event_id:?}"
    );
}

#[tokio::test]
async fn feedback_input_file_uploads() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/feedback"))
        .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({
            "status": "accepted"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let body = home.path().join("note.txt");
    std::fs::write(&body, "  friction: expected json, got table  ").unwrap();
    let endpoint = format!("{}/v1/feedback", server.uri());

    kraken_in(&home)
        .env("KRAKEN_FEEDBACK_ENDPOINT", &endpoint)
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .args([
            "feedback",
            "friction",
            "--input-file",
            body.to_str().unwrap(),
            "-o",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""status":"submitted""#))
        .stdout(predicate::str::contains(r#""type":"friction""#));
}

#[tokio::test]
async fn feedback_rate_limit_fails_the_command() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/feedback"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "60")
                .set_body_json(serde_json::json!({"error":"rate limit exceeded"})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let endpoint = format!("{}/v1/feedback", server.uri());
    kraken_in(&home)
        .env("KRAKEN_FEEDBACK_ENDPOINT", &endpoint)
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .args([
            "feedback",
            "bug",
            "--input-string",
            "should not be accepted",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("rate limited"));
}

#[tokio::test]
async fn feedback_server_error_fails_the_command() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/feedback"))
        .respond_with(ResponseTemplate::new(502).set_body_json(serde_json::json!({
            "error": "failed to store feedback"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let home = tempfile::tempdir().unwrap();
    let endpoint = format!("{}/v1/feedback", server.uri());
    kraken_in(&home)
        .env("KRAKEN_FEEDBACK_ENDPOINT", &endpoint)
        .env("KRAKEN_DANGER_ALLOW_ANY_URL_HOST", "1")
        .args([
            "feedback",
            "bug",
            "--input-string",
            "should not be accepted",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("server error"));
}

#[test]
fn feedback_rejects_oversized_body_before_upload() {
    let home = tempfile::tempdir().unwrap();
    let big = "x".repeat(8193);
    kraken_in(&home)
        .args([
            "feedback",
            "feature-request",
            "--input-string",
            &big,
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("maximum length"));
}
