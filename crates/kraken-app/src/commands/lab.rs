//! CLI and MCP adapters for [`kraken_lab::Lab`].

use super::Execute;
use crate::{
    cli::AppContext,
    errors::Result,
    output::{CommandOutput, rounded},
};
use kraken_contract::lab::{Compare, Curve, LabCommand, New, Next, Score, Show};
use kraken_lab::{
    Check, Comparison, Criteria, CurveSummary, Experiment, Judged, Lab, Metrics, NextAction,
    OutcomeResult, PlannedSession, ScoreSource, Scorecard, ScoredSession, SessionResult,
    TradeStats,
};
use rust_decimal::Decimal;

#[cfg(test)]
mod guard_seam_tests {
    use clap::Parser;

    /// The seal is irreversible and a replay leg cannot run on a live ledger,
    /// so the guard needs the plan's shape before it is sealed. A live-only
    /// plan must NOT trip it — a live workspace can run one.
    #[test]
    fn only_a_plan_carrying_a_replay_leg_is_flagged() {
        #[derive(Parser)]
        struct Harness {
            #[command(flatten)]
            new: super::New,
        }
        let plans = |args: &[&str]| {
            let mut argv = vec!["h", "exp", "--hypothesis", "x"];
            argv.extend_from_slice(args);
            Harness::try_parse_from(argv)
                .expect("harness parses")
                .new
                .plans_a_replay_leg()
        };

        assert!(plans(&["--session", "replay:tape:jun"]));
        assert!(plans(&[
            "--session",
            "live:10s",
            "--session",
            "replay:session:s1"
        ]));
        assert!(!plans(&["--session", "live:10s"]));
        assert!(!plans(&[]), "a plan-less experiment commits to nothing");
    }
}

/// The scope's lab with `tape:` refs pointed at the global shared library
/// (market memory is never workspace-scoped).
fn lab_in(ctx: &AppContext) -> Result<Lab> {
    Ok(Lab::new(ctx.scoped_base()?).with_library(crate::config::config_dir()?))
}

impl Execute for Score {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let spec = self
            .session
            .parse()
            .unwrap_or(kraken_workspace::session::SessionRefSpec::Latest);
        super::session::ensure_session_stopped(ctx, &self.session)?;
        let (scope, _) = super::session::scope_dir(ctx)?;
        let (session_id, manifest) = kraken_workspace::session::resolve(&scope, &spec)?;
        let scored = Lab::new(&scope)
            .with_library(crate::config::config_dir()?)
            .score(session_id.ordinal(), self.experiment.as_deref())?;
        let mut out = output(&scored)?;
        out.stamp_session(&session_id.to_string());
        stamp_scope(
            &mut out,
            ctx,
            manifest
                .window
                .ended_at
                .or(Some(manifest.window.started_at)),
        )?;
        Ok(out)
    }
}

impl Execute for Curve {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let spec = self
            .session
            .parse()
            .unwrap_or(kraken_workspace::session::SessionRefSpec::Latest);
        super::session::ensure_session_stopped(ctx, &self.session)?;
        let (scope, _) = super::session::scope_dir(ctx)?;
        let (session_id, manifest) = kraken_workspace::session::resolve(&scope, &spec)?;
        let replay = Lab::new(&scope)
            .with_library(crate::config::config_dir()?)
            .curve(session_id.ordinal())?;
        let mut out = curve_output(&replay)?;
        out.stamp_session(&session_id.to_string());
        stamp_scope(
            &mut out,
            ctx,
            manifest
                .window
                .ended_at
                .or(Some(manifest.window.started_at)),
        )?;
        Ok(out)
    }
}

/// One row per point, with the fold's honesty disclosures beside the series:
/// `epochs_skipped` and `caveats` ride the payload and the table footer, so
/// a reset-truncated or hole-y recording never reads as a clean curve.
/// The fill ledger stays out — `explain pnl` and `session decisions` own it.
fn curve_output(replay: &kraken_lab::Replay) -> Result<CommandOutput> {
    let curve = &replay.curve;
    let headers = ["Time", "Value", "Complete"].map(String::from).to_vec();
    let rows = curve
        .points
        .iter()
        .map(|point| {
            vec![
                kraken_recording::format_instant(point.at),
                format!("{} {}", point.value, curve.currency),
                if point.complete { "yes" } else { "no" }.to_string(),
            ]
        })
        .collect();
    let data = serde_json::json!({
        "curve": serde_json::to_value(curve)?,
        "epochs_skipped": replay.epochs_skipped,
        "caveats": replay.caveats,
    });
    let mut footer: Vec<String> = replay.caveats.clone();
    if !curve.unmarked.is_empty() {
        let unmarked: Vec<&str> = curve.unmarked.iter().map(String::as_str).collect();
        footer.push(format!(
            "unmarked holdings (never valued): {}",
            unmarked.join(", ")
        ));
    }
    let mut out = CommandOutput::new(data, headers, rows);
    if !footer.is_empty() {
        out = out.with_footer(footer);
    }
    Ok(out)
}

/// Say which kind of money a scorecard was earned with.
///
/// A live workspace's sessions carry real venue fills, and the scorecard's own
/// `source.kind` cannot express that — it reports `live` for a live *market*,
/// which a paper session has too. The promotion gate's `live_run_passed`
/// heuristic reads exactly that field, so an unlabelled live scope is how real
/// money would come to count as paper evidence. The claim is per session:
/// a window anchored before promotion earned simulated money and says so.
fn stamp_scope(
    out: &mut CommandOutput,
    ctx: &AppContext,
    window_anchor: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<()> {
    match super::workspace_guard::active_mode(ctx)? {
        super::workspace_guard::ActiveMode::Live(manifest) => {
            let scope = ctx.scoped_base()?;
            super::workspace_guard::stamp_session_money(out, &manifest, &scope, window_anchor)?;
        }
        super::workspace_guard::ActiveMode::Paper(manifest) => out.stamp_workspace(&manifest.name),
        // Left unlabelled although it scores the real account's own fills:
        // `stamp_live_scope` would claim an allocation the scope lacks.
        super::workspace_guard::ActiveMode::Master => {}
    }
    Ok(())
}

impl Execute for New {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let experiment = lab_in(ctx)?.freeze(
            &self.name,
            &self.hypothesis,
            self.strategy,
            Criteria {
                min_return_pct: self.min_return_pct,
                max_drawdown_pct: self.max_drawdown_pct,
                min_fills: self.min_fills,
            },
            &self.sessions,
        )?;
        experiment_output(&experiment, "frozen")
    }
}

impl Execute for Show {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let experiment = lab_in(ctx)?.show(&self.name)?;
        experiment_output(&experiment, "verified")
    }
}

impl Execute for Compare {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let comparison = lab_in(ctx)?.compare(&self.name)?;
        let mut out = comparison_output(&comparison)?;
        stamp_compare_scope(&mut out, ctx, &comparison)?;
        Ok(out)
    }
}

/// [`stamp_scope`] over a whole experiment: the live stamp is earned only
/// when every scored session post-dates promotion. A mixed experiment keeps
/// the live stamp — real money is in it — and names its paper-era sessions,
/// so no scorecard's money source is left to inference.
fn stamp_compare_scope(
    out: &mut CommandOutput,
    ctx: &AppContext,
    comparison: &Comparison,
) -> Result<()> {
    let manifest = match super::workspace_guard::active_mode(ctx)? {
        super::workspace_guard::ActiveMode::Live(manifest) => manifest,
        super::workspace_guard::ActiveMode::Paper(manifest) => {
            out.stamp_workspace(&manifest.name);
            return Ok(());
        }
        super::workspace_guard::ActiveMode::Master => return Ok(()),
    };
    super::workspace_guard::warn_live_workspace(&manifest);
    let live_since = super::workspace_guard::live_since(&ctx.scoped_base()?)?;
    let scored: Vec<(&SessionResult, chrono::DateTime<chrono::Utc>)> = comparison
        .outcomes
        .iter()
        .filter_map(|outcome| match &outcome.result {
            OutcomeResult::Scored { card, .. } => Some((outcome, card.anchor.ended_at)),
            OutcomeResult::Failed { .. } => None,
        })
        .collect();
    let paper_era: Vec<String> = scored
        .iter()
        .filter(|(_, ended)| super::workspace_guard::predates_live(Some(*ended), live_since))
        .map(|(outcome, _)| outcome.session.session.clone())
        .collect();
    if !scored.is_empty() && paper_era.len() == scored.len() {
        out.stamp_paper_era_scope(&manifest.name);
    } else {
        out.stamp_live_scope(&manifest.name);
        out.stamp_paper_era_sessions(&paper_era);
    }
    Ok(())
}

impl Execute for Next {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let action = lab_in(ctx)?.next(&self.name)?;
        next_output(&self.name, &action)
    }
}

fn next_output(experiment: &str, action: &NextAction) -> Result<CommandOutput> {
    let data = serde_json::json!({
        "group": "lab",
        "type": "next",
        "experiment": experiment,
        "state": action.state,
        "session": action.session,
        "source": action.source,
        "command": action.command,
        "reason": action.reason,
    });
    let mut pairs = vec![
        ("Experiment".to_string(), experiment.to_string()),
        ("State".to_string(), action.state.as_str().to_string()),
        ("Reason".to_string(), action.reason.clone()),
    ];
    if let Some(run) = &action.session {
        pairs.push(("Session".to_string(), run.clone()));
    }
    if let Some(command) = &action.command {
        pairs.push(("Command".to_string(), command.clone()));
    }
    Ok(CommandOutput::key_value(pairs, data))
}

/// Sessions cover different tapes, so only the pre-registered verdict checks
/// have cross-session meaning. Dropped once tape identity is explicit.
const COMPARISON_CAVEAT: &str = "sessions cover different market windows; verdicts judge each \
                                 session against the frozen criteria — metric rows are context, \
                                 never a cross-session comparison";

/// The table line failed columns land on; `comparison_rows` keeps it trailing.
const ERROR_LINE: &str = "Error";

fn comparison_output(comparison: &Comparison) -> Result<CommandOutput> {
    let pass_count = comparison.pass_count();
    let total = comparison.total();
    let sessions = comparison
        .outcomes
        .iter()
        .map(session_entry)
        .collect::<Result<Vec<_>>>()?;
    let data = serde_json::json!({
        "group": "lab",
        "type": "comparison",
        "experiment": comparison.experiment,
        "frozen": comparison.frozen,
        "sessions": sessions,
        "pass_count": pass_count,
        "total": total,
        "caveats": [COMPARISON_CAVEAT],
    });

    if comparison.outcomes.is_empty() {
        return Ok(CommandOutput::key_value(
            vec![
                ("Experiment".to_string(), comparison.experiment.clone()),
                ("Sessions".to_string(), "none".to_string()),
                ("Pass count".to_string(), "0 of 0".to_string()),
            ],
            data,
        ));
    }
    let headers = std::iter::once("Line".to_string())
        .chain(
            comparison
                .outcomes
                .iter()
                .map(|outcome| outcome.session.session.clone()),
        )
        .collect();
    // Comparison-wide notes ride below the table: in a per-session column they'd
    // read as the leftmost session's own value.
    let footer = vec![
        format!("Pass count: {pass_count} of {total}"),
        format!("Caveat: {COMPARISON_CAVEAT}"),
    ];
    Ok(
        CommandOutput::new(data, headers, comparison_rows(&comparison.outcomes))
            .with_footer(footer),
    )
}

fn session_entry(outcome: &SessionResult) -> Result<serde_json::Value> {
    let mut entry = serde_json::Map::new();
    entry.insert("session".into(), serde_json::json!(outcome.session.session));
    entry.insert("n".into(), serde_json::json!(outcome.session.ordinal));
    match &outcome.result {
        OutcomeResult::Scored { card, verdict } => {
            entry.insert("verdict".into(), serde_json::to_value(verdict)?);
            // The anchor and curve ride along so an agent never needs N
            // separate `lab score` calls to see completeness per session.
            entry.insert("anchor".into(), serde_json::to_value(&card.anchor)?);
            entry.insert("metrics".into(), serde_json::to_value(&card.metrics)?);
            entry.insert("trades".into(), serde_json::to_value(&card.trades)?);
            entry.insert("curve".into(), serde_json::to_value(&card.curve)?);
            // First-class provenance: the promotion gate reads source.kind
            // here instead of string-matching the caveats for "replayed from".
            entry.insert("source".into(), serde_json::to_value(&card.source)?);
            entry.insert("caveats".into(), serde_json::to_value(&card.caveats)?);
        }
        OutcomeResult::Failed { error } => {
            entry.insert("error".into(), serde_json::json!(error));
            entry.insert("caveats".into(), serde_json::json!([]));
        }
    }
    Ok(serde_json::Value::Object(entry))
}

/// One row per line, one value column per session; verdict lines lead, context
/// lines follow in first-seen order, failures land on a trailing Error line.
fn comparison_rows(outcomes: &[SessionResult]) -> Vec<Vec<String>> {
    let per_column: Vec<Vec<(String, String)>> = outcomes
        .iter()
        .map(|outcome| column_pairs(&outcome.result))
        .collect();

    let mut lines: Vec<&str> = Vec::new();
    for (line, _) in per_column.iter().flatten() {
        if line != ERROR_LINE && !lines.contains(&line.as_str()) {
            lines.push(line);
        }
    }
    if per_column
        .iter()
        .flatten()
        .any(|(line, _)| line == ERROR_LINE)
    {
        lines.push(ERROR_LINE);
    }

    lines
        .iter()
        .map(|line| {
            std::iter::once((*line).to_string())
                .chain(per_column.iter().map(|pairs| {
                    pairs
                        .iter()
                        .find(|(l, _)| l == line)
                        .map_or_else(|| "—".to_string(), |(_, value)| value.clone())
                }))
                .collect()
        })
        .collect()
}

/// A session's (line, value) cells, reusing the scorecard row builders so cells
/// render exactly as `lab score` — new metric rows surface here automatically.
fn column_pairs(result: &OutcomeResult) -> Vec<(String, String)> {
    match result {
        OutcomeResult::Scored { card, verdict } => {
            let mut pairs = vec![(
                "Verdict".to_string(),
                if verdict.pass { "PASS" } else { "FAIL" }.to_string(),
            )];
            // The frozen threshold is identical in every column, so it lives
            // in the shared line label.
            pairs.extend(verdict.checks.iter().map(|check| {
                (
                    format!("{} (threshold {})", check.criterion, check.threshold),
                    check_cell(check),
                )
            }));
            let context = [source_row(card), total_row(card)]
                .into_iter()
                .chain(metric_rows(&card.metrics))
                .chain([trades_row(&card.trades), curve_row(&card.curve)]);
            pairs.extend(context.map(|row| (row[0].clone(), row[1].clone())));
            // One joined line: comparison_rows keys cells by line name.
            if !card.caveats.is_empty() {
                pairs.push(("Session caveats".to_string(), card.caveats.join("; ")));
            }
            pairs
        }
        OutcomeResult::Failed { error } => vec![
            ("Verdict".to_string(), "error".to_string()),
            (ERROR_LINE.to_string(), error.clone()),
        ],
    }
}

fn check_cell(check: &Check) -> String {
    format!(
        "{} ({})",
        if check.pass { "pass" } else { "fail" },
        observed(check)
    )
}

fn observed(check: &Check) -> String {
    check
        .observed
        .map_or_else(|| "unknown".to_string(), |v| v.to_string())
}

/// Field/Value rows for a sealed experiment; `state` says what just
/// happened to the seal ("frozen" at creation, "verified" on show).
fn experiment_output(experiment: &Experiment, state: &str) -> Result<CommandOutput> {
    let mut pairs = vec![
        ("Experiment".to_string(), experiment.name.clone()),
        ("Hypothesis".to_string(), experiment.hypothesis.clone()),
    ];
    if let Some(strategy) = &experiment.strategy {
        pairs.push(("Strategy".to_string(), strategy.clone()));
    }
    if let Some(pct) = experiment.criteria.min_return_pct {
        pairs.push(("Min return".to_string(), format!("{pct}%")));
    }
    if let Some(pct) = experiment.criteria.max_drawdown_pct {
        pairs.push(("Max drawdown".to_string(), format!("{pct}%")));
    }
    if let Some(fills) = experiment.criteria.min_fills {
        pairs.push(("Min fills".to_string(), fills.to_string()));
    }
    if let Some(plan) = &experiment.session_plan {
        for (position, leg) in plan.0.iter().enumerate() {
            let value = match leg {
                PlannedSession::Live { window } => format!("live {window}"),
                PlannedSession::Replay { tape, content_hash } => {
                    format!("replay {tape} ({content_hash})")
                }
            };
            pairs.push((format!("Plan {}", position + 1), value));
        }
    }
    pairs.push((
        "Created".to_string(),
        kraken_recording::format_instant(experiment.created_at),
    ));
    pairs.push((
        "Seal".to_string(),
        format!("{} ({state})", experiment.frozen),
    ));
    Ok(CommandOutput::key_value(
        pairs,
        serde_json::to_value(experiment)?,
    ))
}

fn output(scored: &ScoredSession) -> Result<CommandOutput> {
    let card = &scored.card;
    let headers = ["Line", "Value", "Explanation"].map(String::from).to_vec();
    let mut rows = vec![source_row(card), total_row(card)];
    rows.extend(metric_rows(&card.metrics));
    rows.push(trades_row(&card.trades));
    rows.push(curve_row(&card.curve));
    if let Some(judged) = &scored.judged {
        rows.extend(verdict_rows(judged));
    }
    rows.extend(card.caveats.iter().map(|caveat| caveat_row(caveat)));

    // The Scorecard stays the payload; a judged run adds the experiment name
    // and verdict as additive top-level fields.
    let mut data = serde_json::to_value(card)?;
    if let (Some(obj), Some(judged)) = (data.as_object_mut(), &scored.judged) {
        obj.insert("experiment".into(), serde_json::json!(judged.experiment));
        obj.insert("verdict".into(), serde_json::to_value(&judged.verdict)?);
    }
    Ok(CommandOutput::new(data, headers, rows))
}

fn verdict_rows(judged: &Judged) -> Vec<Vec<String>> {
    let mut rows = vec![vec![
        "Verdict".to_string(),
        if judged.verdict.pass { "PASS" } else { "FAIL" }.to_string(),
        format!(
            "against experiment '{}' ({})",
            judged.experiment, judged.frozen
        ),
    ]];
    rows.extend(judged.verdict.checks.iter().map(|check| {
        vec![
            "Check".to_string(),
            if check.pass { "pass" } else { "fail" }.to_string(),
            format!(
                "{}: threshold {}, observed {}",
                check.criterion,
                check.threshold,
                observed(check)
            ),
        ]
    }));
    rows
}

/// The session's market-data provenance, read from the scorecard (never
/// asserted).
fn source_row(card: &Scorecard) -> Vec<String> {
    let (value, explanation) = match &card.source {
        ScoreSource::Live {
            symbols,
            window_secs,
        } => (
            // The actual exposure rides in the value cell so `lab compare`
            // (which keeps only values) shows a cut-short live leg too.
            window_secs.map_or_else(
                || "live".to_string(),
                |secs| {
                    format!(
                        "live · {}",
                        humantime::format_duration(std::time::Duration::from_secs(secs))
                    )
                },
            ),
            if symbols.is_empty() {
                "live market".to_string()
            } else {
                format!("live market: {}", symbols.join(", "))
            },
        ),
        ScoreSource::Replay { tape, speed } => {
            (format!("replay @{speed}x"), format!("recorded {tape}"))
        }
    };
    vec!["Source".to_string(), value, explanation]
}

fn total_row(card: &Scorecard) -> Vec<String> {
    vec![
        "Total".to_string(),
        signed(card.anchor.total_pnl),
        format!(
            "ended at {:.2} {} from a {:.2} start (as reported at session stop)",
            rounded(card.anchor.final_value, 2),
            card.anchor.currency,
            rounded(card.anchor.starting_balance, 2)
        ),
    ]
}

/// The Return cell at the row's 0.01% resolution. A real gain or loss too
/// small to show keeps its sign and is flagged sub-resolution (`-<0.01%` /
/// `+<0.01%`) rather than collapsing to a signed zero; `+0.00%` is reserved
/// for an exactly-zero return. The money rows use `signed`, whose deliberate
/// clamp to `+0.00` is the opposite convention.
fn return_cell(pct: Decimal) -> String {
    let shown = rounded(pct, 2);
    if shown.is_zero() && !pct.is_zero() {
        let sign = if pct.is_sign_negative() { '-' } else { '+' };
        return format!("{sign}<0.01%");
    }
    format!("{shown:+.2}%")
}

fn metric_rows(metrics: &Metrics) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    rows.push(vec![
        "Return".to_string(),
        metrics
            .return_pct
            .map_or_else(|| "?".to_string(), return_cell),
        "total P&L relative to the epoch's starting balance".to_string(),
    ]);
    rows.push(vec![
        "Max drawdown".to_string(),
        metrics
            .max_drawdown
            .map_or_else(|| "—".to_string(), |dd| format!("-{:.2}", rounded(dd, 2))),
        // The two maxima can anchor on different peaks, so the pct is its
        // own statement, never a ratio of the absolute drop shown left.
        metrics.max_drawdown_pct.map_or_else(
            || "largest peak-to-trough drop on the marked curve".to_string(),
            |pct| {
                format!(
                    "largest peak-to-trough drop on the marked curve; worst drop vs its peak: \
                     {:.2}%",
                    rounded(pct, 2)
                )
            },
        ),
    ]);
    if let Some(fees) = metrics.fees {
        rows.push(vec![
            "Fees".to_string(),
            signed(fees),
            "the waterfall's fees line, carried verbatim".to_string(),
        ]);
    }
    if let Some(friction) = metrics.friction {
        rows.push(vec![
            "Friction".to_string(),
            signed(friction),
            "spread + slippage from the waterfall".to_string(),
        ]);
    }
    if let Some(turnover) = metrics.turnover_pct {
        rows.push(vec![
            "Turnover".to_string(),
            format!("{:.2}%", rounded(turnover, 2)),
            "total converted fill notional over the starting balance".to_string(),
        ]);
    }
    if let Some(hit_rate) = metrics.hit_rate {
        rows.push(vec![
            "Hit rate".to_string(),
            format!("{:.0}%", rounded(hit_rate * Decimal::ONE_HUNDRED, 0)),
            "closed FIFO round trips won net of pro-rata fees; open lots are never graded"
                .to_string(),
        ]);
    }
    if let Some(profit_factor) = metrics.profit_factor {
        rows.push(vec![
            "Profit factor".to_string(),
            format!("{:.2}", rounded(profit_factor, 2)),
            "gross profit over gross loss across closed round trips".to_string(),
        ]);
    }
    if let Some(sensitivity) = metrics.slippage_sensitivity {
        rows.push(vec![
            "Slippage sensitivity".to_string(),
            format!("{} per bp", signed(sensitivity)),
            "P&L change per basis point of extra slippage (analytic, assumes linearity)"
                .to_string(),
        ]);
    }
    rows
}

fn trades_row(trades: &TradeStats) -> Vec<String> {
    vec![
        "Trades".to_string(),
        trades.fills.to_string(),
        format!(
            "{} buy(s), {} sell(s); {} with a logged reason, {} counterfactual missed fill(s)",
            trades.buys, trades.sells, trades.decided, trades.missed_fills
        ),
    ]
}

fn curve_row(curve: &CurveSummary) -> Vec<String> {
    let span = curve
        .first_value
        .zip(curve.last_value)
        .map(|(first, last)| {
            format!(
                "{:.2} → {:.2} {}",
                rounded(first, 2),
                rounded(last, 2),
                curve.currency
            )
        })
        .unwrap_or_else(|| "no points".to_string());
    let completeness = if curve.complete {
        String::new()
    } else {
        format!(
            "; unmarked holdings: {}",
            curve
                .unmarked
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    vec![
        "Curve".to_string(),
        format!("{} point(s)", curve.points),
        format!("{span}{completeness}"),
    ]
}

fn caveat_row(caveat: &str) -> Vec<String> {
    vec!["Caveat".to_string(), "—".to_string(), caveat.to_string()]
}

/// Signed money for the Value column. A rounded zero clamps to +0.00 so a
/// break-even line never reads as a loss.
fn signed(amount: Decimal) -> String {
    let cents = rounded(amount, 2);
    let amount = if cents.is_zero() {
        Decimal::ZERO
    } else {
        cents
    };
    format!("{amount:+.2}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kraken_lab::SessionRef;
    use kraken_paper::{OrderSide, account::Origin};
    use kraken_session::{
        manifest::SessionOutcome,
        timeline::fixtures::{self, SessionBuilder},
    };
    use rust_decimal_macros::dec;

    // Regression: a signed return floor must survive clap parsing. Without
    // `allow_negative_numbers`, `--min-return-pct -5` fails with "unexpected
    // argument '-5'" and the user has to discover the `=` form.
    #[test]
    fn min_return_pct_accepts_a_negative_threshold() {
        use clap::Parser;
        #[derive(Parser)]
        struct TestCli {
            #[command(subcommand)]
            cmd: LabCommand,
        }
        let cli = TestCli::try_parse_from([
            "kraken",
            "new",
            "exp",
            "--hypothesis",
            "h",
            "--min-return-pct",
            "-5",
        ])
        .expect("negative --min-return-pct parses");
        let LabCommand::New(new) = cli.cmd else {
            panic!("expected the `new` subcommand");
        };
        assert_eq!(new.min_return_pct, Some(dec!(-5)));
    }

    fn scorecard() -> Scorecard {
        let mut builder = SessionBuilder::with_rates(dec!(10_000), dec!(0.0026), dec!(0.0));
        builder.ticker(
            "2026-01-01T00:00:01Z",
            "BTC/USD",
            dec!(50_000),
            dec!(50_000),
        );
        builder.market_fill(
            "2026-01-01T00:00:02Z",
            Origin::Cli,
            OrderSide::Buy,
            "BTCUSD",
            dec!(0.1),
            (dec!(50_000), dec!(50_000)),
        );
        let mut manifest = fixtures::manifest(vec![]);
        manifest.summary = Some(SessionOutcome {
            ended_at: "2026-01-01T00:00:04+00:00".parse().unwrap(),
            final_value: dec!(10_187),
            pnl: dec!(187),
        });
        kraken_lab::score(&builder.session(None, manifest)).unwrap()
    }

    fn experiment(min_return_pct: Decimal) -> Experiment {
        Experiment {
            name: "m1".into(),
            hypothesis: "h".into(),
            strategy: None,
            criteria: Criteria {
                min_return_pct: Some(min_return_pct),
                max_drawdown_pct: None,
                min_fills: None,
            },
            created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
            session_plan: None,
            frozen: "sha256:test".into(),
        }
    }

    fn run(n: u32) -> SessionRef {
        SessionRef {
            ordinal: n,
            session: format!("s{n}"),
        }
    }

    fn scored_outcome(experiment: &Experiment, n: u32) -> SessionResult {
        let card = scorecard();
        let verdict = experiment.criteria.evaluate(&card);
        SessionResult {
            session: run(n),
            result: OutcomeResult::Scored {
                card: Box::new(card),
                verdict,
            },
        }
    }

    fn failed_outcome(n: u32, error: &str) -> SessionResult {
        SessionResult {
            session: run(n),
            result: OutcomeResult::Failed {
                error: error.to_string(),
            },
        }
    }

    fn comparison(experiment: &Experiment, outcomes: Vec<SessionResult>) -> Comparison {
        Comparison {
            experiment: experiment.name.clone(),
            frozen: experiment.frozen.clone(),
            outcomes,
        }
    }

    #[test]
    fn json_payload_is_the_scorecard_verbatim() {
        let scored = ScoredSession {
            card: scorecard(),
            judged: None,
        };
        let out = output(&scored).unwrap();
        assert_eq!(
            out.data,
            serde_json::to_value(&scored.card).unwrap(),
            "the -o json payload is the crate's serialized Scorecard, untouched"
        );
    }

    #[test]
    fn table_leads_with_source_then_the_anchor_total() {
        let scored = ScoredSession {
            card: scorecard(),
            judged: None,
        };
        let out = output(&scored).unwrap();
        let rows = &out.rows;
        // Provenance first (the fixture has no source block ⇒ live), then the
        // anchor total, then the metric/context rows.
        assert_eq!(rows[0][0], "Source");
        assert_eq!(rows[0][1], "live");
        assert_eq!(rows[1][0], "Total");
        assert_eq!(rows[1][1], "+187.00");
        let lines: Vec<&str> = rows.iter().map(|row| row[0].as_str()).collect();
        for expected in ["Return", "Max drawdown", "Fees", "Trades", "Curve"] {
            assert!(lines.contains(&expected), "{expected} missing in {lines:?}");
        }
    }

    #[test]
    fn return_row_renders_the_percentage() {
        let scored = ScoredSession {
            card: scorecard(),
            judged: None,
        };
        let out = output(&scored).unwrap();
        let ret = out
            .rows
            .iter()
            .find(|row| row[0] == "Return")
            .expect("return row")
            .clone();
        assert_eq!(ret[1], "+1.87%");
    }

    /// Below the row's 0.01% resolution a real gain or loss keeps its sign and
    /// is flagged sub-resolution, so it never masquerades as break-even.
    #[test]
    fn return_cell_flags_direction_below_resolution() {
        assert_eq!(return_cell(dec!(-0.0046)), "-<0.01%");
        assert_eq!(return_cell(dec!(0.0046)), "+<0.01%");
    }

    /// `+0.00%` reads only for an exactly-zero return, never a rounded loss.
    #[test]
    fn return_cell_break_even_is_exact_zero_only() {
        assert_eq!(return_cell(dec!(0)), "+0.00%");
    }

    #[test]
    fn comparison_json_pins_the_top_level_keys() {
        let experiment = experiment(dec!(1.0));
        let comparison = comparison(
            &experiment,
            vec![
                scored_outcome(&experiment, 1),
                failed_outcome(2, "validation: no final summary"),
            ],
        );
        let data = comparison_output(&comparison).unwrap().data;
        assert_eq!(data["group"], "lab");
        assert_eq!(data["type"], "comparison");
        assert_eq!(data["experiment"], "m1");
        assert_eq!(data["frozen"], "sha256:test");
        assert_eq!(data["pass_count"], 1);
        assert_eq!(data["total"], 2);
        assert_eq!(data["sessions"][0]["session"], "s1");
        assert_eq!(data["sessions"][0]["n"], 1);
        assert_eq!(data["sessions"][1]["error"], "validation: no final summary");
        assert!(
            data["sessions"][1].get("verdict").is_none(),
            "a failed column carries no verdict"
        );
        assert_eq!(data["caveats"][0], COMPARISON_CAVEAT);
    }

    /// One card on both sides: pins pass-through, not fixture determinism.
    #[test]
    fn comparison_cells_serialize_byte_equal_to_the_source_scorecard() {
        let experiment = experiment(dec!(1.0));
        let card = scorecard();
        let verdict = experiment.criteria.evaluate(&card);
        let expected = [
            ("metrics", serde_json::to_value(&card.metrics).unwrap()),
            ("trades", serde_json::to_value(&card.trades).unwrap()),
            ("verdict", serde_json::to_value(&verdict).unwrap()),
            ("caveats", serde_json::to_value(&card.caveats).unwrap()),
        ];
        let entry = session_entry(&SessionResult {
            session: run(1),
            result: OutcomeResult::Scored {
                card: Box::new(card),
                verdict,
            },
        })
        .unwrap();
        for (key, expected) in expected {
            assert_eq!(
                serde_json::to_vec(&entry[key]).unwrap(),
                serde_json::to_vec(&expected).unwrap(),
                "{key} must cross into the comparison verbatim"
            );
        }
    }

    #[test]
    fn pass_count_counts_only_scored_passing_columns() {
        let passing = experiment(dec!(1.0));
        let failing = experiment(dec!(100.0));
        let comparison = comparison(
            &passing,
            vec![
                scored_outcome(&passing, 1),
                scored_outcome(&failing, 2),
                failed_outcome(3, "validation: unstopped"),
            ],
        );
        let data = comparison_output(&comparison).unwrap().data;
        assert_eq!(data["pass_count"], 1);
        assert_eq!(data["total"], 3);
    }

    #[test]
    fn pass_count_reaches_total_when_every_scored_column_passes() {
        let passing = experiment(dec!(1.0));
        let comparison = comparison(
            &passing,
            vec![scored_outcome(&passing, 1), scored_outcome(&passing, 2)],
        );
        let data = comparison_output(&comparison).unwrap().data;
        assert_eq!(data["pass_count"], 2);
        assert_eq!(data["total"], 2);
    }

    #[test]
    fn comparison_of_no_sessions_is_empty_not_an_error() {
        let experiment = experiment(dec!(1.0));
        let out = comparison_output(&comparison(&experiment, vec![])).unwrap();
        assert_eq!(out.data["sessions"], serde_json::json!([]));
        assert_eq!(out.data["pass_count"], 0);
        assert_eq!(out.data["total"], 0);
    }

    #[test]
    fn comparison_table_leads_with_verdicts_and_names_failures() {
        let experiment = experiment(dec!(1.0));
        let comparison = comparison(
            &experiment,
            vec![
                scored_outcome(&experiment, 1),
                failed_outcome(2, "validation: no final summary"),
            ],
        );
        let out = comparison_output(&comparison).unwrap();
        assert_eq!(out.headers, ["Line", "s1", "s2"]);
        assert_eq!(out.rows[0][0], "Verdict");
        assert_eq!(out.rows[0][1], "PASS");
        assert_eq!(out.rows[0][2], "error");
        // Raw Decimal display, matching `lab score`'s check rows exactly.
        assert_eq!(
            out.rows[1],
            ["min_return_pct (threshold 1.0)", "pass (1.8700)", "—"]
        );
        let error_row = out
            .rows
            .iter()
            .find(|row| row[0] == "Error")
            .expect("error row");
        assert_eq!(error_row[1], "—");
        assert_eq!(error_row[2], "validation: no final summary");
        // Comparison-wide notes leave the table body and land in the footer,
        // so they never sit under a session's header and read as its value.
        assert!(
            out.rows
                .iter()
                .all(|row| row[0] != "Pass count" && row[0] != "Caveat"),
            "comparison-wide notes belong in the footer, not a per-session row"
        );
        assert_eq!(
            out.footer,
            [
                "Pass count: 1 of 2".to_string(),
                format!("Caveat: {COMPARISON_CAVEAT}"),
            ]
        );
    }
}

delegate_execute!(LabCommand {
    Score,
    Curve,
    New,
    Show,
    Compare,
    Next
});
