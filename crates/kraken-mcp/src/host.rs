//! Host side of the MCP seam: assembles the `kraken-mcp` server over
//! the command tree's tool surface and this binary's dispatch path. A tool call
//! crosses the seam as a typed `Command` — the same value the CLI's parser
//! produces — so tool and CLI behaviour cannot drift.
//!
//! Surface pins live in `tests/mcp_surface.rs` (snapshots of the real tool
//! surface plus the surface↔clap bijection); derivation is pinned in
//! `kraken-contract` and `kraken-mcp` against fixtures.

use crate::{
    ActiveWorkspace, GateMode, KrakenMcpServer, McpError, ServerConfig, ToolHost, ToolOutcome,
    ToolRegistry,
};
use kraken_app::{
    AppContext,
    errors::{KrakenError, Result},
};
use kraken_contract::{
    command::Command,
    tool::{Service, Surface},
};
use std::{future::Future, sync::Arc};

pub async fn serve(ctx: &AppContext, services: &str, allow_dangerous: bool) -> Result<()> {
    let active_services = crate::parse_services(services)?;

    // Discovery-time snapshot only: it shapes the advertised schema, which a
    // client caches. Per-call gating re-resolves the same rule through
    // `CliHost::gate_mode` so a promotion mid-session cannot leave a stale
    // relaxation in place.
    let workspace = kraken_app::active_manifest(ctx)?;
    let gate_mode = resolve_gate_mode(ctx);
    let registry = ToolRegistry::build(
        Surface::shared(),
        &active_services,
        allow_dangerous,
        gate_mode,
    )?;

    let mcp_ctx = ctx.for_tool_server();

    let banner = workspace.as_ref().map(|m| match m.mode {
        kraken_workspace::WorkspaceMode::Paper => ActiveWorkspace::Paper {
            name: m.name.clone(),
        },
        mode => ActiveWorkspace::Live {
            name: m.name.clone(),
            mode: mode.to_string(),
        },
    });

    let server = KrakenMcpServer::new(
        registry,
        CliHost {
            ctx: Arc::new(mcp_ctx),
            exec_lock: tokio::sync::Mutex::new(()),
        },
        ServerConfig {
            allow_dangerous,
            active_services,
            workspace: banner,
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            agent: kraken_app::telemetry::agent_client().to_string(),
            instance_id: kraken_app::telemetry::instance_id().to_string(),
        },
    );

    Ok(crate::serve_stdio(server).await?)
}

/// The binary's implementation of the MCP execution seam: dispatch one typed
/// command through the shared executor.
struct CliHost {
    ctx: Arc<AppContext>,
    /// Serializes command execution across concurrent tool calls. The account
    /// journal and its state files are single-writer (an exclusive file lock),
    /// and that lock is fail-fast, so two overlapping tool calls would
    /// otherwise race it and the loser would fail "in use by another
    /// process". A long-lived server handles requests concurrently, so they
    /// queue here instead — the CLI's own process-per-command isolation has
    /// no equivalent to reproduce.
    exec_lock: tokio::sync::Mutex<()>,
}

/// Whether a command from `group` must queue behind the others.
///
/// Only a group that opens no local file at all can skip the queue, and the
/// judgment is about files rather than about money: a `paper balance` is
/// read-only on the account and still replays the journal under its exclusive
/// lock. Market data is the one group with nothing local to open — it fetches
/// every asset catalog it needs from the venue — and it is also the slowest, so
/// exempting it is where the latency is.
///
/// Erring towards the queue is deliberate: a group wrongly exempted here fails
/// a call, a group needlessly queued only waits.
const fn queues(group: Service) -> bool {
    !matches!(group, Service::Market)
}

/// Whether a group can sign a request, so its calls read the stored keys
/// again. Only market data never signs: a paper verb such as `paper balance`
/// signs for the real account when the workspace is live.
const fn signs(group: Service) -> bool {
    !matches!(group, Service::Market)
}

impl ToolHost for CliHost {
    async fn execute(&self, command: Command, group: Service) -> ToolOutcome {
        // The async guard is held across the command's `.await` deliberately —
        // that is the window being serialized — and released the moment it
        // returns.
        let _exec = if queues(group) {
            Some(self.exec_lock.lock().await)
        } else {
            None
        };
        // Reading config.toml is blocking file I/O. An unreadable file already
        // resolves to no stored key; a refresh that panicked fails the call
        // too, never falling back to the startup keys, which can be stale.
        let refreshed = if signs(group) {
            let shared = Arc::clone(&self.ctx);
            match tokio::task::spawn_blocking(move || shared.with_current_stored_keys()).await {
                Ok(refreshed) => Some(refreshed),
                Err(error) => {
                    tracing::warn!(%error, "the stored-key refresh failed");
                    return ToolOutcome::Failure(
                        KrakenError::Config(
                            "Could not refresh the stored API keys. Retry the call.".into(),
                        )
                        .to_json_envelope(),
                    );
                }
            }
        } else {
            None
        };
        let ctx = refreshed.as_ref().unwrap_or(self.ctx.as_ref());
        // Type-erased deliberately: the executor's future spans every leaf of
        // the command tree, and naming it here would bloat this crate's own
        // future for no gain; the `dyn` boundary keeps the seam one pointer wide.
        let executing: std::pin::Pin<
            Box<
                dyn Future<Output = kraken_app::errors::Result<kraken_app::output::CommandOutput>>
                    + Send,
            >,
        > = Box::pin(kraken_app::execute_command(ctx, command));
        match executing.await {
            Ok(output) => ToolOutcome::Success(output.data),
            Err(e) => ToolOutcome::Failure(e.to_json_envelope()),
        }
    }

    fn gate_mode(&self) -> GateMode {
        resolve_gate_mode(&self.ctx)
    }
}

/// The active workspace's gate mode. Only a paper workspace relaxes the
/// mode-routed trading verbs; anything else — no workspace, a live workspace,
/// or a manifest that cannot be read — gates like the real account.
fn resolve_gate_mode(ctx: &AppContext) -> GateMode {
    match kraken_app::active_manifest(ctx) {
        Ok(Some(m)) if m.mode == kraken_workspace::WorkspaceMode::Paper => GateMode::Paper,
        _ => GateMode::Master,
    }
}

/// The MCP front-end's envelope mapping: assembly refusals are agent guidance
/// (`validation`), and transport failures are local configuration faults.
impl From<McpError> for KrakenError {
    fn from(err: McpError) -> Self {
        match err {
            McpError::Validation(what) => KrakenError::Validation(what),
            McpError::Transport(what) => KrakenError::Config(what),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exemption list, from the other side: every group the contract
    /// declares queues except the ones named here. A group added to the
    /// exemption fails this test, which is the moment to check that it
    /// really opens no local file.
    #[test]
    fn only_market_data_skips_the_queue() {
        let exempt: Vec<Service> = Service::all().filter(|group| !queues(*group)).collect();
        assert_eq!(exempt, vec![Service::Market]);
    }

    /// A group exempted here signs with the keys of the server's first call,
    /// which a later `auth switch` or `auth login` makes stale.
    #[test]
    fn only_market_data_skips_the_key_refresh() {
        let exempt: Vec<Service> = Service::all().filter(|group| !signs(*group)).collect();
        assert_eq!(exempt, vec![Service::Market]);
    }
}
