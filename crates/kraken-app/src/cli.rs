//! CLI parsing and runtime-context resolution.
//!
//! [`AppContext::from_cli`] centralizes precedence across flags, environment,
//! and config so every command sees the same resolved state.

use crate::{
    config,
    errors::{self, Result},
    output::OutputFormat,
    url_policy,
    venue::Venue,
};
use clap::Parser;
use kraken_auth::{Engine, Supplied};
use kraken_contract::command::Command;
use kraken_rest::{
    futures::{FuturesAccount, FuturesRest},
    spot::{SpotAccount, SpotRest},
};
use secrecy::SecretString;

/// Parse a boolean "danger" toggle leniently: `1`, `true`, or `yes`
/// (case-insensitive, surrounding whitespace ignored) enable it; anything else
/// — including an empty value or an unrecognized string — leaves it disabled.
///
/// clap's built-in `bool` + `env` parser is strict (`true`/`false` only) and
/// errors at parse time on `1`/`yes`/empty, which would break the documented
/// `KRAKEN_DANGER_*=1` convention. This parser never errors, matching the
/// previous accessor semantics while letting clap own the env read.
fn parse_danger_toggle(value: &str) -> std::result::Result<bool, std::convert::Infallible> {
    Ok(matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    ))
}

/// Treats blank and `default` scopes as the flat account.
///
/// Other values remain verbatim so whitespace and spelling errors fail at the
/// validation boundary instead of selecting a different account.
fn normalize_workspace(workspace: Option<String>) -> Option<String> {
    workspace
        .filter(|w| !w.trim().is_empty())
        .filter(|w| w != kraken_workspace::DEFAULT_WORKSPACE)
}

/// Which global flags were typed on a REPL line, as opposed to inherited from
/// the environment or from the shell's startup context. The parsed [`Cli`]
/// cannot tell those apart — clap merges flag and env into one value — so the
/// shell reads `ValueSource` and passes the answer here.
#[derive(Default)]
pub struct LineFlags {
    /// `--workspace`: authoritative for the line that typed it.
    pub workspace: bool,
}

/// Runtime context assembled from global CLI flags and config.
pub struct AppContext {
    pub format: OutputFormat,
    pub ws_public_url: Option<String>,
    pub ws_auth_url: Option<String>,
    pub ws_l3_url: Option<String>,
    pub ws_futures_url: Option<String>,
    pub oauth: OAuthSettings,
    /// Base reconnect backoff (ms) for spot WebSocket streams; compresses the
    /// backoff schedule when set. Resolved once here so the WS layer never
    /// reads the environment.
    pub ws_reconnect_base_ms: Option<u64>,
    /// API secret source (stdin/file/flag), retained as a secret so `auth set`
    /// can persist it. Signing uses the venues' accounts; this is the flag-tier
    /// secret that feeds them.
    pub api_secret: Option<SecretString>,
    /// Workspace name (resolved `--workspace` > `$KRAKEN_WORKSPACE`), or
    /// `None` for the flat layout. The reserved `default` collapses to `None`
    /// at resolution. Carries the raw name only; validation and the existence
    /// gate run in [`scoped_base`](Self::scoped_base), the single point where
    /// scoped commands resolve their state root.
    pub workspace: Option<String>,
    pub force: bool,
    /// Disable TLS certificate verification for HTTP and WebSocket transport.
    /// Resolved once here from `KRAKEN_DANGER_ACCEPT_INVALID_CERTS` and
    /// injected into the clients/WS layer so no transport code reads the
    /// environment.
    pub accept_invalid_certs: bool,
    /// Whether non-Kraken hosts are permitted in URL overrides. Retained so the
    /// REPL can re-validate per-line URL overrides under the same policy the
    /// session was started with (see `AppContext::from_cli_with_fallback`).
    pub allow_any_url_host: bool,
    /// True when running in MCP server mode. Disables all interactive prompts.
    pub mcp_mode: bool,
    /// Live stream-reliability monitor configuration, or `None` when
    /// `--monitor` is unset. Consumed by
    /// [`stream::run`](crate::stream::run) for `ws`/`streamd`/
    /// `record`; ignored by everything else.
    pub(crate) monitor: Option<crate::monitor::MonitorConfig>,
    /// The REST venues, sharing one connection pool. A long-lived MCP server
    /// or shell keeps them, and so its keep-alive connections, across commands.
    pub(crate) spot: Venue<SpotRest>,
    pub(crate) futures: Venue<FuturesRest>,
    /// The `--otp` value, kept beside the venues so that a key stored after
    /// startup still gets it.
    pub(crate) otp: Option<SecretString>,
    /// `KRAKEN_API_KEY` and `KRAKEN_API_SECRET` are both set, so a stored Spot
    /// key is never used, whatever this command's own flags are.
    pub(crate) spot_env_key: bool,
}

/// Settings for `auth login`. An unset field means production. The token and
/// key endpoints use the Spot API URL.
#[derive(Clone, Debug, Default)]
pub struct OAuthSettings {
    pub client_id: Option<String>,
    /// Raw: `auth login` validates it, so a bad value breaks no other command.
    pub authorize_url: Option<String>,
}

impl AppContext {
    /// This context, re-scoped to another workspace — exactly what a
    /// `--workspace <name>` line would have resolved, sharing the venues'
    /// clients. For commands that thread a scope explicitly (the playground
    /// demo) instead of asking the user to export it first.
    pub fn rescope(&self, workspace: Option<String>) -> Self {
        self.with_venues(workspace, self.spot.clone(), self.futures.clone())
    }

    fn with_venues(
        &self,
        workspace: Option<String>,
        spot: Venue<SpotRest>,
        futures: Venue<FuturesRest>,
    ) -> Self {
        Self {
            format: self.format,
            ws_public_url: self.ws_public_url.clone(),
            ws_auth_url: self.ws_auth_url.clone(),
            ws_l3_url: self.ws_l3_url.clone(),
            ws_futures_url: self.ws_futures_url.clone(),
            oauth: self.oauth.clone(),
            ws_reconnect_base_ms: self.ws_reconnect_base_ms,
            api_secret: self.api_secret.clone(),
            workspace,
            force: self.force,
            accept_invalid_certs: self.accept_invalid_certs,
            allow_any_url_host: self.allow_any_url_host,
            mcp_mode: self.mcp_mode,
            monitor: None,
            spot,
            futures,
            otp: self.otp.clone(),
            spot_env_key: self.spot_env_key,
        }
    }

    /// This context with the stored keys read again, for a long-lived tool
    /// server: `auth login` deletes the key it replaces, and `auth switch`
    /// moves the active key. Flag and env keys stay, and so do the clients.
    #[must_use]
    pub fn with_current_stored_keys(&self) -> Self {
        let stored = config::StoredKeys::default();
        let spot = Venue::resolve(self.spot().clone(), None, Some(&self.spot), &stored)
            .with_otp(self.otp.clone());
        let futures = Venue::resolve(self.futures().clone(), None, Some(&self.futures), &stored);
        self.with_venues(self.workspace.clone(), spot, futures)
    }

    /// This context as a long-lived tool server's.
    ///
    /// Keeps the credentials and URLs the entry point already resolved, and
    /// overrides only what serving tool calls requires: machine output, no
    /// interactive prompt (acknowledgment is enforced at the tool layer
    /// instead), and none of the secret-bearing flag tiers, which exist to feed
    /// `auth set` and must not be reachable from a tool call. Every call shares
    /// the venues' connection pool.
    #[must_use]
    pub fn for_tool_server(&self) -> Self {
        Self {
            format: crate::output::OutputFormat::Json,
            api_secret: None,
            force: true,
            mcp_mode: true,
            // The server never drives the streaming `stream::run` path.
            monitor: None,
            ..self.rescope(self.workspace.clone())
        }
    }

    /// Resolves existing scope state without caching cross-request changes.
    pub(crate) fn scoped_base(&self) -> Result<std::path::PathBuf> {
        let base = config::config_dir()?;
        let Some(raw) = self.workspace.as_deref() else {
            return Ok(base);
        };
        // `default` was collapsed to `None` at resolution, so every name
        // reaching this point scopes a real subtree.
        let name: kraken_workspace::WorkspaceName = raw.parse()?;
        kraken_workspace::Workspaces::new(&base).ensure_exists(&name)?;
        Ok(kraken_workspace::scoped_base(&base, Some(&name)))
    }

    /// Gate a destructive operation behind an interactive yes/no prompt,
    /// aborting on decline. `--yes` skips it; the MCP server pins `force: true`
    /// because acknowledgment is enforced at its tool layer instead.
    pub(crate) fn confirm_destructive(&self, prompt: &str) -> Result<()> {
        if self.force {
            return Ok(());
        }
        // A pipeline can never answer the prompt; refuse with the fix instead
        // of surfacing dialoguer's raw "IO error: not a terminal".
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            return Err(errors::KrakenError::Validation(format!(
                "confirmation needed ({prompt}), but stdin is not a terminal — re-run with --yes"
            )));
        }
        let confirmed = dialoguer::Confirm::new()
            .with_prompt(prompt)
            .default(false)
            .interact()
            .map_err(|e| errors::KrakenError::Config(format!("Prompt error: {e}")))?;
        if confirmed {
            Ok(())
        } else {
            Err(errors::KrakenError::Validation(
                "Operation cancelled by user".into(),
            ))
        }
    }

    pub(crate) fn spot(&self) -> &SpotRest {
        self.spot.rest()
    }

    /// The Spot account, or the actionable error when no usable credentials
    /// resolved.
    pub(crate) fn spot_account(&self) -> Result<&SpotAccount> {
        self.spot.account()
    }

    pub(crate) fn futures(&self) -> &FuturesRest {
        self.futures.rest()
    }

    pub(crate) fn futures_account(&self) -> Result<&FuturesAccount> {
        self.futures.account()
    }

    /// The WS client config — URL overrides, TLS switch, token source,
    /// handshake headers — projected from the resolved context, beside the
    /// REST factories. `token_source` is `None` without spot credentials
    /// (public-only use; private endpoints then fail closed inside the
    /// client).
    pub(crate) fn ws_config(&self) -> Result<kraken_ws::WsConfig> {
        Ok(kraken_ws::WsConfig {
            urls: kraken_ws::Urls {
                public: self.ws_public_url.clone(),
                auth: self.ws_auth_url.clone(),
                l3: self.ws_l3_url.clone(),
            },
            accept_invalid_certs: self.accept_invalid_certs,
            token_source: self.ws_token_source(),
            // The transport crate takes the identity headers as data; it knows
            // nothing about this binary's identity.
            headers: crate::telemetry::ws_handshake_headers()
                .into_iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        })
    }

    /// The WS reconnect schedule, with the `--ws-reconnect-base-ms` /
    /// `KRAKEN_WS_RECONNECT_BASE_MS` knob folded into `initial` (the rest keeps
    /// its defaults). A base above the default cap is honoured: the backoff
    /// raises its effective `max` to `initial`.
    pub(crate) fn ws_reconnect_policy(&self) -> kraken_ws::ReconnectPolicy {
        use kraken_ws::ReconnectPolicy;
        match self.ws_reconnect_base_ms {
            Some(ms) => ReconnectPolicy {
                initial: std::time::Duration::from_millis(ms),
                ..ReconnectPolicy::default()
            },
            None => ReconnectPolicy::default(),
        }
    }

    /// The live token source, or `None` when no spot credentials are
    /// configured.
    fn ws_token_source(&self) -> Option<std::sync::Arc<dyn kraken_ws::TokenSource>> {
        let account = self.spot.optional_account()?;
        Some(std::sync::Arc::new(crate::ws_token::SpotTokenSource::new(
            account.clone(),
        )))
    }

    /// Assemble the runtime context for a one-shot CLI invocation.
    ///
    /// This is the single place where global flags, environment variables, and
    /// the config file are resolved into runtime state. clap already applies
    /// flag > env precedence for every value carrying an `env = ...` attribute
    /// (URLs, OTP); this builder only validates those values and layers in the
    /// pieces clap cannot express: the stdin/file secret sources and the
    /// credential precedence (flag > env > stored key) in [`kraken_auth`].
    pub fn from_cli(cli: &Cli) -> Result<Self> {
        Self::from_globals(&cli.globals)
    }

    /// Assemble the runtime context from the global options alone.
    ///
    /// The entry every product shares: `kraken-shell` and `kraken-mcp` have no
    /// command tree to parse, only the same flags pointing at the same account.
    pub fn from_globals(globals: &GlobalArgs) -> Result<Self> {
        // One-shot: clap has already applied flag > env for the scope names, so
        // the flag-vs-env distinction is irrelevant here.
        Self::build(globals, None, LineFlags::default())
    }

    /// Assemble the runtime context for a single REPL line, falling back to
    /// `base` (the session context) for anything the line did not set.
    ///
    /// Routes through the exact same resolution as [`from_cli`], so per-line
    /// behaviour — including URL scheme **and host-allowlist** enforcement —
    /// matches one-shot invocations rather than diverging from them.
    ///
    /// `line` flags indicate which global flags were explicitly set on this
    /// line (clap's `ValueSource::CommandLine`) rather than inherited from
    /// the session context. The shell's environment is constant, so an
    /// env-sourced value is already baked into `base`; letting it
    /// re-resolve per line would override an explicit session start flag.
    /// See [`build`].
    ///
    /// [`from_cli`]: AppContext::from_cli
    /// [`build`]: AppContext::build
    pub fn from_cli_with_fallback(cli: &Cli, base: &AppContext, line: LineFlags) -> Result<Self> {
        Self::build(&cli.globals, Some(base), line)
    }

    /// Shared resolution core. With `base = None` this is a one-shot
    /// invocation; with `base = Some(shell)` it is a REPL line layered over
    /// the shell's startup context.
    ///
    /// `line` contains which global flags were explicitly set on this line; see
    /// [`from_cli_with_fallback`](AppContext::from_cli_with_fallback).
    fn build(cli: &GlobalArgs, base: Option<&AppContext>, line: LineFlags) -> Result<Self> {
        let format = cli
            .interactive
            .output
            .unwrap_or_else(|| base.map_or(OutputFormat::Table, |b| b.format));

        // The API secret source. A one-shot invocation may read it from stdin or
        // a file; in the REPL only the inline flag is honoured (stdin is the line
        // reader) and resolution falls back to the session secret.
        let api_secret: Option<SecretString> = match base {
            None => resolve_secret_source(cli)?,
            Some(b) => cli
                .credentials
                .api_secret
                .clone()
                .map(SecretString::from)
                .or_else(|| b.api_secret.clone()),
        };

        // Transport "danger" toggles come from clap (flag or env, leniently
        // parsed). In the REPL they are sticky: once a session enables a toggle
        // it stays enabled, and per-line flags can only add to it.
        let allow_any_url_host =
            cli.connection.danger_allow_any_url_host || base.is_some_and(|b| b.allow_any_url_host);
        let accept_invalid_certs = cli.connection.danger_accept_invalid_certs
            || base.is_some_and(|b| b.accept_invalid_certs);
        // Warned here, where the flag is resolved: this target passes the default
        // stderr filter (`kraken_app=warn`), so the safety notice survives log
        // configurations that would filter the transport crates' own warnings out.
        if accept_invalid_certs {
            tracing::warn!(
                "TLS certificate verification is DISABLED. Connections are vulnerable to \
                 man-in-the-middle attacks. Do NOT use this in production."
            );
        }

        // URL overrides: clap has already applied flag > env precedence, so here
        // we only validate scheme/host, then fall back to the session's resolved
        // value when the line set nothing. Errors are tagged with the originating
        // flag/env names so the user knows which value to fix.
        let resolve_url =
            |value: Option<&str>, label: &str, fallback: Option<&str>| -> Result<Option<String>> {
                let resolved = url_policy::resolve_url_override(value, allow_any_url_host)
                    .map_err(|e| e.labelled(label))?;
                Ok(resolved.or_else(|| fallback.map(str::to_owned)))
            };

        let api_url = resolve_url(
            cli.connection.api_url.as_deref(),
            "--api-url / KRAKEN_SPOT_URL",
            base.map(|b| b.spot().base_url()),
        )?;
        let futures_url = resolve_url(
            cli.connection.futures_url.as_deref(),
            "--futures-url / KRAKEN_FUTURES_URL",
            base.map(|b| b.futures().base_url()),
        )?;
        let ws_public_url = resolve_url(
            cli.connection.ws_public_url.as_deref(),
            "--ws-public-url / KRAKEN_WS_PUBLIC_URL",
            base.and_then(|b| b.ws_public_url.as_deref()),
        )?;
        let ws_auth_url = resolve_url(
            cli.connection.ws_auth_url.as_deref(),
            "--ws-auth-url / KRAKEN_WS_AUTH_URL",
            base.and_then(|b| b.ws_auth_url.as_deref()),
        )?;
        let ws_l3_url = resolve_url(
            cli.connection.ws_l3_url.as_deref(),
            "--ws-l3-url / KRAKEN_WS_L3_URL",
            base.and_then(|b| b.ws_l3_url.as_deref()),
        )?;
        // The private futures feed authenticates over this socket, so its host
        // override takes the same trusted-host gate as every other URL.
        let ws_futures_url = resolve_url(
            cli.connection.ws_futures_url.as_deref(),
            "--ws-futures-url / KRAKEN_FUTURES_WS_URL",
            base.and_then(|b| b.ws_futures_url.as_deref()),
        )?;
        let oauth = OAuthSettings {
            // An empty value, as a plugin host passes for a blank field, is unset.
            client_id: cli
                .connection
                .oauth_client_id
                .clone()
                .filter(|id| !id.is_empty())
                .or_else(|| base.and_then(|b| b.oauth.client_id.clone())),
            authorize_url: cli
                .connection
                .oauth_authorize_url
                .clone()
                .filter(|url| !url.is_empty())
                .or_else(|| base.and_then(|b| b.oauth.authorize_url.clone())),
        };

        // clap reads the flags and the env vars; `kraken_auth` applies their
        // precedence and `Venue::resolve` the session's and the stored key's.
        let flag = Supplied {
            key: cli.credentials.api_key.clone(),
            secret: api_secret.clone(),
        };
        let spot_env = Supplied {
            key: cli.connection.spot_api_key.clone(),
            secret: cli
                .connection
                .spot_api_secret
                .clone()
                .map(SecretString::from),
        };
        let spot_env_key = spot_env.is_complete();
        let futures_env = Supplied {
            key: cli.connection.futures_api_key.clone(),
            secret: cli
                .connection
                .futures_api_secret
                .clone()
                .map(SecretString::from),
        };
        let stored = config::StoredKeys::default();

        // One pool serves both venues. A REPL line keeps the session's unless it
        // turns the cert toggle on, since rebuilding per line would discard it.
        let transport = match base {
            Some(b) if b.accept_invalid_certs == accept_invalid_certs => {
                b.spot().transport().clone()
            }
            _ => kraken_rest::Transport::new(&rest_options(accept_invalid_certs))?,
        };
        let otp = cli
            .connection
            .otp
            .clone()
            .map(SecretString::from)
            .or_else(|| base.and_then(|b| b.otp.clone()));
        let spot = Venue::resolve(
            SpotRest::new(transport.clone(), api_url),
            kraken_auth::resolve_supplied(Engine::Spot, flag.clone(), spot_env),
            base.map(|b| &b.spot),
            &stored,
        )
        .with_otp(otp.clone());
        let futures = Venue::resolve(
            FuturesRest::new(transport, futures_url),
            kraken_auth::resolve_supplied(Engine::Futures, flag, futures_env),
            base.map(|b| &b.futures),
            &stored,
        );

        Ok(Self {
            format,
            ws_public_url,
            ws_auth_url,
            ws_l3_url,
            ws_futures_url,
            oauth,
            ws_reconnect_base_ms: cli
                .connection
                .ws_reconnect_base_ms
                .or_else(|| base.and_then(|b| b.ws_reconnect_base_ms)),
            api_secret,
            // Blank normalizes to "no workspace" (the real account); the
            // reserved `default` collapses the same way. REPL lines are
            // sticky: an explicit per-line `--workspace` is authoritative
            // for that line; a line without the flag inherits the startup
            // scope ($KRAKEN_WORKSPACE is constant for the shell and already
            // reflected in `base`).
            workspace: match base {
                None => normalize_workspace(cli.connection.workspace.clone()),
                Some(_) if line.workspace => normalize_workspace(cli.connection.workspace.clone()),
                Some(b) => b.workspace.clone(),
            },
            force: cli.interactive.yes || base.is_some_and(|b| b.force),
            accept_invalid_certs,
            allow_any_url_host,
            mcp_mode: false,
            // Enabled by `--monitor`; in the REPL it is sticky, falling back to the
            // session's setting when a line doesn't pass the flag (like `verbose`).
            monitor: if cli.monitor.monitor {
                Some(crate::monitor::MonitorConfig::new(
                    cli.monitor.monitor_health_interval,
                    cli.monitor.monitor_stale_after,
                    cli.monitor.monitor_heartbeat_timeout,
                ))
            } else {
                base.and_then(|b| b.monitor.clone())
            },
            spot,
            futures,
            otp,
            spot_env_key,
        })
    }
}

/// The transport inputs both REST venues share.
pub(crate) fn rest_options(accept_invalid_certs: bool) -> kraken_rest::ClientOptions {
    kraken_rest::ClientOptions {
        tls: if accept_invalid_certs {
            kraken_rest::Tls::DangerAcceptInvalidCerts
        } else {
            kraken_rest::Tls::Verified
        },
        identity: kraken_rest::Identity {
            user_agent: crate::telemetry::user_agent().to_owned(),
            headers: crate::telemetry::rest_identity_headers()
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
        },
    }
}

/// The API secret from its one-shot sources: stdin > file > inline flag.
fn resolve_secret_source(cli: &GlobalArgs) -> Result<Option<SecretString>> {
    Ok(kraken_auth::read_secret(kraken_auth::SecretSource {
        stdin: cli.credentials.api_secret_stdin,
        file: cli.credentials.api_secret_file.clone(),
        inline: cli.credentials.api_secret.clone().map(SecretString::from),
        inline_flag: "--api-secret",
    })?)
}

/// Which account, which endpoints, and how loud — the one question every
/// kraken product must answer the same way.
#[derive(Debug, clap::Args)]
pub struct ConnectionArgs {
    /// Show request/response details on stderr.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Diagnostic log format on stderr [default: json]. JSON (JSONL) is the
    /// machine-parseable default everywhere; use `pretty` (or `compact`) for
    /// interactive work. Independent of `-o`/`--output`, which formats the
    /// command result on stdout. Use `RUST_LOG` (or `-v`) to control the level.
    #[arg(long, value_enum, global = true, env = "KRAKEN_LOG_FORMAT")]
    pub log_format: Option<crate::logging::LogFormat>,

    /// Override Spot API base URL.
    #[arg(long, global = true, env = "KRAKEN_SPOT_URL")]
    pub api_url: Option<String>,

    /// Override Futures API base URL.
    #[arg(long, global = true, env = "KRAKEN_FUTURES_URL")]
    pub futures_url: Option<String>,

    /// Override public WebSocket URL.
    #[arg(long, global = true, env = "KRAKEN_WS_PUBLIC_URL")]
    pub ws_public_url: Option<String>,

    /// Override authenticated WebSocket URL.
    #[arg(long, global = true, env = "KRAKEN_WS_AUTH_URL")]
    pub ws_auth_url: Option<String>,

    /// Override L3 (authenticated) WebSocket URL.
    #[arg(long, global = true, env = "KRAKEN_WS_L3_URL")]
    pub ws_l3_url: Option<String>,

    /// Override Futures WebSocket URL.
    #[arg(long, global = true, env = "KRAKEN_FUTURES_WS_URL")]
    pub ws_futures_url: Option<String>,

    /// The OAuth client ID for `auth login`.
    #[arg(long, global = true, env = "KRAKEN_OAUTH_CLIENT_ID", hide = true)]
    pub oauth_client_id: Option<String>,

    /// Override the Kraken sign-in page that `auth login` opens.
    #[arg(long, global = true, env = "KRAKEN_OAUTH_AUTHORIZE_URL", hide = true)]
    pub oauth_authorize_url: Option<String>,

    /// Base WebSocket reconnect backoff in milliseconds (testing/tuning knob;
    /// compresses the backoff schedule). Defaults to the production schedule.
    /// Zero is rejected: multiplicative backoff has a fixed point at zero, so a
    /// 0 base never escalates and hammers the venue with sub-second dials
    /// forever.
    #[arg(long, global = true, env = "KRAKEN_WS_RECONNECT_BASE_MS", hide = true,
          value_parser = clap::value_parser!(u64).range(1..))]
    pub ws_reconnect_base_ms: Option<u64>,

    // Per-engine credential environment variables. clap owns the env read; the
    // values feed the env tier of the credential precedence in `kraken_auth`. These
    // are hidden because the supported entry points are the env vars themselves
    // and the shared `--api-key`/`--api-secret` flags above — not per-engine
    // flags. Spot and Futures use distinct env vars, which the single shared
    // flag pair cannot express, so they are surfaced as separate fields here.
    /// Spot API key (env: KRAKEN_API_KEY).
    #[arg(
        long = "spot-api-key",
        env = "KRAKEN_API_KEY",
        hide = true,
        global = true
    )]
    pub spot_api_key: Option<String>,

    /// Spot API secret (env: KRAKEN_API_SECRET).
    #[arg(
        long = "spot-api-secret",
        env = "KRAKEN_API_SECRET",
        hide = true,
        global = true
    )]
    pub spot_api_secret: Option<String>,

    /// Futures API key (env: KRAKEN_FUTURES_API_KEY).
    #[arg(
        long = "futures-api-key",
        env = "KRAKEN_FUTURES_API_KEY",
        hide = true,
        global = true
    )]
    pub futures_api_key: Option<String>,

    /// Futures API secret (env: KRAKEN_FUTURES_API_SECRET).
    #[arg(
        long = "futures-api-secret",
        env = "KRAKEN_FUTURES_API_SECRET",
        hide = true,
        global = true
    )]
    pub futures_api_secret: Option<String>,

    /// OTP (two-factor authentication code).
    #[arg(long, global = true, env = "KRAKEN_OTP")]
    pub otp: Option<String>,

    /// Workspace name. Scopes the paper account, its sessions, and lab
    /// experiments to an isolated `workspaces/<name>/` tree; recorded market
    /// data stays global. Unset (or `default`) uses the flat layout. Must be a
    /// single path-safe segment: letters, digits, '.', '-', or '_' (no '/',
    /// spaces, or '.'/'..').
    #[arg(long, global = true, env = "KRAKEN_WORKSPACE")]
    pub workspace: Option<String>,

    /// DANGEROUS: disable TLS certificate verification for HTTP and WebSocket
    /// connections. Exposes traffic to MITM; for local testing only.
    #[arg(
        long,
        global = true,
        env = "KRAKEN_DANGER_ACCEPT_INVALID_CERTS",
        num_args = 0..=1,
        default_value_t = false,
        default_missing_value = "true",
        value_parser = parse_danger_toggle,
    )]
    pub danger_accept_invalid_certs: bool,

    /// DANGEROUS: allow non-Kraken hosts in URL overrides (e.g. for test or
    /// self-hosted environments).
    #[arg(
        long,
        global = true,
        env = "KRAKEN_DANGER_ALLOW_ANY_URL_HOST",
        num_args = 0..=1,
        default_value_t = false,
        default_missing_value = "true",
        value_parser = parse_danger_toggle,
    )]
    pub danger_allow_any_url_host: bool,
}

/// Options that only mean something where a human reads the result and can
/// answer a prompt. `kraken-mcp` overrides both, so it does not offer them.
#[derive(Debug, Default, clap::Args)]
pub struct InteractiveArgs {
    /// Output format: table (default) or json.
    #[arg(short, long, value_enum, global = true)]
    pub output: Option<OutputFormat>,

    /// Skip confirmation prompts for destructive operations.
    #[arg(long, alias = "force", global = true)]
    pub yes: bool,
}

/// Stream-reliability reporting for the long-running commands (`ws`,
/// `streamd`, `record`). Only a product that can run one accepts these.
#[derive(Debug, Default, clap::Args)]
pub struct MonitorArgs {
    /// Emit live stream-reliability diagnostics (latency, trade-id gaps,
    /// staleness, throughput, connection health) to stderr while streaming.
    /// Applies to `ws`, `streamd`, and `record`; ignored by other commands.
    #[arg(long, global = true)]
    pub monitor: bool,

    /// Seconds between periodic monitor `health` snapshots (requires
    /// --monitor).
    #[arg(long, global = true, default_value_t = 10, requires = "monitor")]
    pub monitor_health_interval: u64,

    /// Soft alert after this many seconds with no market data (requires
    /// --monitor).
    #[arg(long, global = true, default_value_t = 30, requires = "monitor")]
    pub monitor_stale_after: u64,

    /// Hard alert after this many seconds with no heartbeat — connection-level
    /// liveness (requires --monitor).
    #[arg(long, global = true, default_value_t = 5, requires = "monitor")]
    pub monitor_heartbeat_timeout: u64,
}

/// The flag-tier credential override, and the three mutually exclusive ways to
/// supply the secret.
///
/// `kraken` alone. `--api-secret-stdin` would eat a shell's prompt or a
/// server's protocol channel, and clap's exclusivity cannot span a struct those
/// products do not flatten — so the group travels together. They read
/// credentials from the environment and the stored config instead, which is the
/// recommended path for anything long-running anyway.
#[derive(Debug, Default, clap::Args)]
pub struct CredentialArgs {
    /// API key override (takes precedence over env/config).
    #[arg(long, global = true)]
    pub api_key: Option<String>,

    /// API secret override (prefer --api-secret-stdin for security).
    #[arg(long, global = true)]
    pub api_secret: Option<String>,

    /// Path to file containing API secret (mutually exclusive with --api-secret
    /// and --api-secret-stdin).
    #[arg(long, global = true, conflicts_with_all = ["api_secret", "api_secret_stdin"])]
    pub api_secret_file: Option<std::path::PathBuf>,

    /// Read API secret from stdin (mutually exclusive with --api-secret and
    /// --api-secret-file).
    #[arg(long, global = true, conflicts_with_all = ["api_secret", "api_secret_file"])]
    pub api_secret_stdin: bool,
}

/// Every option the engine reads, as one value.
///
/// The shared core enriched with the layers a product offers: `kraken` has all
/// of them, `kraken-shell` all but the stdin secret, `kraken-mcp` only
/// [`ConnectionArgs`]. A flag a product would ignore is one it never accepts.
#[derive(Debug, clap::Args)]
pub struct GlobalArgs {
    #[command(flatten)]
    pub connection: ConnectionArgs,

    #[command(flatten)]
    pub interactive: InteractiveArgs,

    #[command(flatten)]
    pub monitor: MonitorArgs,

    #[command(flatten)]
    pub credentials: CredentialArgs,
}

impl GlobalArgs {
    /// Enrich the shared options with the layers a product offers. What it
    /// omits stays at its default, which is what "this product does not offer
    /// that flag" means to the engine.
    #[must_use]
    pub fn new(
        connection: ConnectionArgs,
        interactive: InteractiveArgs,
        monitor: MonitorArgs,
    ) -> Self {
        Self {
            connection,
            interactive,
            monitor,
            credentials: CredentialArgs::default(),
        }
    }

    /// The shared options alone — a product that offers no other layer.
    #[must_use]
    pub fn connection_only(connection: ConnectionArgs) -> Self {
        Self::new(
            connection,
            InteractiveArgs::default(),
            MonitorArgs::default(),
        )
    }
}

/// The `kraken` command tree and its global flags.
#[derive(Parser)]
// A bare `about` would print kraken-app's Cargo description, not the product's.
#[command(
    name = "kraken",
    version,
    about = "Kraken CLI: trade, query and manage your Kraken account from the terminal",
    long_about = None
)]
pub struct Cli {
    #[command(flatten)]
    pub globals: GlobalArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_destructive_without_tty_names_the_yes_flag() {
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            // Interactive `cargo test` would hit the real prompt and hang;
            // the branch under test only exists without a TTY.
            return;
        }
        let cli = Cli::try_parse_from(["kraken"]).expect("bare invocation parses");
        let ctx = AppContext::from_cli(&cli).expect("default context resolves");
        let err = ctx.confirm_destructive("drop it?").unwrap_err();
        assert!(
            matches!(err, errors::KrakenError::Validation(_)),
            "got: {err}"
        );
        assert!(err.to_string().contains("--yes"), "got: {err}");
    }

    #[test]
    fn a_repl_line_keeps_the_sessions_urls() {
        let cli = Cli::try_parse_from(["kraken", "--api-url", "https://api.kraken.com/"])
            .expect("session with URL override parses");
        let base = AppContext::from_cli(&cli).expect("session context resolves");

        let line = Cli::try_parse_from(["kraken"]).expect("bare line parses");
        let derived = AppContext::from_cli_with_fallback(&line, &base, LineFlags::default())
            .expect("fallback context resolves");

        assert_eq!(derived.spot().base_url(), base.spot().base_url());
        assert_eq!(derived.futures().base_url(), base.futures().base_url());
    }

    #[test]
    fn a_repl_line_overrides_the_sessions_url() {
        let cli = Cli::try_parse_from(["kraken"]).expect("bare invocation parses");
        let base = AppContext::from_cli(&cli).expect("default context resolves");

        let line = Cli::try_parse_from(["kraken", "--api-url", "https://api.kraken.com/x"])
            .expect("line with URL override parses");
        let derived = AppContext::from_cli_with_fallback(&line, &base, LineFlags::default())
            .expect("fallback context resolves");

        assert_eq!(derived.spot().base_url(), "https://api.kraken.com/x");
    }

    #[test]
    fn futures_ws_url_override_rejects_untrusted_host() {
        // The explicit `false` pins the toggle regardless of the ambient
        // environment, which may export it.
        let cli = Cli::try_parse_from([
            "kraken",
            "--danger-allow-any-url-host",
            "false",
            "--ws-futures-url",
            "wss://evil.example.com/ws/v1",
        ])
        .expect("line with futures WS override parses");
        let Err(err) = AppContext::from_cli(&cli) else {
            panic!("untrusted futures WS host must not resolve");
        };
        assert!(err.to_string().contains("Untrusted URL host"), "got: {err}");
        assert!(err.to_string().contains("--ws-futures-url"), "got: {err}");
    }

    #[test]
    fn normalize_workspace_treats_blank_as_unset() {
        // An exported-but-empty `KRAKEN_WORKSPACE` (clap yields `Some("")`)
        // and a whitespace-only value must both degrade to the real account,
        // not an invalid-name error.
        assert_eq!(normalize_workspace(Some(String::new())), None);
        assert_eq!(normalize_workspace(Some("   ".to_string())), None);
        assert_eq!(normalize_workspace(Some("\t\n".to_string())), None);
        assert_eq!(normalize_workspace(None), None);
    }

    #[test]
    fn normalize_workspace_collapses_blank_and_the_reserved_default() {
        // Blank degrades like a session; `default` is the alias-in-place
        // workspace, so it must resolve exactly like "no workspace".
        assert_eq!(normalize_workspace(Some(String::new())), None);
        assert_eq!(normalize_workspace(Some("default".to_string())), None);
        assert_eq!(
            normalize_workspace(Some("btc-momentum".to_string())),
            Some("btc-momentum".to_string())
        );
    }
}
