//! `kraken feedback`: capture product feedback from agents and users and upload
//! it to Kraken's ingest endpoint.
//!
//! Bodies stay free-form (`--input-string` / `--input-file`). Soft structure
//! recommendations live in help text only — the submit path never enforces
//! them. Upload is fail-closed: a non-2xx or network error fails the command.

use super::Execute;
use crate::{
    cli::AppContext,
    config,
    errors::{KrakenError, Result},
    output::CommandOutput,
    telemetry, url_policy,
};
use kraken_contract::feedback::Feedback;
use serde_json::json;
use std::{path::Path, time::Duration};

/// Hard cap on the feedback body (UTF-8 byte length after trim).
pub(crate) const MAX_BODY_BYTES: usize = 8192;

/// Production feedback ingest endpoint.
pub(crate) const DEFAULT_FEEDBACK_ENDPOINT: &str = "https://cli-feedback.kraken.com/v1/feedback";

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(10);

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

        let mut pairs = vec![
            ("Status".into(), "submitted".into()),
            ("EventId".into(), event_id.clone()),
            ("Type".into(), self.feedback_type.as_str().into()),
            ("CliVersion".into(), cli_version.clone()),
        ];
        if let Some(model) = &llm_model {
            pairs.push(("LlmModel".into(), model.clone()));
        }

        let json = json!({
            "status": "submitted",
            "event_id": event_id,
            "type": self.feedback_type.as_str(),
            "cli_version": cli_version,
            "llm_model": llm_model,
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
            "feedback body exceeds the maximum length of {MAX_BODY_BYTES} bytes (got {} bytes \
             after trim)",
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
            // http(s) to any host so a local mock can stand in for production.
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
        Some(url) => match url_policy::resolve_url_override(Some(&url), false)? {
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
                "failed to upload feedback to {endpoint}: {e}. Check network connectivity and try \
                 again."
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
