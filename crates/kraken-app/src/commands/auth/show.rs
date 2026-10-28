use super::active_key::{active_cell, login_label};
use crate::{AppContext, commands::Execute, config, errors::Result, output::CommandOutput};
use kraken_auth::{CredentialSource, KeyPair, StoredAuth};
use kraken_contract::auth::Show;

const REDACTED: &str = "[REDACTED]";

impl Execute for Show {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        // An MCP tool: the file read must not block a shared runtime worker.
        let stored = tokio::task::spawn_blocking(|| -> Result<StoredAuth> {
            Ok(config::credential_store()?.load()?)
        })
        .await??;
        Ok(render(&stored, ctx.spot.source_in_use()))
    }
}

/// Keys show a last-4 tail so the operator can tell which key is stored. A
/// secret is never shown, not even in part.
fn render(stored: &StoredAuth, in_use: Option<CredentialSource>) -> CommandOutput {
    let own_key = stored.own().map(KeyPair::masked_key);
    let api_secret = own_key.as_ref().map(|_| REDACTED.to_string());
    let futures_key = stored.futures().map(KeyPair::masked_key);
    // With no stored active key, a flag or env key overrides nothing.
    let overridden = stored.active().is_some() && in_use.is_some_and(CredentialSource::is_override);
    let in_use_cell = in_use.map(|source| {
        if overridden {
            format!("{source} (overrides the active key)")
        } else {
            source.to_string()
        }
    });

    let cell = |value: &Option<String>| value.clone().unwrap_or_else(|| "(not set)".into());
    let pairs = vec![
        ("API Key".into(), cell(&own_key)),
        ("API Secret".into(), cell(&api_secret)),
        ("Sign-in Key".into(), cell(&stored.login().map(login_label))),
        ("Futures API Key".into(), cell(&futures_key)),
        ("Active Spot Key".into(), active_cell(stored.active())),
        ("Spot Key In Use".into(), cell(&in_use_cell)),
    ];

    // `{present, masked}` lets a script branch on presence without parsing a
    // string.
    let field = |value: &Option<String>| match value {
        Some(masked) => serde_json::json!({ "present": true, "masked": masked }),
        None => serde_json::json!({ "present": false }),
    };
    let login_json = match stored.login() {
        Some(login) => serde_json::json!({
            "present": true,
            "name": login.name.as_str(),
            "valid_until": login.valid_until.to_rfc3339(),
            "expired": login.is_expired(),
        }),
        None => serde_json::json!({ "present": false }),
    };
    let json = serde_json::json!({
        "api_key": field(&own_key),
        "api_secret": field(&api_secret),
        "sign_in_key": login_json,
        "futures_api_key": field(&futures_key),
        "active_spot_key": stored.active(),
        "active_spot_source": in_use.map(|source| source.to_string()),
        "overridden": overridden,
    });
    CommandOutput::key_value(pairs, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_env_key_with_no_stored_key_overrides_nothing() {
        let shown = render(&StoredAuth::default(), Some(CredentialSource::Env));
        assert_eq!(shown.data["overridden"], false);
    }
}
