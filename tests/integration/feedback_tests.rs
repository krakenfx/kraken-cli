//! `kraken feedback` integration: upload to a mock ingest endpoint, then
//! local DuckDB persistence under an isolated config home.

use predicates::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::common::{config_dir_in, kraken_in};

#[test]
fn feedback_help_documents_types_and_limits() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["feedback", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("feature-request"))
        .stdout(predicate::str::contains("friction"))
        .stdout(predicate::str::contains("bug"))
        .stdout(predicate::str::contains("--input-string"))
        .stdout(predicate::str::contains("--input-file"))
        .stdout(predicate::str::contains("--llm-model"))
        .stdout(predicate::str::contains("8192"))
        .stdout(predicate::str::contains("uploads the feedback"));
}

#[tokio::test]
async fn feedback_input_string_uploads_then_persists() {
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
    assert!(v["id"].as_i64().unwrap() >= 1);
    assert!(v["event_id"].as_str().unwrap().len() > 10);

    let db_path = config_dir_in(&home).join("feedback.duckdb");
    assert!(db_path.exists(), "expected {}", db_path.display());
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
async fn feedback_rate_limit_fails_without_local_row() {
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
            "should not be stored",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("rate limited"));

    let db_path = config_dir_in(&home).join("feedback.duckdb");
    assert!(
        !db_path.exists(),
        "failed upload must not create {}",
        db_path.display()
    );
}

#[tokio::test]
async fn feedback_server_error_fails_without_local_row() {
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
            "should not be stored",
            "-o",
            "json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("server error"));

    assert!(!config_dir_in(&home).join("feedback.duckdb").exists());
}

#[test]
fn feedback_rejects_empty_body() {
    let home = tempfile::tempdir().unwrap();
    kraken_in(&home)
        .args(["feedback", "bug", "--input-string", "   ", "-o", "json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("must not be empty"));
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

#[test]
fn feedback_rejects_both_input_sources() {
    let home = tempfile::tempdir().unwrap();
    let body = home.path().join("note.txt");
    std::fs::write(&body, "x").unwrap();
    kraken_in(&home)
        .args([
            "feedback",
            "bug",
            "--input-string",
            "a",
            "--input-file",
            body.to_str().unwrap(),
        ])
        .assert()
        .failure();
}
