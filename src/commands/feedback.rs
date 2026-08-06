//! `kraken feedback`: capture product feedback from agents and users, upload
//! it to Kraken's ingest endpoint, and mirror a successful submission in a
//! local DuckDB under the Kraken config directory.
//!
//! Bodies stay free-form (`--input-string` / `--input-file`). Soft structure
//! recommendations live in help text only — the submit path never enforces
//! them. Upload is fail-closed: a non-2xx or network error fails the command
//! and nothing is written locally.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ValueEnum;
use serde_json::json;

use super::Execute;
use crate::cli::AppContext;
use crate::client;
use crate::config;
use crate::errors::{KrakenError, Result};
use crate::output::CommandOutput;
use crate::telemetry;

/// Hard cap on the feedback body (UTF-8 byte length after trim).
pub(crate) const MAX_BODY_BYTES: usize = 8192;

/// Production ingest endpoint (Cloudflare Worker on the kraken-prod account).
pub(crate) const DEFAULT_FEEDBACK_ENDPOINT: &str = "https://cli-feedback.kraken.com/v1/feedback";

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) const LONG_ABOUT: &str = "\
Submit product feedback so we can continuously improve kraken-cli and adapt it \
to what users and agents need.

Submitting uploads the feedback text (and metadata such as CLI version, \
instance id, and optional --llm-model) to Kraken over HTTPS. A copy is also \
saved locally under the Kraken config directory as a DuckDB file after a \
successful upload. Do not include API keys, secrets, passwords, or personal \
data in the body.

Types (soft recommendations for the body; only a string is required):
  feature-request  Desired outcome, why it matters, and any current workaround.
  friction         What you tried, what felt wrong, and expected vs actual.
  bug              Description, steps to reproduce, and versions (CLI via \
`kraken --version`, OS, skill name/version if relevant). Prefer --llm-model \
for the model identity instead of burying it only in the body.

Examples:
  kraken feedback bug --input-string \"order buy rejected with unclear error\" -o json
  kraken feedback friction --input-file ./notes.txt --llm-model claude-opus-4 -o json

The body must be non-empty after trim and at most 8192 UTF-8 bytes. A failed \
upload fails the command (nothing is stored locally).
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum FeedbackType {
    /// Request a new capability or improvement.
    #[value(name = "feature-request")]
    FeatureRequest,
    /// Report awkward or confusing CLI/skill behaviour.
    Friction,
    /// Report incorrect or broken behaviour.
    Bug,
}

impl FeedbackType {
    fn as_str(self) -> &'static str {
        match self {
            Self::FeatureRequest => "feature-request",
            Self::Friction => "friction",
            Self::Bug => "bug",
        }
    }
}

/// Submit product feedback (feature request, friction, or bug).
#[derive(Debug, clap::Args)]
pub(crate) struct Feedback {
    /// Feedback category: feature-request, friction, or bug.
    #[arg(value_enum)]
    feedback_type: FeedbackType,

    /// Inline feedback body (mutually exclusive with --input-file).
    #[arg(
        long,
        required_unless_present = "input_file",
        conflicts_with = "input_file"
    )]
    input_string: Option<String>,

    /// Read the feedback body from a UTF-8 file (mutually exclusive with --input-string).
    #[arg(
        long,
        required_unless_present = "input_string",
        conflicts_with = "input_string"
    )]
    input_file: Option<PathBuf>,

    /// Optional LLM / agent model identity (e.g. claude-opus-4, gpt-5).
    #[arg(long)]
    llm_model: Option<String>,
}

impl Execute for Feedback {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let body = resolve_body(self.input_string.as_deref(), self.input_file.as_deref())?;
        let llm_model = self
            .llm_model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let event_id = telemetry::new_uuid_v4();
        let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let cli_version = env!("CARGO_PKG_VERSION").to_string();
        let payload = json!({
            "event_id": event_id,
            "type": self.feedback_type.as_str(),
            "body": body,
            "cli_version": cli_version,
            "instance_id": telemetry::instance_id(),
            "agent_client": telemetry::agent_client(),
            "llm_model": llm_model,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "created_at": created_at,
        });

        upload_feedback(ctx, &payload).await?;

        let stored = persist(
            self.feedback_type,
            &body,
            llm_model.as_deref(),
            &event_id,
            &created_at,
        )?;

        let mut pairs = vec![
            ("Status".into(), "submitted".into()),
            ("Id".into(), stored.id.to_string()),
            ("EventId".into(), event_id.clone()),
            ("Type".into(), self.feedback_type.as_str().into()),
            ("Path".into(), stored.path.display().to_string()),
            ("CliVersion".into(), stored.cli_version.clone()),
        ];
        if let Some(model) = &stored.llm_model {
            pairs.push(("LlmModel".into(), model.clone()));
        }

        let json = json!({
            "status": "submitted",
            "id": stored.id,
            "event_id": event_id,
            "type": self.feedback_type.as_str(),
            "path": stored.path,
            "cli_version": stored.cli_version,
            "llm_model": stored.llm_model,
            "message": "Feedback submitted to Kraken. Thank you — we use this to improve kraken-cli.",
        });
        Ok(CommandOutput::key_value(pairs, json))
    }
}

fn resolve_body(input_string: Option<&str>, input_file: Option<&Path>) -> Result<String> {
    let raw = match (input_string, input_file) {
        (Some(s), None) => s.to_string(),
        (None, Some(path)) => std::fs::read_to_string(path).map_err(|e| {
            KrakenError::Validation(format!(
                "failed to read --input-file '{}': {e}",
                path.display()
            ))
        })?,
        _ => {
            return Err(KrakenError::Validation(
                "provide exactly one of --input-string or --input-file".into(),
            ));
        }
    };
    let body = raw.trim().to_string();
    if body.is_empty() {
        return Err(KrakenError::Validation(
            "feedback body must not be empty".into(),
        ));
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(KrakenError::Validation(format!(
            "feedback body exceeds the maximum length of {MAX_BODY_BYTES} bytes \
             (got {} bytes after trim)",
            body.len()
        )));
    }
    Ok(body)
}

fn resolve_feedback_endpoint(allow_any_host: bool) -> Result<String> {
    match config::feedback_endpoint_override() {
        None => Ok(DEFAULT_FEEDBACK_ENDPOINT.to_string()),
        Some(url) if allow_any_host => {
            // Test / staging hook: with KRAKEN_DANGER_ALLOW_ANY_URL_HOST, accept
            // http(s) to any host so local wiremock can stand in for the Worker.
            let parsed = url::Url::parse(&url)
                .map_err(|_| KrakenError::Validation(format!("Invalid URL: {url}")))?;
            let scheme = parsed.scheme();
            if scheme != "https" && scheme != "http" {
                return Err(KrakenError::Validation(format!(
                    "Unsupported URL scheme '{scheme}' for {}",
                    config::FEEDBACK_ENDPOINT_ENV
                )));
            }
            if parsed.host().is_none() {
                return Err(KrakenError::Validation(
                    "URL override must include a host".into(),
                ));
            }
            if scheme == "http" {
                tracing::warn!(
                    %url,
                    "using insecure http feedback endpoint because \
                     KRAKEN_DANGER_ALLOW_ANY_URL_HOST is enabled"
                );
            }
            Ok(url)
        }
        Some(url) => match client::resolve_url_override(Some(&url), false)? {
            Some(u) => Ok(u),
            None => Ok(DEFAULT_FEEDBACK_ENDPOINT.to_string()),
        },
    }
}

async fn upload_feedback(ctx: &AppContext, payload: &serde_json::Value) -> Result<()> {
    let endpoint = resolve_feedback_endpoint(ctx.allow_any_url_host)?;
    let http = reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(UPLOAD_TIMEOUT)
        .user_agent(telemetry::user_agent())
        .build()
        .map_err(|e| KrakenError::Network(format!("Failed to build HTTP client: {e}")))?;

    let response = http
        .post(&endpoint)
        .header("content-type", "application/json")
        .header("X-Kraken-Cli-Version", env!("CARGO_PKG_VERSION"))
        .header("X-Kraken-Instance-Id", telemetry::instance_id())
        .json(payload)
        .send()
        .await
        .map_err(|e| {
            KrakenError::Network(format!(
                "failed to upload feedback to {endpoint}: {e}. \
                 Check network connectivity and try again."
            ))
        })?;

    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body_text = response.text().await.unwrap_or_default();

    if status.as_u16() == 202 || status.is_success() {
        return Ok(());
    }

    let detail = extract_error_detail(&body_text);
    let msg = match status.as_u16() {
        413 => format!(
            "feedback upload rejected: body too large (413){}",
            detail_suffix(&detail)
        ),
        429 => {
            let retry = retry_after
                .map(|s| format!("; retry after {s}s"))
                .unwrap_or_default();
            format!(
                "feedback upload rate limited (429){retry}{}",
                detail_suffix(&detail)
            )
        }
        400..=499 => format!(
            "feedback upload rejected by server ({status}){}",
            detail_suffix(&detail)
        ),
        _ => format!(
            "feedback upload failed with server error ({status}){}",
            detail_suffix(&detail)
        ),
    };
    Err(KrakenError::Network(msg))
}

fn extract_error_detail(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(err) = v.get("error").and_then(|e| e.as_str())
    {
        return Some(err.to_string());
    }
    Some(trimmed.chars().take(200).collect())
}

fn detail_suffix(detail: &Option<String>) -> String {
    match detail {
        Some(d) => format!(": {d}"),
        None => String::new(),
    }
}

struct StoredFeedback {
    id: i64,
    path: PathBuf,
    cli_version: String,
    llm_model: Option<String>,
}

#[cfg(feature = "record-duckdb")]
fn persist(
    feedback_type: FeedbackType,
    body: &str,
    llm_model: Option<&str>,
    event_id: &str,
    submitted_at: &str,
) -> Result<StoredFeedback> {
    use duckdb::params;
    use kraken_recording::frames::duckdb::Db;

    let path = config::config_dir()?.join("feedback.duckdb");
    let db = Db::open(&path)?;
    db.ensure_schema(&[
        "CREATE SEQUENCE IF NOT EXISTS feedback_id_seq START 1",
        "CREATE TABLE IF NOT EXISTS feedback (
            id UBIGINT PRIMARY KEY DEFAULT nextval('feedback_id_seq'),
            created_at TIMESTAMP NOT NULL,
            type VARCHAR NOT NULL,
            body VARCHAR NOT NULL,
            cli_version VARCHAR NOT NULL,
            llm_model VARCHAR,
            event_id VARCHAR,
            submitted_at VARCHAR
        )",
        "ALTER TABLE feedback ADD COLUMN IF NOT EXISTS event_id VARCHAR",
        "ALTER TABLE feedback ADD COLUMN IF NOT EXISTS submitted_at VARCHAR",
    ])?;

    let cli_version = env!("CARGO_PKG_VERSION").to_string();
    let created_at = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string();

    let mut stmt = db
        .conn()
        .prepare(
            "INSERT INTO feedback (created_at, type, body, cli_version, llm_model, event_id, submitted_at)
             VALUES (CAST(? AS TIMESTAMP), ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .map_err(|e| KrakenError::Io(std::io::Error::other(e)))?;
    let id: i64 = stmt
        .query_row(
            params![
                created_at,
                feedback_type.as_str(),
                body,
                cli_version,
                llm_model,
                event_id,
                submitted_at,
            ],
            |row| row.get(0),
        )
        .map_err(|e| KrakenError::Io(std::io::Error::other(e)))?;

    Ok(StoredFeedback {
        id,
        path,
        cli_version,
        llm_model: llm_model.map(str::to_string),
    })
}

#[cfg(not(feature = "record-duckdb"))]
fn persist(
    _feedback_type: FeedbackType,
    _body: &str,
    _llm_model: Option<&str>,
    _event_id: &str,
    _submitted_at: &str,
) -> Result<StoredFeedback> {
    Err(KrakenError::Validation(
        "this build has no DuckDB support; rebuild with `--features record-duckdb` \
         to store feedback locally"
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_body() {
        let err = resolve_body(Some("   \n"), None).unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn rejects_oversized_body() {
        let big = "x".repeat(MAX_BODY_BYTES + 1);
        let err = resolve_body(Some(&big), None).unwrap_err();
        assert!(err.to_string().contains("maximum length"));
    }

    #[test]
    fn accepts_max_sized_body() {
        let body = "x".repeat(MAX_BODY_BYTES);
        assert_eq!(
            resolve_body(Some(&body), None).unwrap().len(),
            MAX_BODY_BYTES
        );
    }
}
