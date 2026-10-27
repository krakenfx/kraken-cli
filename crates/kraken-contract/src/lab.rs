//! `kraken lab`: arguments for scoring recorded sessions against their tape.

use clap::Subcommand;
use rust_decimal::Decimal;

/// The `lab` family: scoring recorded sessions against the tape they ran on,
/// and sealing the experiments that a promotion reads as evidence.
#[derive(Debug, Subcommand, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LabCommand {
    /// Score a stopped session: the explain-pnl waterfall plus mechanical
    /// metrics over the marked curve.
    ///
    /// A source block naming the session's provenance (replay tape and speed,
    /// or live and symbols), the explain-pnl waterfall carried verbatim,
    /// and mechanical metrics — return, max drawdown, fees, friction,
    /// turnover, hit rate, profit factor, slippage sensitivity, trade
    /// counts — over the session's own recorded marks. Hit rate and profit
    /// factor grade closed FIFO round trips only and read null when no
    /// lot closed. Needs `kraken session stop` to have run first. With
    /// `--experiment`, judges the scorecard against a frozen experiment's
    /// criteria and adds a mechanical pass/fail verdict.
    Score(Score),
    /// The marked equity curve point by point — the full series `lab score`
    /// summarizes.
    ///
    /// One point per valuation-changing instant with its timestamp, value and
    /// completeness flag, so a consumer can compute its own metrics (Sharpe,
    /// Sortino, rolling drawdown) without re-deriving the fold from the
    /// journal and tape. Refuses the same sessions `lab score` refuses:
    /// still recording, no final summary, a partial journal, or no account
    /// epoch.
    Curve(Curve),
    /// Pre-register an experiment: freeze a hypothesis and its success criteria
    /// (hash-sealed) before any session.
    ///
    /// The seal is what makes a post-hoc goalpost move detectable; a frozen
    /// spec is immutable.
    New(New),
    /// Show a frozen experiment, verifying its seal.
    Show(Show),
    /// Compare every session of a frozen experiment side by side: each session
    /// re-scored from disk and judged against the sealed criteria.
    ///
    /// Per-session verdicts are the only cross-session comparable numbers,
    /// because sessions cover different market windows. A session that
    /// cannot be scored appears as a named error column, never an omission,
    /// and `pass_count` counts only mechanically scored passing verdicts.
    Compare(Compare),
    /// What the sealed session plan says to do now: the exact start command for
    /// the next run, the session still in progress, or `plan_complete`
    /// pointing at `kraken lab compare`.
    ///
    /// Progress is re-derived from the sessions tree on every call; aborted
    /// sessions keep their ordinals and never count toward the plan.
    /// Refuses to hand out a replay session whose tape no longer matches
    /// its sealed content hash.
    Next(Next),
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Score {
    /// Which session: `latest`, an ordinal (`s3`), or a label.
    #[arg(long, default_value = "latest")]
    #[serde(default = "crate::value_types::latest_session")]
    pub session: String,
    /// Judge the scorecard against a frozen experiment's criteria; the
    /// output gains a mechanical pass/fail verdict.
    #[arg(long)]
    pub experiment: Option<String>,
}

// A loss-tolerant return floor (e.g. `--min-return-pct -5`) is a valid
// criterion, so a leading-minus number must bind as the option's value rather
// than trip clap into reading it as an unknown flag.
#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[command(allow_negative_numbers = true)]
pub struct New {
    /// Experiment name (session charset; becomes session names).
    pub name: String,
    /// The falsifiable claim being tested.
    #[arg(long)]
    pub hypothesis: String,
    /// The strategy under test, if one is named.
    #[arg(long)]
    pub strategy: Option<String>,
    /// Success: return of at least this percent.
    #[arg(long)]
    pub min_return_pct: Option<Decimal>,
    /// Success: worst drop relative to its peak (max percentage
    /// drawdown) of at most this percent.
    // The two maxima can anchor on different peaks, so the pct is its
    // own statement, never a ratio of the absolute drop shown left.
    #[arg(long)]
    pub max_drawdown_pct: Option<Decimal>,
    /// Success: at least this many fills (guards a pass-by-not-trading).
    #[arg(long)]
    pub min_fills: Option<u64>,
    /// Pre-register a session (repeatable, ordered): `live:<window>` or
    /// `replay:<ref>`. Replay tapes must exist and be finalized — their
    /// content hash is sealed with the spec.
    #[arg(long = "session")]
    #[serde(default)]
    pub sessions: Vec<String>,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Show {
    /// Experiment name.
    pub name: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Compare {
    /// Experiment name.
    pub name: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Next {
    /// Experiment name.
    pub name: String,
}

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Curve {
    /// Which session: `latest`, an ordinal (`s3`), or a label.
    #[arg(long, default_value = "latest")]
    #[serde(default = "crate::value_types::latest_session")]
    pub session: String,
}

impl New {
    /// Whether the plan commits this workspace to replaying a tape. Read by
    /// [`kraken-cli's workspace guard`]: a seal is irreversible, so a leg
    /// the scope cannot run is refused here rather than stranding the plan at
    /// `lab next`. Matched on the prefix the plan parser splits on, so an
    /// unparseable entry stays the lab's error to report, not the guard's.
    pub fn plans_a_replay_leg(&self) -> bool {
        self.sessions
            .iter()
            .any(|entry| entry.starts_with(REPLAY_LEG_PREFIX))
    }
}

/// The `--session` prefix `kraken_lab` splits a replay leg on.
const REPLAY_LEG_PREFIX: &str = "replay:";

crate::tools! {
    /// The `kraken lab` commands.
    fn lab in ["lab"] of LabCommand {
        Score   as "score"   => Paper, Local,    None, Tool
            example "kraken lab score --session latest --experiment momentum-1 -o json";
        Curve   as "curve"   => Paper, ReadOnly, None, Tool
            example "kraken lab curve --session latest -o json";
        New     as "new"     => Paper, Local,    None, Tool
            example "kraken lab new momentum-1 --hypothesis 'buying strength beats the tape' --min-return-pct 1 --min-fills 2 -o json";
        Show    as "show"    => Paper, ReadOnly, None, Tool
            example "kraken lab show momentum-1 -o json";
        Compare as "compare" => Paper, ReadOnly, None, Tool
            example "kraken lab compare momentum-1 -o json";
        Next    as "next"    => Paper, Local,    None, Tool
            example "kraken lab next dca-robust-1 -o json";
    }
}
