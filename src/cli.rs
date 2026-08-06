//! CLI parsing and runtime-context resolution.
//!
//! [`AppContext::from_cli`] centralizes precedence across flags, environment,
//! and config so every command sees the same resolved state.

use clap::Parser;
use secrecy::{ExposeSecret, SecretString};

use crate::commands::Command;
use crate::errors::{self, Result};
use crate::output::OutputFormat;
use crate::{client, config};

/// Parse a boolean "danger" toggle leniently: `1`, `true`, or `yes`
/// (case-insensitive, surrounding whitespace ignored) enable it; anything else —
/// including an empty value or an unrecognized string — leaves it disabled.
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

/// Runtime context assembled from global CLI flags and config.
pub struct AppContext {
    pub format: OutputFormat,
    pub api_url: Option<String>,
    pub futures_url: Option<String>,
    pub ws_public_url: Option<String>,
    pub ws_auth_url: Option<String>,
    pub ws_l3_url: Option<String>,
    pub ws_futures_url: Option<String>,
    /// Base reconnect backoff (ms) for spot WebSocket streams; compresses the
    /// backoff schedule when set. Resolved once here so the WS layer never reads
    /// the environment.
    pub ws_reconnect_base_ms: Option<u64>,
    /// Resolved Spot credentials (flag > env > config), or `None` if unset.
    pub spot_credentials: Option<config::Credentials>,
    /// Resolved Futures credentials (flag > env > config), or `None` if unset.
    pub futures_credentials: Option<config::Credentials>,
    /// API secret source (stdin/file/flag), retained as a secret so `auth set`
    /// can persist it. Authentication uses `spot_credentials` /
    /// `futures_credentials`; this is the flag-tier secret that feeds them.
    pub api_secret: Option<SecretString>,
    /// OTP second factor, as a secret so the derived `Debug` redacts it — in
    /// static-password 2FA mode it is as durable as the API secret.
    pub otp: Option<SecretString>,
    /// Workspace name (resolved `--workspace` > `$KRAKEN_WORKSPACE`), or
    /// `None` for the flat layout. The reserved `default` collapses to `None`
    /// at resolution. Carries the raw name only; validation and the existence
    /// gate run in [`scoped_base`](Self::scoped_base), the single point where
    /// scoped commands resolve their state root.
    pub workspace: Option<String>,
    pub force: bool,
    /// Disable TLS certificate verification for HTTP and WebSocket transport.
    /// Resolved once here from `KRAKEN_DANGER_ACCEPT_INVALID_CERTS` and injected
    /// into the clients/WS layer so no transport code reads the environment.
    pub accept_invalid_certs: bool,
    /// Whether non-Kraken hosts are permitted in URL overrides. Retained so the
    /// REPL can re-validate per-line URL overrides under the same policy the
    /// session was started with (see `AppContext::from_cli_with_fallback`).
    pub allow_any_url_host: bool,
    /// True when running in MCP server mode. Disables all interactive prompts.
    pub mcp_mode: bool,
    /// Live stream-reliability monitor configuration, or `None` when `--monitor` is
    /// unset. Consumed by [`stream::run`](crate::stream::run) for `ws`/`streamd`/
    /// `record`; ignored by everything else.
    pub(crate) monitor: Option<crate::monitor::MonitorConfig>,
    /// Memoized HTTP clients, built once on first use. A `reqwest::Client` owns a
    /// connection pool; rebuilding one per command discards keep-alive reuse. This
    /// matters for the long-lived MCP server, which shares one `AppContext` across
    /// every tool call (`run_server`). Accessed via [`spot`](Self::spot) /
    /// [`futures`](Self::futures). The spot client is held behind an `Arc` so the
    /// WS token source shares the same pool instead of building its own.
    pub(crate) spot_client: std::sync::OnceLock<std::sync::Arc<client::SpotClient>>,
    pub(crate) futures_client: std::sync::OnceLock<client::FuturesClient>,
}

/// Memoize a fallibly-built value: return the cached one, or build and cache.
/// On a benign race two values may be built; `get_or_init` drops the loser.
fn memoized<T>(slot: &std::sync::OnceLock<T>, build: impl FnOnce() -> Result<T>) -> Result<&T> {
    if let Some(v) = slot.get() {
        return Ok(v);
    }
    let built = build()?;
    Ok(slot.get_or_init(|| built))
}

impl AppContext {
    /// This context, re-scoped to another workspace — exactly what a
    /// `--workspace <name>` line would have resolved, sharing the memoized
    /// clients. For commands that thread a scope explicitly (the playground
    /// demo) instead of asking the user to export it first.
    pub(crate) fn rescope(&self, workspace: Option<String>) -> Self {
        Self {
            format: self.format,
            api_url: self.api_url.clone(),
            futures_url: self.futures_url.clone(),
            ws_public_url: self.ws_public_url.clone(),
            ws_auth_url: self.ws_auth_url.clone(),
            ws_l3_url: self.ws_l3_url.clone(),
            ws_futures_url: self.ws_futures_url.clone(),
            ws_reconnect_base_ms: self.ws_reconnect_base_ms,
            spot_credentials: self.spot_credentials.clone(),
            futures_credentials: self.futures_credentials.clone(),
            api_secret: self.api_secret.clone(),
            otp: self.otp.clone(),
            workspace,
            force: self.force,
            accept_invalid_certs: self.accept_invalid_certs,
            allow_any_url_host: self.allow_any_url_host,
            mcp_mode: self.mcp_mode,
            monitor: None,
            spot_client: self.spot_client.clone(),
            futures_client: self.futures_client.clone(),
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

    /// Borrow the memoized Spot client, building it once on first use.
    ///
    /// The client depends only on `api_url` and `accept_invalid_certs`, both
    /// immutable for the lifetime of the context, so a single build is always
    /// correct.
    pub(crate) fn spot(&self) -> Result<&client::SpotClient> {
        Ok(self.spot_shared()?.as_ref())
    }

    /// The memoized Spot client as an owned handle, for the WS token source —
    /// the same instance [`spot`](Self::spot) borrows, never a second pool.
    fn spot_shared(&self) -> Result<&std::sync::Arc<client::SpotClient>> {
        memoized(&self.spot_client, || {
            client::SpotClient::new(self.api_url.as_deref(), self.accept_invalid_certs)
                .map(std::sync::Arc::new)
        })
    }

    /// Borrow the memoized Futures client, building it once on first use.
    pub(crate) fn futures(&self) -> Result<&client::FuturesClient> {
        memoized(&self.futures_client, || {
            client::FuturesClient::new(self.futures_url.as_deref(), self.accept_invalid_certs)
        })
    }

    /// Client, credentials, and optional OTP for an authenticated Spot command.
    /// The OTP stays wrapped; it is exposed only inside `private_post*` request
    /// construction.
    pub(crate) fn spot_authed(
        &self,
    ) -> Result<(
        &client::SpotClient,
        &config::Credentials,
        Option<&SecretString>,
    )> {
        let creds = self.require_credentials(config::Engine::Spot)?;
        Ok((self.spot()?, creds, self.otp.as_ref()))
    }

    /// Client + credentials for an authenticated Futures command.
    ///
    /// Credentials resolve before the client so a missing-key setup always
    /// surfaces the actionable auth error, never a client-construction failure.
    pub(crate) fn futures_authed(&self) -> Result<(&client::FuturesClient, &config::Credentials)> {
        let creds = self.require_credentials(config::Engine::Futures)?;
        Ok((self.futures()?, creds))
    }

    /// The WS client config — URL overrides, TLS switch, token source, handshake
    /// headers — projected from the resolved context, beside the REST factories.
    /// `token_source` is `None` without spot credentials (public-only use; private
    /// endpoints then fail closed inside the client).
    pub(crate) fn ws_config(&self) -> Result<kraken_ws::WsConfig> {
        Ok(kraken_ws::WsConfig {
            urls: kraken_ws::Urls {
                public: self.ws_public_url.clone(),
                auth: self.ws_auth_url.clone(),
                l3: self.ws_l3_url.clone(),
            },
            accept_invalid_certs: self.accept_invalid_certs,
            token_source: self.ws_token_source()?,
            // The transport crate takes the identity headers as data; it knows
            // nothing about this binary's identity.
            headers: crate::telemetry::ws_handshake_headers()
                .into_iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        })
    }

    /// The WS reconnect schedule, with the `--ws-reconnect-base-ms` /
    /// `KRAKEN_WS_RECONNECT_BASE_MS` knob folded into `initial` (the rest keeps its
    /// defaults). A base above the default cap is honoured: the backoff raises its
    /// effective `max` to `initial`.
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

    /// The live token source, or `None` when no spot credentials are configured.
    fn ws_token_source(&self) -> Result<Option<std::sync::Arc<dyn kraken_ws::TokenSource>>> {
        use std::sync::Arc;
        let Ok(credentials) = self.require_credentials(config::Engine::Spot) else {
            return Ok(None);
        };
        let credentials = credentials.clone();
        let spot = Arc::clone(self.spot_shared()?);
        let source = client::SpotTokenSource::new(spot, Arc::new(credentials), self.otp.clone());
        Ok(Some(Arc::new(source)))
    }

    /// Borrow resolved credentials for `engine`, or the standard "not
    /// configured" error naming that engine's env vars.
    pub(crate) fn require_credentials(
        &self,
        engine: config::Engine,
    ) -> Result<&config::Credentials> {
        let resolved = match engine {
            config::Engine::Spot => &self.spot_credentials,
            config::Engine::Futures => &self.futures_credentials,
        };
        resolved.as_ref().ok_or_else(|| {
            let (key_env, secret_env) = engine.env_names();
            errors::KrakenError::Auth(format!(
                "No {} API credentials found. Use `kraken auth set` or set {key_env} / {secret_env} env vars.",
                engine.label()
            ))
        })
    }

    /// Assemble the runtime context for a one-shot CLI invocation.
    ///
    /// This is the single place where global flags, environment variables, and
    /// the config file are resolved into runtime state. clap already applies
    /// flag > env precedence for every value carrying an `env = ...` attribute
    /// (URLs, OTP); this builder only validates those values and layers in the
    /// pieces clap cannot express: the stdin/file secret sources and the
    /// three-tier (flag > env > config) credential resolver in [`config`].
    pub fn from_cli(cli: &Cli) -> Result<Self> {
        // One-shot: clap has already applied flag > env for the scope names, so
        // the flag-vs-env distinction is irrelevant here.
        Self::build(cli, None, false)
    }

    /// Assemble the runtime context for a single REPL line, falling back to
    /// `base` (the session context) for anything the line did not set.
    ///
    /// Routes through the exact same resolution as [`from_cli`], so per-line
    /// behaviour — including URL scheme **and host-allowlist** enforcement —
    /// matches one-shot invocations rather than diverging from them.
    ///
    /// `workspace_from_flag` must be `true` only when `--workspace` was typed
    /// on this line (clap's `ValueSource::CommandLine`), not merely inherited
    /// from `$KRAKEN_WORKSPACE`. The shell's environment is constant, so an
    /// env-sourced workspace is already baked into `base`; letting it
    /// re-resolve per line would override an explicit `kraken shell
    /// --workspace`. See [`build`].
    ///
    /// [`from_cli`]: AppContext::from_cli
    /// [`build`]: AppContext::build
    pub(crate) fn from_cli_with_fallback(
        cli: &Cli,
        base: &AppContext,
        workspace_from_flag: bool,
    ) -> Result<Self> {
        Self::build(cli, Some(base), workspace_from_flag)
    }

    /// Shared resolution core. With `base = None` this is a one-shot invocation;
    /// with `base = Some(shell)` it is a REPL line layered over the shell's
    /// startup context.
    ///
    /// `workspace_from_flag` is consulted only in the REPL case; see
    /// [`from_cli_with_fallback`](AppContext::from_cli_with_fallback).
    fn build(cli: &Cli, base: Option<&AppContext>, workspace_from_flag: bool) -> Result<Self> {
        let format = cli
            .output
            .unwrap_or_else(|| base.map_or(OutputFormat::Table, |b| b.format));

        // The API secret source. A one-shot invocation may read it from stdin or
        // a file; in the REPL only the inline flag is honoured (stdin is the line
        // reader) and resolution falls back to the session secret.
        let api_secret: Option<SecretString> = match base {
            None => resolve_secret_source(cli)?,
            Some(b) => cli
                .api_secret
                .clone()
                .map(SecretString::from)
                .or_else(|| b.api_secret.clone()),
        };

        // Transport "danger" toggles come from clap (flag or env, leniently
        // parsed). In the REPL they are sticky: once a session enables a toggle
        // it stays enabled, and per-line flags can only add to it.
        let allow_any_url_host =
            cli.danger_allow_any_url_host || base.is_some_and(|b| b.allow_any_url_host);
        let accept_invalid_certs =
            cli.danger_accept_invalid_certs || base.is_some_and(|b| b.accept_invalid_certs);
        // Warned here, where the flag is resolved: this target passes the default
        // stderr filter (`kraken_cli=warn`), so the safety notice survives log
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
        let resolve_url = |value: Option<&str>,
                           label: &str,
                           fallback: Option<&String>|
         -> Result<Option<String>> {
            let resolved =
                client::resolve_url_override(value, allow_any_url_host).map_err(|e| {
                    let inner = match &e {
                        errors::KrakenError::Validation(msg) => msg.clone(),
                        other => other.to_string(),
                    };
                    errors::KrakenError::Validation(format!("{label}: {inner}"))
                })?;
            Ok(resolved.or_else(|| fallback.cloned()))
        };

        let api_url = resolve_url(
            cli.api_url.as_deref(),
            "--api-url / KRAKEN_SPOT_URL",
            base.and_then(|b| b.api_url.as_ref()),
        )?;
        let futures_url = resolve_url(
            cli.futures_url.as_deref(),
            "--futures-url / KRAKEN_FUTURES_URL",
            base.and_then(|b| b.futures_url.as_ref()),
        )?;
        let ws_public_url = resolve_url(
            cli.ws_public_url.as_deref(),
            "--ws-public-url / KRAKEN_WS_PUBLIC_URL",
            base.and_then(|b| b.ws_public_url.as_ref()),
        )?;
        let ws_auth_url = resolve_url(
            cli.ws_auth_url.as_deref(),
            "--ws-auth-url / KRAKEN_WS_AUTH_URL",
            base.and_then(|b| b.ws_auth_url.as_ref()),
        )?;
        let ws_l3_url = resolve_url(
            cli.ws_l3_url.as_deref(),
            "--ws-l3-url / KRAKEN_WS_L3_URL",
            base.and_then(|b| b.ws_l3_url.as_ref()),
        )?;
        // The private futures feed authenticates over this socket, so its host
        // override takes the same trusted-host gate as every other URL.
        let ws_futures_url = resolve_url(
            cli.ws_futures_url.as_deref(),
            "--ws-futures-url / KRAKEN_FUTURES_WS_URL",
            base.and_then(|b| b.ws_futures_url.as_ref()),
        )?;

        // Credentials keep their dedicated three-tier (flag > env > config)
        // resolver, then fall back to the session's already-resolved credentials.
        // clap reads every env var (shared --api-key/--api-secret flags plus the
        // per-engine env vars); the resolver only applies precedence and the
        // config-file tier, so it no longer touches the environment itself.
        let flag_secret = api_secret.as_ref().map(|s| s.expose_secret());
        let spot_credentials = config::resolve_credentials(
            config::Engine::Spot,
            cli.api_key.as_deref(),
            flag_secret,
            cli.spot_api_key.as_deref(),
            cli.spot_api_secret.as_deref(),
        )
        .or_else(|| base.and_then(|b| b.spot_credentials.clone()));
        let futures_credentials = config::resolve_credentials(
            config::Engine::Futures,
            cli.api_key.as_deref(),
            flag_secret,
            cli.futures_api_key.as_deref(),
            cli.futures_api_secret.as_deref(),
        )
        .or_else(|| base.and_then(|b| b.futures_credentials.clone()));

        // A REPL line that overrides the URL or the cert toggle must build its
        // own client; otherwise the base's already-built client is carried over,
        // since rebuilding per line would discard its connection pool.
        let spot_client = base
            .filter(|b| b.api_url == api_url && b.accept_invalid_certs == accept_invalid_certs)
            .and_then(|b| b.spot_client.get().cloned())
            .map_or_else(std::sync::OnceLock::new, std::sync::OnceLock::from);
        let futures_client = base
            .filter(|b| {
                b.futures_url == futures_url && b.accept_invalid_certs == accept_invalid_certs
            })
            .and_then(|b| b.futures_client.get().cloned())
            .map_or_else(std::sync::OnceLock::new, std::sync::OnceLock::from);

        Ok(Self {
            format,
            api_url,
            futures_url,
            ws_public_url,
            ws_auth_url,
            ws_l3_url,
            ws_futures_url,
            ws_reconnect_base_ms: cli
                .ws_reconnect_base_ms
                .or_else(|| base.and_then(|b| b.ws_reconnect_base_ms)),
            spot_credentials,
            futures_credentials,
            api_secret,
            otp: cli
                .otp
                .clone()
                .map(SecretString::from)
                .or_else(|| base.and_then(|b| b.otp.clone())),
            // Blank normalizes to "no workspace" (the real account); the
            // reserved `default` collapses the same way. REPL lines are
            // sticky: an explicit per-line `--workspace` is authoritative
            // for that line; a line without the flag inherits the startup
            // scope ($KRAKEN_WORKSPACE is constant for the shell and already
            // reflected in `base`).
            workspace: match base {
                None => normalize_workspace(cli.workspace.clone()),
                Some(_) if workspace_from_flag => normalize_workspace(cli.workspace.clone()),
                Some(b) => b.workspace.clone(),
            },
            force: cli.yes || base.is_some_and(|b| b.force),
            accept_invalid_certs,
            allow_any_url_host,
            mcp_mode: false,
            // Enabled by `--monitor`; in the REPL it is sticky, falling back to the
            // session's setting when a line doesn't pass the flag (like `verbose`).
            monitor: if cli.monitor {
                Some(crate::monitor::MonitorConfig::new(
                    cli.monitor_health_interval,
                    cli.monitor_stale_after,
                    cli.monitor_heartbeat_timeout,
                ))
            } else {
                base.and_then(|b| b.monitor.clone())
            },
            spot_client,
            futures_client,
        })
    }
}

/// Resolve the API secret from its one-shot sources, in precedence order:
/// stdin > file > inline flag. Warns when the inline flag is used, since it is
/// visible in process listings.
fn resolve_secret_source(cli: &Cli) -> Result<Option<SecretString>> {
    if cli.api_secret_stdin {
        Ok(Some(config::read_secret_from_stdin()?))
    } else if let Some(path) = &cli.api_secret_file {
        Ok(Some(config::read_secret_from_file(path)?))
    } else if let Some(secret) = &cli.api_secret {
        tracing::warn!(
            "passing --api-secret on the command line exposes it in process listings. \
             Prefer --api-secret-stdin, --api-secret-file, or environment variables."
        );
        Ok(Some(SecretString::from(secret.clone())))
    } else {
        Ok(None)
    }
}

/// Kraken CLI — trade, query, and manage your Kraken account from the terminal.
#[derive(Parser)]
#[command(name = "kraken", version, about, long_about = None)]
pub struct Cli {
    /// Output format: table (default) or json.
    #[arg(short, long, value_enum, global = true)]
    pub output: Option<OutputFormat>,

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

    /// Base WebSocket reconnect backoff in milliseconds (testing/tuning knob;
    /// compresses the backoff schedule). Defaults to the production schedule.
    /// Zero is rejected: multiplicative backoff has a fixed point at zero, so a
    /// 0 base never escalates and hammers the venue with sub-second dials forever.
    #[arg(long, global = true, env = "KRAKEN_WS_RECONNECT_BASE_MS", hide = true,
          value_parser = clap::value_parser!(u64).range(1..))]
    pub ws_reconnect_base_ms: Option<u64>,

    /// API key override (takes precedence over env/config).
    #[arg(long, global = true)]
    pub api_key: Option<String>,

    /// API secret override (prefer --api-secret-stdin for security).
    #[arg(long, global = true)]
    pub api_secret: Option<String>,

    /// Read API secret from stdin (mutually exclusive with --api-secret and --api-secret-file).
    #[arg(long, global = true, conflicts_with_all = ["api_secret", "api_secret_file"])]
    pub api_secret_stdin: bool,

    /// Path to file containing API secret (mutually exclusive with --api-secret and --api-secret-stdin).
    #[arg(long, global = true, conflicts_with_all = ["api_secret", "api_secret_stdin"])]
    pub api_secret_file: Option<std::path::PathBuf>,

    // Per-engine credential environment variables. clap owns the env read; the
    // values feed the env tier of the credential resolver in `config.rs`. These
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
    /// data stays global. Unset (or `default`) uses the flat layout. Must be a single
    /// path-safe segment: letters, digits, '.', '-', or '_' (no '/', spaces,
    /// or '.'/'..').
    #[arg(long, global = true, env = "KRAKEN_WORKSPACE")]
    pub workspace: Option<String>,

    /// Skip confirmation prompts for destructive operations.
    #[arg(long, alias = "force", global = true)]
    pub yes: bool,

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

    /// DANGEROUS: allow non-Kraken hosts in URL overrides (e.g. for UAT).
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

    /// Emit live stream-reliability diagnostics (latency, trade-id gaps, staleness,
    /// throughput, connection health) to stderr while streaming. Applies to `ws`,
    /// `streamd`, and `record`; ignored by other commands.
    #[arg(long, global = true)]
    pub monitor: bool,

    /// Seconds between periodic monitor `health` snapshots (requires --monitor).
    #[arg(long, global = true, default_value_t = 10, requires = "monitor")]
    pub monitor_health_interval: u64,

    /// Soft alert after this many seconds with no market data (requires --monitor).
    #[arg(long, global = true, default_value_t = 30, requires = "monitor")]
    pub monitor_stale_after: u64,

    /// Hard alert after this many seconds with no heartbeat — connection-level
    /// liveness (requires --monitor).
    #[arg(long, global = true, default_value_t = 5, requires = "monitor")]
    pub monitor_heartbeat_timeout: u64,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
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
    fn normalize_workspace_keeps_real_names() {
        assert_eq!(
            normalize_workspace(Some("btc-dip".to_string())),
            Some("btc-dip".to_string())
        );
    }

    #[test]
    fn repl_line_reuses_base_client_when_transport_inputs_unchanged() {
        let cli = Cli::try_parse_from(["kraken"]).expect("bare invocation parses");
        let base = AppContext::from_cli(&cli).expect("default context resolves");
        base.spot().expect("client builds offline");

        let line = Cli::try_parse_from(["kraken"]).expect("bare line parses");
        let derived = AppContext::from_cli_with_fallback(&line, &base, false)
            .expect("fallback context resolves");

        // The base's memoized client is carried over, not rebuilt lazily.
        assert!(derived.spot_client.get().is_some());
    }

    #[test]
    fn repl_line_with_api_url_override_builds_its_own_client() {
        let cli = Cli::try_parse_from(["kraken"]).expect("bare invocation parses");
        let base = AppContext::from_cli(&cli).expect("default context resolves");
        base.spot().expect("client builds offline");

        let line = Cli::try_parse_from(["kraken", "--api-url", "https://api.kraken.com"])
            .expect("line with URL override parses");
        let derived = AppContext::from_cli_with_fallback(&line, &base, false)
            .expect("fallback context resolves");

        // An overridden URL must not inherit a client built for the old one.
        assert!(derived.spot_client.get().is_none());
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
