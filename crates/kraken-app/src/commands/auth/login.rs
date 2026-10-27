use super::active_key::{EXPIRY_FORMAT, active_cell, can_prompt, offer_new_key, report_override};
use crate::{
    AppContext,
    commands::Execute,
    config,
    errors::{KrakenError, Result},
    output::CommandOutput,
    url_policy,
};
use kraken_auth::{
    AuthClient, ClientId, Config, Endpoints, KeyLifetime, KeySlot, Permissions, ReplacedKey,
    SavedLogin,
};
use kraken_contract::auth::Login;
use url::Url;

impl Execute for Login {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Self {
            read_only,
            valid_for,
            no_browser,
        } = self;
        let lifetime = key_lifetime(valid_for.as_deref())?;
        let permissions = if read_only {
            Permissions::read_only()
        } else {
            Permissions::all()
        };

        // A broken file fails here, before it costs a sign-in.
        let store = config::credential_store()?;
        store.load()?;

        let config = sign_in_config(ctx)?;
        // Friction, not a boundary: a script can allocate a terminal. Kraken can
        // skip the consent page after a first approval, so without it an agent
        // could mint a trading key with no one at the keyboard.
        if !can_prompt() {
            return Err(KrakenError::Validation(
                "`kraken auth login` needs a terminal: run it yourself, not from a script or an \
                 agent."
                    .into(),
            ));
        }
        let http = kraken_rest::Transport::sign_in_http(&crate::cli::rest_options(
            ctx.accept_invalid_certs,
        ))?;
        let pending = AuthClient::new(config, http)
            .start_login(permissions)
            .await?;
        prompt_sign_in(pending.authorize_url(), no_browser);
        let grant = pending.finish().await?;

        let mut saved = kraken_auth::save_login_key(&grant, &store, lifetime).await?;
        let active = offer_new_key(&store, std::mem::take(&mut saved.stored), KeySlot::SignIn);
        // A kept key still signs; kraken-auth already warned that its delete failed.
        if let Some(ReplacedKey::Deleted(name)) = &saved.replaced {
            tracing::warn!(
                key = %name,
                "the replaced sign-in key is deleted. Restart a stream or session that still uses it."
            );
        }
        let overridden_by = report_override(active, ctx.spot.source_in_use());
        Ok(signed_in(&saved, active, overridden_by))
    }
}

/// Checked before the browser opens, so a bad value never costs a sign-in.
fn key_lifetime(valid_for: Option<&str>) -> Result<KeyLifetime> {
    let Some(text) = valid_for else {
        return Ok(KeyLifetime::default());
    };
    let refuse = |why: &dyn std::fmt::Display| {
        KrakenError::Validation(format!("--valid-for '{text}': {why}"))
    };
    let parsed = kraken_lab::parse_window(text).map_err(|why| refuse(&why))?;
    KeyLifetime::try_from(parsed).map_err(|why| refuse(&why))
}

/// kraken-cli's Kraken Connect client: public, PKCE S256, localhost redirects
/// only. A public client ID is not a secret.
const KRAKEN_CLI_CLIENT_ID: &str = "230dc4f672711a31e0ae01db611cf68c";

/// Which sign-in page, and where its code goes. A production code needs
/// verified TLS: it grants the user's real account.
fn sign_in_config(ctx: &AppContext) -> Result<Config> {
    let spot = ctx.spot();
    let authorize = url_policy::resolve_authorize_url_override(
        ctx.oauth.authorize_url.as_deref(),
        ctx.allow_any_url_host,
    )
    .map_err(|e| e.labelled("--oauth-authorize-url / KRAKEN_OAUTH_AUTHORIZE_URL"))?;
    let authorize = match authorize {
        Some(url) => Url::parse(&url)?,
        None => Endpoints::production().authorize().clone(),
    };
    let endpoints = Endpoints::new(authorize, Url::parse(spot.base_url())?)?;
    if endpoints.is_production() && ctx.accept_invalid_certs {
        return Err(KrakenError::Validation(
            "The sign-in to your Kraken account needs verified TLS. Unset \
             KRAKEN_DANGER_ACCEPT_INVALID_CERTS."
                .into(),
        ));
    }
    let client_id = client_id_for(ctx.oauth.client_id.as_deref(), &endpoints)?;
    Ok(Config::new(client_id).with_endpoints(endpoints))
}

/// The built-in client is registered on the production sign-in page only, so
/// another page needs its own client ID rather than a confusing remote refusal.
fn client_id_for(client_id: Option<&str>, endpoints: &Endpoints) -> Result<ClientId> {
    match client_id {
        Some(id) => Ok(id.parse()?),
        None if endpoints.is_production() => Ok(KRAKEN_CLI_CLIENT_ID.parse()?),
        None => Err(KrakenError::Config(
            "A sign-in page override (KRAKEN_OAUTH_AUTHORIZE_URL) needs its own client ID: set \
             KRAKEN_OAUTH_CLIENT_ID."
                .into(),
        )),
    }
}

/// Always write the URL to stderr. Then a session without a browser, for
/// example over SSH, can complete the sign-in.
fn prompt_sign_in(url: &Url, no_browser: bool) {
    eprintln!("Sign in to Kraken to create an API key for kraken-cli:\n\n  {url}\n");
    if !no_browser && let Err(error) = open_browser(url) {
        tracing::warn!(%error, "could not open a browser. Open the URL above.");
    }
    eprintln!("Waiting for your approval in the browser...");
}

/// Give the URL to the OS opener as one argument. No shell parses it. The
/// opener is not awaited: `xdg-open` can block until the browser exits, and
/// `explorer` exits 1 even on success.
fn open_browser(url: &Url) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(opener)
        .arg(url.as_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // A detached thread reaps the opener and never holds the process open.
    std::thread::spawn(move || {
        if let Err(error) = child.wait() {
            tracing::debug!(%error, "could not wait for the browser opener");
        }
    });
    Ok(())
}

fn signed_in(
    saved: &SavedLogin,
    active: Option<KeySlot>,
    overridden_by: Option<String>,
) -> CommandOutput {
    let expires = saved.valid_until.format(EXPIRY_FORMAT).to_string();
    let granted: Vec<String> = saved.permissions.iter().map(|p| p.to_string()).collect();
    let mut pairs = vec![
        ("Status".into(), "Signed in".to_string()),
        ("API Key".into(), saved.name.to_string()),
        ("Expires".into(), expires),
        ("Permissions".into(), granted.join(", ")),
        ("Active Spot Key".into(), active_cell(active)),
    ];
    match &saved.replaced {
        Some(ReplacedKey::Deleted(name)) => pairs.push(("Deleted Key".into(), name.to_string())),
        Some(ReplacedKey::Kept(name)) => pairs.push((
            "Replaced Key".into(),
            format!("{name} (not deleted: it expires, or delete it in the Kraken UI)"),
        )),
        None => {}
    }
    let json = serde_json::json!({
        "status": "signed_in",
        "api_key_name": saved.name.as_str(),
        "valid_until": saved.valid_until.to_rfc3339(),
        "permissions": granted,
        "active_spot_key": active,
        "replaced_api_key_name": saved.replaced.as_ref().map(|replaced| replaced.name().as_str()),
        "replaced_api_key_deleted": saved.replaced.as_ref().map(ReplacedKey::is_deleted),
        "overridden_by": overridden_by,
    });
    CommandOutput::key_value(pairs, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_auth::{KeyName, StoredAuth};

    fn saved(replaced: ReplacedKey) -> SavedLogin {
        SavedLogin {
            name: "kraken-cli-0000beef".parse().unwrap(),
            valid_until: chrono::Utc::now() + chrono::Duration::days(1),
            permissions: Permissions::all(),
            replaced: Some(replaced),
            stored: StoredAuth::default(),
        }
    }

    fn old_key() -> KeyName {
        "kraken-cli-0ldkey00".parse().unwrap()
    }

    fn sign_in_page(authorize: &str) -> Endpoints {
        Endpoints::new(
            Url::parse(authorize).unwrap(),
            Url::parse("https://api.kraken.com").unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn production_sign_in_uses_the_registered_client() {
        let id = client_id_for(None, &Endpoints::production()).unwrap();
        assert_eq!(id.as_str(), "230dc4f672711a31e0ae01db611cf68c");
    }

    #[test]
    fn an_explicit_client_id_wins_over_the_registered_one() {
        let id = client_id_for(Some("abc123"), &Endpoints::production()).unwrap();
        assert_eq!(id.as_str(), "abc123");
    }

    #[test]
    fn another_sign_in_page_never_gets_the_production_client() {
        let page = sign_in_page("https://sign-in.example/oauth/authorize");
        let refused = client_id_for(None, &page).unwrap_err();
        assert!(matches!(refused, KrakenError::Config(_)), "{refused}");
    }

    #[test]
    fn a_replaced_key_that_was_deleted_is_reported_as_deleted() {
        let shown = signed_in(&saved(ReplacedKey::Deleted(old_key())), None, None);
        assert_eq!(shown.data["replaced_api_key_deleted"], true);
        assert!(shown.rows.iter().any(|row| row[0] == "Deleted Key"));
    }

    #[test]
    fn a_replaced_key_that_was_kept_is_never_called_deleted() {
        let shown = signed_in(&saved(ReplacedKey::Kept(old_key())), None, None);
        assert_eq!(shown.data["replaced_api_key_deleted"], false);
        assert_eq!(shown.data["replaced_api_key_name"], "kraken-cli-0ldkey00");
        assert!(shown.rows.iter().all(|row| row[0] != "Deleted Key"));
    }
}
