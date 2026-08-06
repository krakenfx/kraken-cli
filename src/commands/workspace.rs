//! Account commands backed by the [`Workspaces`] application facade.

use clap::Subcommand;
use enum_dispatch::enum_dispatch;
use kraken_paper::CommandEntry;
use kraken_paper::account::{Origin, PaperAccount};
use kraken_workspace::{CreateSpec, Target, WorkspaceMode, Workspaces};
use rust_decimal::Decimal;

use super::{Execute, paper};
use crate::cli::AppContext;
use crate::config;
use crate::errors::{KrakenError, Result};
use crate::output::CommandOutput;

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum WorkspaceCommand {
    /// Create AND fund a workspace account; capital is fixed at create.
    Create(Create),
    /// List every account; `default` is the real Kraken account.
    List(List),
    /// Show a workspace's contract.
    Show(Show),
    /// Account summary with marked equity and P&L.
    Status(Status),
    /// Account balances, folded from the workspace journal.
    Balance(Balance),
    /// Return a paper workspace to its starting capital (a new journal
    /// epoch — history survives).
    Reset(Reset),
    /// Evaluate the paper→live promotion checklist against the workspace's
    /// recorded evidence. Refused until scoped credentials land.
    Promote(Promote),
    /// Account-first report: every session's outcome, the manual segment, and
    /// digit-exact totals.
    Report(Report),
}

#[derive(Debug, clap::Args)]
pub(crate) struct Create {
    /// Workspace name (one path-safe segment).
    pub(crate) name: String,
    /// Starting capital, denominated in --currency.
    #[arg(long)]
    pub(crate) capital: Decimal,
    /// Capital currency.
    #[arg(long, default_value = "USD")]
    pub(crate) currency: String,
    /// Required execution mode; named live workspaces are not yet available.
    #[arg(long)]
    pub(crate) mode: WorkspaceMode,
    /// Fee rate as a decimal (default: 0.0026 = 0.26% Kraken Starter tier).
    #[arg(long)]
    pub(crate) fee_rate: Option<Decimal>,
    /// Slippage rate as a decimal (default: 0.0 = no slippage simulation).
    #[arg(long, alias = "slippage")]
    pub(crate) slippage_rate: Option<Decimal>,
    /// Restrict trading to these pairs (comma-separated) — the paper
    /// analogue of API-key pair permissions. Omit = unrestricted; an empty
    /// value = deny-all.
    #[arg(long, value_delimiter = ',', num_args = 0..)]
    pub(crate) allow_pairs: Option<Vec<String>>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct List {}

#[derive(Debug, clap::Args)]
pub(crate) struct Show {
    /// Workspace name (defaults to the active workspace, else `default`).
    name: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Status {
    /// Workspace name (defaults to the active workspace, else `default`).
    pub(crate) name: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Balance {
    /// Workspace name (defaults to the active workspace, else `default`).
    pub(crate) name: Option<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Reset {
    /// Workspace name (defaults to the active workspace).
    pub(crate) name: Option<String>,
    /// Re-parameterize on reset: new starting capital (updates the
    /// contract; omit to return to the created capital).
    #[arg(long, alias = "balance")]
    pub(crate) capital: Option<Decimal>,
    /// New capital currency.
    #[arg(long)]
    pub(crate) currency: Option<String>,
    /// New fee rate as a decimal.
    #[arg(long)]
    pub(crate) fee_rate: Option<Decimal>,
    /// New slippage rate as a decimal.
    #[arg(long, alias = "slippage")]
    pub(crate) slippage_rate: Option<Decimal>,
}

/// The account a command addresses: explicit arg > active workspace > the
/// real account.
fn target(ctx: &AppContext, name: Option<&str>) -> Result<Target> {
    Ok(Target::resolve(name.or(ctx.workspace.as_deref()))?)
}

/// The facade over the outer config dir. Workspace commands NEVER resolve
/// `scoped_base()`: they address accounts by name, they don't run inside one.
fn workspaces() -> Result<Workspaces> {
    Ok(Workspaces::new(config::config_dir()?))
}

/// The one balances table shape, shared with the session-scoped `paper
/// balance` read that survives until runs replace sessions.
pub(crate) fn balance_rows(
    balances: &std::collections::BTreeMap<String, kraken_workspace::AssetBalance>,
) -> (Vec<String>, Vec<Vec<String>>) {
    let headers = ["Asset", "Total", "Reserved", "Available"]
        .map(String::from)
        .to_vec();
    let rows = balances
        .iter()
        .map(|(asset, balance)| {
            vec![
                asset.clone(),
                balance.total.to_string(),
                balance.reserved.to_string(),
                balance.available.to_string(),
            ]
        })
        .collect();
    (headers, rows)
}

impl Execute for Create {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let manifest = workspaces()?.create(
            CreateSpec {
                name: self.name.parse()?,
                capital: self.capital,
                currency: self.currency.to_uppercase(),
                mode: self.mode,
                fee_rate: self.fee_rate.unwrap_or(kraken_paper::DEFAULT_FEE_RATE),
                slippage_rate: self
                    .slippage_rate
                    .unwrap_or(kraken_paper::DEFAULT_SLIPPAGE_RATE),
                allowed_pairs: self.allow_pairs,
            },
            Origin::from_mcp_mode(ctx.mcp_mode),
        )?;
        Ok(CommandOutput::key_value(
            vec![
                ("Workspace".to_string(), manifest.name.clone()),
                ("Mode".to_string(), manifest.mode.to_string()),
                (
                    "Capital".to_string(),
                    format!("{} {}", manifest.capital, manifest.currency),
                ),
                ("Fee rate".to_string(), manifest.fee_rate.to_string()),
                (
                    "Slippage rate".to_string(),
                    manifest.slippage_rate.to_string(),
                ),
            ],
            serde_json::to_value(&manifest)?,
        ))
    }
}

impl Execute for List {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        let rows = workspaces()?.list()?;
        let table: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                vec![
                    row.name.clone(),
                    row.mode.to_string(),
                    match (&row.capital, &row.currency) {
                        (Some(capital), Some(currency)) => format!("{capital} {currency}"),
                        _ => "—".to_string(),
                    },
                    row.created_at
                        .map_or_else(|| "—".to_string(), |at| at.to_rfc3339()),
                    row.damaged.clone().unwrap_or_default(),
                ]
            })
            .collect();
        let data = serde_json::json!({
            "group": "workspace",
            "type": "list",
            "workspaces": rows,
        });
        let headers = ["Workspace", "Mode", "Capital", "Created", "Note"]
            .map(String::from)
            .to_vec();
        Ok(CommandOutput::new(data, headers, table))
    }
}

impl Execute for Show {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let overview = workspaces()?.show(&target(ctx, self.name.as_deref())?)?;
        let dash = || "—".to_string();
        Ok(CommandOutput::key_value(
            vec![
                ("Workspace".to_string(), overview.name.clone()),
                ("Mode".to_string(), overview.mode.to_string()),
                (
                    "Capital".to_string(),
                    match (&overview.capital, &overview.currency) {
                        (Some(capital), Some(currency)) => format!("{capital} {currency}"),
                        _ => "managed by the venue".to_string(),
                    },
                ),
                (
                    "Fee rate".to_string(),
                    overview.fee_rate.map_or_else(dash, |rate| rate.to_string()),
                ),
                (
                    "Slippage rate".to_string(),
                    overview
                        .slippage_rate
                        .map_or_else(dash, |rate| rate.to_string()),
                ),
                (
                    "Created".to_string(),
                    overview.created_at.map_or_else(dash, |at| at.to_rfc3339()),
                ),
            ],
            serde_json::to_value(&overview)?,
        ))
    }
}

impl Execute for Status {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match target(ctx, self.name.as_deref())? {
            // The real account: venue-held equity, exactly what `kraken
            // trade-balance` reports (auth required) — led by the mode, so
            // the reader always knows which account answered.
            Target::Default => {
                let mut out = super::account::TradeBalance::plain().execute(ctx).await?;
                if let Some(object) = out.data.as_object_mut() {
                    object.insert("mode".to_string(), serde_json::json!("live"));
                }
                if out.headers.len() == 2 {
                    out.rows.insert(
                        0,
                        vec![
                            "Mode".to_string(),
                            "live — the real Kraken account".to_string(),
                        ],
                    );
                }
                Ok(out)
            }
            Target::Named(name) => {
                let workspaces = workspaces()?;
                let manifest = workspaces.manifest(&name)?;
                let mut account = PaperAccount::open_at(
                    workspaces.journal_path(&name),
                    Origin::from_mcp_mode(ctx.mcp_mode),
                )?;
                let entry = CommandEntry {
                    name: "workspace status".to_string(),
                    ..CommandEntry::default()
                };
                let mut out = paper::execute_status(
                    &mut account,
                    entry,
                    // No session: the workspace's ambient account is the
                    // subject, marked off the live venue.
                    &mut paper::Venue::resolve_ambient(ctx)?,
                )
                .await?;
                out.stamp_workspace(manifest.name.as_str());
                Ok(out)
            }
        }
    }
}

impl Execute for Balance {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match target(ctx, self.name.as_deref())? {
            // The real account's balances live at the venue (auth required).
            // Straight to the venue view: `default` IS the master account, so
            // re-entering the mode router would only recurse.
            Target::Default => super::account::Balance::plain().venue(ctx).await,
            Target::Named(name) => {
                let balances =
                    workspaces()?.balances(&name, Origin::from_mcp_mode(ctx.mcp_mode))?;
                let (headers, rows) = balance_rows(&balances.balances);
                Ok(CommandOutput::new(
                    serde_json::to_value(&balances)?,
                    headers,
                    rows,
                ))
            }
        }
    }
}

impl Execute for Reset {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Target::Named(name) = target(ctx, self.name.as_deref())? else {
            return Err(kraken_workspace::WorkspaceError::CapitalManaged(
                "'default' is the real Kraken account; there is no capital to reset to. \
                 Name a paper workspace: kraken workspace reset <name>."
                    .to_string(),
            )
            .into());
        };
        ctx.confirm_destructive(&format!(
            "Reset workspace '{name}' to its starting capital? History survives in the journal."
        ))?;
        let manifest = workspaces()?.reset(
            &name,
            Origin::from_mcp_mode(ctx.mcp_mode),
            kraken_workspace::ResetOverrides {
                capital: self.capital,
                currency: self.currency,
                fee_rate: self.fee_rate,
                slippage_rate: self.slippage_rate,
            },
        )?;
        Ok(CommandOutput::key_value(
            vec![
                ("Workspace".to_string(), manifest.name.clone()),
                (
                    "Reset to".to_string(),
                    format!("{} {}", manifest.capital, manifest.currency),
                ),
            ],
            serde_json::json!({
                "group": "workspace",
                "type": "reset",
                "workspace": manifest.name,
                "capital": manifest.capital.to_string(),
                "currency": manifest.currency,
            }),
        ))
    }
}

#[derive(Debug, clap::Args)]
pub(crate) struct Promote {
    /// Workspace name (defaults to the active workspace).
    name: Option<String>,
}

impl Execute for Promote {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Target::Named(name) = target(ctx, self.name.as_deref())? else {
            return Err(KrakenError::Validation(
                "'default' is the real Kraken account; there is nothing to promote. \
                 Name a paper workspace: kraken workspace promote <name>."
                    .to_string(),
            ));
        };
        ctx.confirm_destructive(&format!(
            "Evaluate promotion of workspace '{name}' toward live trading?"
        ))?;
        let scope = kraken_workspace::workspace_dir(&config::config_dir()?, &name);
        let evidence = gather_promotion_evidence(&scope)?;
        match workspaces()?.promote(&name, evidence) {
            Ok(manifest) => Ok(CommandOutput::key_value(
                vec![
                    ("Workspace".to_string(), manifest.name.clone()),
                    ("Mode".to_string(), manifest.mode.to_string()),
                ],
                serde_json::json!({ "workspace": manifest.name, "mode": manifest.mode.to_string(), "promoted": true }),
            )),
            Err(err) => {
                // Humans on a table terminal get the checklist as stderr
                // diagnostics; the machine contract is the JSON envelope's
                // `checklist` field, carried by the error itself.
                if ctx.format == crate::output::OutputFormat::Table
                    && let kraken_workspace::WorkspaceError::NotPromotable { checklist, .. } = &err
                {
                    render_checklist(checklist);
                }
                Err(err.into())
            }
        }
    }
}

fn render_checklist(checklist: &kraken_workspace::promote::PromotionChecklist) {
    eprintln!(
        "promotion checklist for '{}' ({}):",
        checklist.workspace, checklist.mode
    );
    for item in &checklist.criteria {
        let mark = if item.satisfied { "ok " } else { "MISSING" };
        eprintln!("  [{mark:>7}] {} — {}", item.criterion, item.detail);
    }
    for blocker in &checklist.blockers {
        eprintln!("  blocked: {blocker}");
    }
}

/// Walk the workspace's recorded evidence for the checklist: per-experiment
/// verdicts from `lab compare`, and manual (decision-less) fills inside
/// stopped session windows.
fn gather_promotion_evidence(
    scope: &std::path::Path,
) -> Result<kraken_workspace::promote::PromotionInputs> {
    use kraken_session::timeline::event::EventPayload;

    let lab = kraken_lab::Lab::new(scope).with_library(config::config_dir()?);
    let mut experiments = Vec::new();
    for experiment in lab.experiments()? {
        match lab.compare(&experiment) {
            Ok(comparison) => {
                let total_sessions = comparison.outcomes.len();
                let mut passing_sessions = 0;
                let mut passing_live_sessions = 0;
                for outcome in &comparison.outcomes {
                    if let kraken_lab::OutcomeResult::Scored { card, verdict } = &outcome.result
                        && verdict.pass
                    {
                        passing_sessions += 1;
                        if matches!(card.source, kraken_lab::ScoreSource::Live { .. }) {
                            passing_live_sessions += 1;
                        }
                    }
                }
                experiments.push(kraken_workspace::promote::ExperimentEvidence {
                    experiment,
                    total_sessions,
                    passing_sessions,
                    passing_live_sessions,
                });
            }
            // An unreadable experiment cannot carry evidence; promotion must
            // still report the rest rather than fail the evaluation.
            Err(err) => {
                tracing::warn!(experiment = %experiment, error = %err, "experiment excluded from promotion evidence");
            }
        }
    }

    let mut manual_trades_in_windows = 0usize;
    for row in kraken_workspace::session::list(scope)? {
        if row.status != kraken_workspace::session::SessionStatus::Stopped {
            continue;
        }
        let (session_id, manifest) = kraken_workspace::session::resolve(
            scope,
            &kraken_workspace::session::SessionRefSpec::Ordinal(row.session.ordinal()),
        )?;
        let timeline =
            kraken_session::session::read_window(kraken_session::session::SessionTracks {
                journal: scope.join(kraken_workspace::JOURNAL_FILE),
                session_dir: kraken_session::session::session_dir(scope, session_id.ordinal()),
                manifest,
            })
            .map_err(kraken_workspace::WorkspaceError::from)?;
        let mut decided = std::collections::HashSet::new();
        let mut fills = Vec::new();
        for event in &timeline.events {
            match &event.payload {
                EventPayload::Decision(decision) => {
                    if let Some(order_id) = &decision.order_id {
                        decided.insert(order_id.clone());
                    }
                }
                EventPayload::Account(record) => {
                    if let kraken_paper::AccountEvent::OrderFilled { trade } = &record.event {
                        fills.push(trade.order_id.clone());
                    }
                }
                EventPayload::Market(_) => {}
            }
        }
        manual_trades_in_windows += fills.iter().filter(|id| !decided.contains(*id)).count();
    }

    Ok(kraken_workspace::promote::PromotionInputs {
        experiments,
        manual_trades_in_windows,
    })
}

#[derive(Debug, clap::Args)]
pub(crate) struct Report {
    /// Workspace name (defaults to the active workspace).
    name: Option<String>,
}

impl Execute for Report {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Target::Named(name) = target(ctx, self.name.as_deref())? else {
            return Err(KrakenError::Validation(
                "'default' is the real Kraken account; its equity lives at the venue — use \
                 'kraken workspace status'. Name a workspace: kraken workspace report <name>."
                    .to_string(),
            ));
        };
        let manifest = workspaces()?.manifest(&name)?;
        let scope = kraken_workspace::workspace_dir(&config::config_dir()?, &name);

        let (runs, running) = gather_run_rows(&scope)?;
        let ledger = scan_journal_ledger(&scope)?;
        // Valuation is injected here, at the network boundary — the crate
        // assembling the report stays offline.
        let equity = {
            let state = {
                let account = PaperAccount::open_at(
                    scope.join(kraken_workspace::JOURNAL_FILE),
                    Origin::from_mcp_mode(ctx.mcp_mode),
                )?;
                account.state()?.clone()
            };
            let (value, complete) = paper::value_account(ctx.spot()?, &state).await;
            let drift = ledger.last_reconciled.as_ref().map(|snapshot| {
                kraken_workspace::session::drift(&state.balances, &snapshot.balances)
            });
            (Some((value, complete)), drift)
        };
        let (equity, drift) = equity;

        let report = kraken_workspace::report::assemble(
            &manifest,
            kraken_workspace::report::ReportInputs {
                sessions: runs,
                running,
                manual_fills: ledger.manual_fills,
                reset_epochs: ledger.reset_epochs,
                equity,
                drift,
            },
        );
        report_output(&report)
    }
}

/// Per-run rows from the scorecard anchor, verbatim — the report can never
/// disagree with `explain pnl`. A run that cannot be scored is an error row.
fn gather_run_rows(
    scope: &std::path::Path,
) -> Result<(Vec<kraken_workspace::report::SessionRow>, Option<String>)> {
    use kraken_workspace::report::SessionRow;
    use kraken_workspace::session::SessionStatus;

    let lab = kraken_lab::Lab::new(scope).with_library(config::config_dir()?);
    let mut rows = Vec::new();
    let mut running = None;
    for record in kraken_workspace::session::list(scope)? {
        match record.status {
            SessionStatus::Recording => {
                // Disclosed and excluded: an open window has no anchor yet.
                running.get_or_insert(record.session.to_string());
            }
            SessionStatus::Stopped => {
                let row = match lab.score(record.session.ordinal(), record.experiment.as_deref()) {
                    Ok(scored) => SessionRow {
                        session: record.session.to_string(),
                        label: record.label,
                        experiment: record.experiment,
                        status: record.status,
                        opening_equity: Some(scored.card.anchor.starting_balance),
                        closing_equity: Some(scored.card.anchor.final_value),
                        pnl: Some(scored.card.anchor.total_pnl),
                        fills: Some(scored.card.trades.fills),
                        manual_trades: Some(scored.card.trades.fills - scored.card.trades.decided),
                        verdict: scored.judged.map(|judged| judged.verdict.pass),
                        error: None,
                    },
                    Err(err) => error_row(&record, format!("{}: {err}", err.category())),
                };
                rows.push(row);
            }
            SessionStatus::Aborted => rows.push(error_row(
                &record,
                "aborted — the window never closed, so there is no summary to anchor on"
                    .to_string(),
            )),
            SessionStatus::Damaged => rows.push(error_row(
                &record,
                record
                    .note
                    .clone()
                    .unwrap_or_else(|| "damaged session.json".to_string()),
            )),
        }
    }
    Ok((rows, running))
}

fn error_row(
    record: &kraken_workspace::session::SessionRecord,
    error: String,
) -> kraken_workspace::report::SessionRow {
    kraken_workspace::report::SessionRow {
        session: record.session.to_string(),
        label: record.label.clone(),
        experiment: record.experiment.clone(),
        status: record.status,
        opening_equity: None,
        closing_equity: None,
        pnl: None,
        fills: None,
        manual_trades: None,
        verdict: None,
        error: Some(error),
    }
}

/// One lock-free pass over the journal for the ledger facts the report
/// discloses: fills outside any session window, reset epochs, and the last
/// venue reconciliation snapshot.
struct JournalLedger {
    manual_fills: usize,
    reset_epochs: usize,
    last_reconciled: Option<kraken_paper::VenueSnapshot>,
}

fn scan_journal_ledger(scope: &std::path::Path) -> Result<JournalLedger> {
    use kraken_paper::AccountEvent;
    use kraken_paper::account::AccountRecord;
    use kraken_recording::{JsonlSource, Source};
    use kraken_session::session::{START_MARKER, STOP_MARKER};

    let mut ledger = JournalLedger {
        manual_fills: 0,
        reset_epochs: 0,
        last_reconciled: None,
    };
    let journal = scope.join(kraken_workspace::JOURNAL_FILE);
    let Some(source) = JsonlSource::<AccountRecord>::open(journal) else {
        return Ok(ledger);
    };
    let mut open_windows = std::collections::HashSet::new();
    for record in source.read()? {
        match &record.event {
            AccountEvent::Command(entry) => {
                if let Some(run) = &entry.session {
                    if entry.name == START_MARKER {
                        open_windows.insert(run.clone());
                    } else if entry.name == STOP_MARKER {
                        open_windows.remove(run);
                    }
                }
            }
            AccountEvent::OrderFilled { .. } if open_windows.is_empty() => {
                ledger.manual_fills += 1;
            }
            AccountEvent::Reset(_) => ledger.reset_epochs += 1,
            AccountEvent::Reconciled(snapshot) => {
                ledger.last_reconciled = Some(snapshot.clone());
            }
            _ => {}
        }
    }
    Ok(ledger)
}

fn report_output(report: &kraken_workspace::report::WorkspaceReport) -> Result<CommandOutput> {
    let dash = || "—".to_string();
    let rows: Vec<Vec<String>> = report
        .sessions
        .iter()
        .map(|row| {
            vec![
                row.session.clone(),
                row.label.clone().unwrap_or_else(dash),
                row.status.to_string(),
                row.pnl.map_or_else(dash, |p| format!("{p:+}")),
                row.fills.map_or_else(dash, |f| f.to_string()),
                row.verdict
                    .map_or_else(dash, |pass| if pass { "pass" } else { "fail" }.to_string()),
                row.error.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let headers = [
        "Session", "Label", "Status", "P&L", "Fills", "Verdict", "Error",
    ]
    .map(String::from)
    .to_vec();
    let mut footer = vec![
        format!(
            "{} ({}) — capital {} {}, equity {}, sessions P&L {:+}",
            report.workspace,
            report.mode,
            report.capital,
            report.currency,
            report
                .equity
                .map_or_else(dash, |e| format!("{e} {}", report.currency)),
            report.sessions_pnl,
        ),
        format!(
            "manual fills outside windows: {}; reset epochs: {}",
            report.manual.fills, report.reset_epochs
        ),
    ];
    footer.extend(report.caveats.iter().map(|c| format!("caveat: {c}")));
    Ok(CommandOutput::new(serde_json::to_value(report)?, headers, rows).with_footer(footer))
}
