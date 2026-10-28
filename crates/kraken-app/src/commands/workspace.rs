//! Account commands backed by the [`Workspaces`] application facade.

use super::{Execute, paper};
use crate::{
    cli::AppContext,
    config,
    errors::{KrakenError, Result},
    output::CommandOutput,
};
use kraken_contract::workspace::{
    Balance, Create, List, Promote, Report, Reset, Show, Status, WorkspaceCommand,
};
use kraken_paper::{
    CommandEntry, FillSource,
    account::{Origin, PaperAccount},
};
use kraken_workspace::{CreateSpec, Target, WorkspaceMode, Workspaces};
use rust_decimal::Decimal;

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

/// Reuse-or-create the named paper workspace for a composed run (the
/// playground demo, `recipe run`). Reuse never re-funds an account that
/// exists, and a non-paper manifest under the name refuses at provision —
/// not later as a session error. Simulator knobs stay at their defaults:
/// a composed run is a demo surface, not a fee model.
pub(crate) fn provision_paper_workspace(
    ctx: &AppContext,
    name: &kraken_workspace::WorkspaceName,
    capital: Decimal,
    currency: &str,
) -> Result<()> {
    let workspaces = workspaces()?;
    match workspaces.manifest(name) {
        Ok(manifest) if manifest.mode == WorkspaceMode::Paper => Ok(()),
        Ok(manifest) => Err(KrakenError::Validation(format!(
            "the '{name}' workspace exists in {} mode; this run trades simulated fills only — \
             remove or rename workspaces/{name} to run it",
            manifest.mode
        ))),
        Err(kraken_workspace::WorkspaceError::NotFound(_)) => {
            workspaces.create(
                CreateSpec {
                    name: name.clone(),
                    capital,
                    currency: currency.to_string(),
                    mode: WorkspaceMode::Paper,
                    fee_rate: kraken_paper::DEFAULT_FEE_RATE,
                    slippage_rate: kraken_paper::DEFAULT_SLIPPAGE_RATE,
                    allowed_pairs: None,
                },
                Origin::from_mcp_mode(ctx.mcp_mode),
            )?;
            eprintln!("{name} workspace created: {capital} {currency} paper capital");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
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

/// The workspace contract as a table, shared by `create` and `show` so the two
/// can never label the same contract differently.
///
/// `allocation` marks a live *named* workspace — not the `default` real
/// account. Its capital is a claim on money you already hold rather than
/// capital the CLI minted, and the simulator's fill model does not govern its
/// fills.
fn contract_rows(
    name: &str,
    mode: WorkspaceMode,
    capital: String,
    fee_rate: Option<Decimal>,
    slippage_rate: Option<Decimal>,
    allocation: bool,
) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Workspace".to_string(), name.to_string()),
        ("Mode".to_string(), mode.to_string()),
        (
            if allocation { "Allocation" } else { "Capital" }.to_string(),
            capital,
        ),
    ];
    if allocation {
        rows.push((
            "Fees".to_string(),
            crate::output::LIVE_FEES_NOTE.to_string(),
        ));
        return rows;
    }
    let dash = || "—".to_string();
    rows.push((
        "Fee rate".to_string(),
        fee_rate.map_or_else(dash, |rate| rate.to_string()),
    ));
    rows.push((
        "Slippage rate".to_string(),
        slippage_rate.map_or_else(dash, |rate| rate.to_string()),
    ));
    rows
}

/// Refuse a live allocation the account cannot cover.
///
/// The one moment a user is paying attention to the allocation is when they
/// type it, so an unbacked claim is refused here rather than disclosed forever
/// afterwards. That makes live creation need working credentials: a claim
/// nobody can check is not a safer claim.
async fn ensure_allocation_available(
    ctx: &AppContext,
    name: &kraken_workspace::WorkspaceName,
    capital: Decimal,
    currency: &str,
) -> Result<kraken_workspace::backing::Backing> {
    use kraken_workspace::backing::BackingStatus;

    let backing = super::journal_mirror::measure_backing(ctx, name, capital, currency).await?;
    match backing.status {
        BackingStatus::Backed => Ok(backing),
        BackingStatus::OverAllocated => Err(crate::errors::KrakenError::Validation(format!(
            "cannot allocate {capital} {currency} to a live workspace: {}",
            backing.summary()
        ))),
        BackingStatus::Unverified => Err(crate::errors::KrakenError::Validation(format!(
            "cannot allocate {capital} {currency} to a live workspace before the claim can be \
             checked: {}",
            backing.summary()
        ))),
    }
}

impl Execute for Create {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        // Measured before the workspace exists, and carried through so the
        // stamp below does not re-read the same venue balance.
        let mode = super::domain_bridge::workspace_mode(self.mode);
        let backing = match mode {
            WorkspaceMode::Live => Some(
                ensure_allocation_available(
                    ctx,
                    &self.name.parse()?,
                    self.capital,
                    &self.currency.to_uppercase(),
                )
                .await?,
            ),
            WorkspaceMode::Paper => None,
        };
        // The same ceremony `promote` requires, in the same order: the full
        // explanation BEFORE the yes/no, because direct live creation is the
        // shortcut past the evidence gate and must not also skip the moment
        // where the user learns what an allocation is.
        if let Some(backing) = &backing {
            super::workspace_guard::explain_live_workspace(
                &self.name,
                self.capital,
                &self.currency.to_uppercase(),
            );
            ctx.confirm_destructive(&format!(
                "Create LIVE workspace '{}'? Its {} {} allocation is real money on the configured \
                 Kraken account ({}).",
                self.name,
                self.capital,
                self.currency.to_uppercase(),
                backing.summary()
            ))?;
        }
        // Re-measured AFTER the prompt: the figure the user confirmed goes
        // stale while they read it — a concurrent live create or an external
        // spend could claim the same balance in the meantime. The stamp below
        // carries this fresh reading.
        let backing = match backing {
            Some(_) => Some(
                ensure_allocation_available(
                    ctx,
                    &self.name.parse()?,
                    self.capital,
                    &self.currency.to_uppercase(),
                )
                .await?,
            ),
            None => None,
        };
        let manifest = workspaces()?.create(
            CreateSpec {
                name: self.name.parse()?,
                capital: self.capital,
                currency: self.currency.to_uppercase(),
                mode,
                fee_rate: self.fee_rate.unwrap_or(kraken_paper::DEFAULT_FEE_RATE),
                slippage_rate: self
                    .slippage_rate
                    .unwrap_or(kraken_paper::DEFAULT_SLIPPAGE_RATE),
                allowed_pairs: self.allow_pairs,
            },
            Origin::from_mcp_mode(ctx.mcp_mode),
        )?;
        let rows = contract_rows(
            &manifest.name,
            manifest.mode,
            format!("{} {}", manifest.capital, manifest.currency),
            Some(manifest.fee_rate),
            Some(manifest.slippage_rate),
            // `create` never makes the `default` account, so live means
            // allocation here.
            manifest.mode == WorkspaceMode::Live,
        );
        let mut out = CommandOutput::key_value(rows, serde_json::to_value(&manifest)?);
        if let Some(backing) = backing {
            let sync =
                super::journal_mirror::LiveSync::with_measured_backing(ctx, &manifest, backing)
                    .await?;
            sync.stamp(&mut out, &manifest.name);
        }
        Ok(out)
    }
}

/// What a listed row's money actually is. Without this column `default` and a
/// live workspace both render `Mode: live`, which hides the only distinction
/// that matters: one addresses the whole account, the other a slice of it.
fn scope_label(row: &kraken_workspace::ListedWorkspace) -> &'static str {
    if row.damaged.is_some() {
        return "unreadable";
    }
    if row.name == kraken_workspace::DEFAULT_WORKSPACE {
        return "whole real account";
    }
    match row.mode {
        WorkspaceMode::Paper => "simulated capital",
        WorkspaceMode::Live => "allocation of the real account",
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
                    scope_label(row).to_string(),
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
        let headers = ["Workspace", "Mode", "Capital", "Scope", "Created", "Note"]
            .map(String::from)
            .to_vec();
        Ok(CommandOutput::new(data, headers, table))
    }
}

impl Execute for Show {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let overview = workspaces()?.show(&target(ctx, self.name.as_deref())?)?;
        let is_live_workspace = overview.name != kraken_workspace::DEFAULT_WORKSPACE
            && overview.mode == WorkspaceMode::Live;
        let workspace_name = overview.name.clone();
        let mut rows = contract_rows(
            &overview.name,
            overview.mode,
            match (&overview.capital, &overview.currency) {
                (Some(capital), Some(currency)) => format!("{capital} {currency}"),
                _ => "managed by the venue".to_string(),
            },
            overview.fee_rate,
            overview.slippage_rate,
            is_live_workspace,
        );
        rows.push((
            "Created".to_string(),
            overview
                .created_at
                .map_or_else(|| "—".to_string(), |at| at.to_rfc3339()),
        ));
        let mut out = CommandOutput::key_value(rows, serde_json::to_value(&overview)?);
        if is_live_workspace {
            let manifest = workspaces()?.manifest(&workspace_name.parse()?)?;
            super::workspace_guard::warn_live_workspace(&manifest);
            let sync = super::journal_mirror::sync_live_workspace(ctx, &manifest).await?;
            sync.stamp(&mut out, &workspace_name);
        }
        Ok(out)
    }
}

impl Execute for Status {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        match target(ctx, self.name.as_deref())? {
            // The real account: venue-held equity, exactly what `kraken
            // trade-balance` reports (auth required) — led by the mode, so
            // the reader always knows which account answered.
            Target::Default => {
                let mut out = kraken_contract::account::TradeBalance::plain()
                    .execute(ctx)
                    .await?;
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
                // Reconcile before opening the journal for the read: the sync
                // takes the same writer lock, so it has to finish first.
                let sync = if manifest.mode == WorkspaceMode::Live {
                    super::workspace_guard::warn_live_workspace(&manifest);
                    Some(super::journal_mirror::sync_live_workspace(ctx, &manifest).await?)
                } else {
                    None
                };
                let mut account = PaperAccount::open_at(
                    workspaces.journal_path(&name),
                    Origin::from_mcp_mode(ctx.mcp_mode),
                )?;
                let entry = CommandEntry {
                    name: "workspace status".to_string(),
                    ..CommandEntry::default()
                };
                let mut venue = paper::Venue::resolve_ambient(ctx)?;
                // No session: the workspace's ambient account is the subject,
                // marked off the live venue either way.
                let source = match sync {
                    Some(_) => FillSource::Venue,
                    None => FillSource::Simulated,
                };
                let mut out =
                    paper::execute_status(&mut account, entry, &mut venue, source).await?;
                match sync {
                    Some(sync) => sync.stamp(&mut out, &manifest.name),
                    None => out.stamp_workspace(&manifest.name),
                }
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
            Target::Default => {
                super::account::venue_balance(kraken_contract::account::Balance::plain(), ctx).await
            }
            Target::Named(name) => {
                let workspaces = workspaces()?;
                let manifest = workspaces.manifest(&name)?;
                let sync = if manifest.mode == WorkspaceMode::Live {
                    super::workspace_guard::warn_live_workspace(&manifest);
                    Some(super::journal_mirror::sync_live_workspace(ctx, &manifest).await?)
                } else {
                    None
                };
                let balances = workspaces.balances(&name, Origin::from_mcp_mode(ctx.mcp_mode))?;
                let (headers, rows) = balance_rows(&balances.balances);
                let mut out = CommandOutput::new(serde_json::to_value(&balances)?, headers, rows);
                if let Some(sync) = sync {
                    sync.stamp(&mut out, &manifest.name);
                }
                Ok(out)
            }
        }
    }
}

impl Execute for Reset {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Target::Named(name) = target(ctx, self.name.as_deref())? else {
            return Err(kraken_workspace::WorkspaceError::CapitalManaged(
                "'default' is the real Kraken account; there is no capital to reset to. Name a \
                 paper workspace: kraken workspace reset <name>."
                    .to_string(),
            )
            .into());
        };
        let workspaces = workspaces()?;
        let current = workspaces.manifest(&name)?;
        if current.mode == WorkspaceMode::Live {
            return Err(kraken_workspace::WorkspaceError::CapitalManaged(format!(
                "workspace '{name}' is live: its allocation cannot be reset, because the journal \
                 records real venue fills. Reset a paper workspace instead."
            ))
            .into());
        }
        ctx.confirm_destructive(&format!(
            "Reset workspace '{name}' to its starting capital? History survives in the journal."
        ))?;
        let manifest = workspaces.reset(
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

impl Execute for Promote {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let Target::Named(name) = target(ctx, self.name.as_deref())? else {
            return Err(KrakenError::Validation(
                "'default' is the real Kraken account; there is nothing to promote. Name a paper \
                 workspace: kraken workspace promote <name>."
                    .to_string(),
            ));
        };
        let workspaces = workspaces()?;
        let manifest = workspaces.manifest(&name)?;
        if manifest.mode == WorkspaceMode::Live {
            super::workspace_guard::warn_live_workspace(&manifest);
            return Ok(CommandOutput::key_value(
                vec![
                    ("Workspace".to_string(), manifest.name.clone()),
                    ("Mode".to_string(), manifest.mode.to_string()),
                    ("Status".to_string(), "already live".to_string()),
                ],
                serde_json::json!({
                    "workspace": manifest.name,
                    "mode": "live",
                    "promoted": false,
                    "already_live": true,
                }),
            ));
        }
        let scope = kraken_workspace::workspace_dir(&config::config_dir()?, &name);
        let evidence = gather_promotion_evidence(&scope, Origin::from_mcp_mode(ctx.mcp_mode))?;
        let checklist = kraken_workspace::promote::evaluate(&manifest, evidence.clone());
        if !checklist.promotable {
            if ctx.format == crate::output::OutputFormat::Table {
                render_checklist(&checklist);
            }
            return Err(kraken_workspace::WorkspaceError::NotPromotable {
                workspace: manifest.name,
                checklist: Box::new(checklist),
            }
            .into());
        }
        if ctx.format == crate::output::OutputFormat::Table {
            render_checklist(&checklist);
        }
        // The evidence gate says the strategy earned live money; this says the
        // account actually holds what the workspace will claim.
        let backing =
            ensure_allocation_available(ctx, &name, manifest.capital, &manifest.currency).await?;
        // The full explanation BEFORE the yes/no, the same ceremony as live
        // creation: a briefing delivered after the switch is not consent.
        super::workspace_guard::explain_live_workspace(
            &manifest.name,
            manifest.capital,
            &manifest.currency,
        );
        ctx.confirm_destructive(&format!(
            "Promote workspace '{name}' to LIVE trading on the configured Kraken account? Its {} \
             {} allocation becomes real money ({}).",
            manifest.capital,
            manifest.currency,
            backing.summary()
        ))?;
        // Re-measured AFTER the prompt, like the blockers `promote` re-checks
        // under the journal lock: the backing the user confirmed goes stale
        // while they read it, and a concurrent live create could claim the
        // same balance in the meantime.
        let backing =
            ensure_allocation_available(ctx, &name, manifest.capital, &manifest.currency).await?;
        match workspaces.promote(&name, evidence, Origin::from_mcp_mode(ctx.mcp_mode)) {
            Ok(manifest) => {
                let sync =
                    super::journal_mirror::LiveSync::with_measured_backing(ctx, &manifest, backing)
                        .await?;
                let mut out = CommandOutput::key_value(
                    vec![
                        ("Workspace".to_string(), manifest.name.clone()),
                        ("Mode".to_string(), manifest.mode.to_string()),
                    ],
                    serde_json::json!({
                        "workspace": manifest.name,
                        "mode": manifest.mode.to_string(),
                        "promoted": true,
                    }),
                );
                sync.stamp(&mut out, &manifest.name);
                Ok(out)
            }
            Err(err) => {
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
    origin: Origin,
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

    let active_session = kraken_workspace::session::active(scope)?.is_some();
    let open_paper_orders = if active_session {
        0
    } else {
        PaperAccount::open_at(scope.join(kraken_workspace::JOURNAL_FILE), origin)?
            .state()?
            .open_orders
            .len()
    };

    Ok(kraken_workspace::promote::PromotionInputs {
        experiments,
        manual_trades_in_windows,
        active_session,
        open_paper_orders,
    })
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

        // Reconcile before opening the journal for the read: the sync takes
        // the same writer lock, and terminal venue orders it folds must show
        // up in the rows, ledger facts, and equity assembled below.
        let sync = if manifest.mode == WorkspaceMode::Live {
            super::workspace_guard::warn_live_workspace(&manifest);
            Some(super::journal_mirror::sync_live_workspace(ctx, &manifest).await?)
        } else {
            None
        };
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
            let (value, complete) = paper::value_account(ctx.spot(), &state).await;
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
                live_since: ledger.live_since,
                equity,
                drift,
            },
        );
        let mut out = report_output(&report)?;
        if let Some(sync) = sync {
            sync.stamp(&mut out, &manifest.name);
        }
        Ok(out)
    }
}

/// Per-run rows from the scorecard anchor, verbatim — the report can never
/// disagree with `explain pnl`. A run that cannot be scored is an error row.
fn gather_run_rows(
    scope: &std::path::Path,
) -> Result<(Vec<kraken_workspace::report::SessionRow>, Option<String>)> {
    use kraken_workspace::{report::SessionRow, session::SessionStatus};

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
    /// The instant the journal stopped recording simulated fills.
    live_since: Option<chrono::DateTime<chrono::Utc>>,
    last_reconciled: Option<kraken_paper::VenueSnapshot>,
}

fn scan_journal_ledger(scope: &std::path::Path) -> Result<JournalLedger> {
    use kraken_paper::{AccountEvent, account::AccountRecord};
    use kraken_recording::{JsonlSource, Source};
    use kraken_session::session::{START_MARKER, STOP_MARKER};

    let mut ledger = JournalLedger {
        manual_fills: 0,
        reset_epochs: 0,
        live_since: None,
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
            AccountEvent::FillSourceChanged {
                to: kraken_paper::FillSource::Venue,
                ..
            } => ledger.live_since = Some(record.ts),
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
            "{} ({}) — {} {} {}, equity {}, sessions P&L {:+}",
            report.workspace,
            report.mode,
            match report.mode {
                WorkspaceMode::Paper => "capital",
                WorkspaceMode::Live => "allocation",
            },
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

delegate_execute!(WorkspaceCommand {
    Create,
    List,
    Show,
    Status,
    Balance,
    Reset,
    Promote,
    Report
});
