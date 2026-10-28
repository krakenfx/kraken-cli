//! `kraken workspace`: arguments for the account lifecycle verbs.

use crate::domain_values::WorkspaceMode;
use clap::Subcommand;
use rust_decimal::Decimal;

/// The `workspace` family: the account lifecycle — created and funded once,
/// reset to its capital, and promoted from paper to live.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceCommand {
    /// Create a workspace and seed its local capital journal.
    ///
    /// Writes the contract (`workspace.json`) and seeds the journal with the
    /// capital epoch, in one step. Paper mode simulates fills locally and
    /// needs no credentials. Live mode allocates part of the configured
    /// Kraken account: it needs working credentials, checks the requested
    /// allocation against the real balance minus what other live workspaces
    /// already claim, and refuses an allocation the account cannot cover.
    /// Capital is fixed at create. Refuses the name `default`.
    Create(Create),
    /// List every account; `default` is the real Kraken account.
    List(List),
    /// Show a workspace's contract.
    Show(Show),
    /// Account summary with marked equity and P&L.
    Status(Status),
    /// Account balances, folded from the workspace journal.
    Balance(Balance),
    /// Return a paper workspace to its starting capital.
    ///
    /// A new reset epoch on the journal: history survives, but open orders and
    /// balances are superseded — destructive, and confirmed.
    Reset(Reset),
    /// Evaluate the paper→live promotion checklist and switch the workspace to
    /// live execution on the real account when it passes.
    ///
    /// The journal and history are preserved and the paper era is marked with a
    /// `fill_source_changed` event, so simulated history stays distinguishable
    /// from real history. Promotion also verifies that the account can
    /// cover the workspace's allocation, then requires explicit
    /// confirmation. Subsequent real spot orders use the normal configured
    /// credentials and are reconciled back into this workspace's ledger.
    Promote(Promote),
    /// Account-first report of every session's outcome.
    ///
    /// One row per session with opening and closing equity, P&L, fills, and the
    /// verdict taken verbatim from the scorecard anchor (never recomputed),
    /// plus the manual segment, reset epochs, venue drift, and digit-exact
    /// totals. A session that cannot be scored is a named error row, never
    /// an omission; a still-recording session is disclosed and excluded
    /// from every sum.
    Report(Report),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Create {
    /// Workspace name (one path-safe segment).
    pub name: String,
    /// Starting capital, or the live workspace's logical allocation.
    #[arg(long)]
    pub capital: Decimal,
    /// Capital currency, as the modern Kraken asset code (`BTC`, not `XBT`;
    /// `USD`, not `ZUSD`) — a workspace must key what its fills key.
    #[arg(long, default_value = "USD")]
    #[serde(default = "crate::value_types::usd")]
    pub currency: String,
    /// Required execution mode.
    #[arg(long)]
    pub mode: WorkspaceMode,
    /// Fee rate as a decimal (default: 0.0026 = 0.26% Kraken Starter tier).
    #[arg(long)]
    pub fee_rate: Option<Decimal>,
    /// Slippage rate as a decimal (default: 0.0 = no slippage simulation).
    #[arg(long, alias = "slippage")]
    pub slippage_rate: Option<Decimal>,
    /// Restrict trading to these pairs (comma-separated) — the paper
    /// analogue of API-key pair permissions. Omit = unrestricted; an empty
    /// value = deny-all.
    #[arg(long, value_delimiter = ',', num_args = 0..)]
    pub allow_pairs: Option<Vec<String>>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct List {}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Show {
    /// Workspace name (defaults to the active workspace, else `default`).
    pub name: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Status {
    /// Workspace name (defaults to the active workspace, else `default`).
    pub name: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Balance {
    /// Workspace name (defaults to the active workspace, else `default`).
    pub name: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Reset {
    /// Workspace name (defaults to the active workspace).
    pub name: Option<String>,
    /// Re-parameterize on reset: new starting capital (updates the
    /// contract; omit to return to the created capital).
    #[arg(long, alias = "balance")]
    pub capital: Option<Decimal>,
    /// New capital currency.
    #[arg(long)]
    pub currency: Option<String>,
    /// New fee rate as a decimal.
    #[arg(long)]
    pub fee_rate: Option<Decimal>,
    /// New slippage rate as a decimal.
    #[arg(long, alias = "slippage")]
    pub slippage_rate: Option<Decimal>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Promote {
    /// Workspace name (defaults to the active workspace).
    pub name: Option<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Report {
    /// Workspace name (defaults to the active workspace).
    pub name: Option<String>,
}

crate::tools! {
    /// The `kraken workspace` commands.
    fn workspace in ["workspace"] of WorkspaceCommand {
        Create  as "create"  => Workspace, LocalOrLive, None, Tool
            example "kraken workspace create btc-momentum --capital 10000 --mode paper -o json";
        List    as "list"    => Workspace, ReadOnly,    None, Tool
            example "kraken workspace list -o json";
        Show    as "show"    => Workspace, ReadOnly,    None, Tool
            example "kraken workspace show btc-momentum -o json";
        Status  as "status"  => Workspace, ReadOnly,    None, Tool
            example "kraken workspace status btc-momentum -o json";
        Balance as "balance" => Workspace, ReadOnly,    None, Tool
            example "kraken workspace balance btc-momentum -o json";
        Reset   as "reset"   => Workspace, Dangerous,   None, Tool
            example "kraken workspace reset btc-momentum -o json";
        Promote as "promote" => Workspace, Dangerous,   None, Tool
            example "kraken workspace promote btc-momentum -o json";
        Report  as "report"  => Workspace, ReadOnly,    None, Tool
            example "kraken workspace report btc-momentum -o json";
    }
}
