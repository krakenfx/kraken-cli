//! The `Workspaces` facade — the one seam the binary dispatches to.

use crate::{
    Result, WorkspaceError,
    backing::Claims,
    manifest::{WorkspaceManifest, WorkspaceMode},
    naming::{DEFAULT_WORKSPACE, WorkspaceName},
    store,
};
use chrono::{DateTime, Utc};
use kraken_paper::{
    AccountEvent, CommandEntry, CommandOutcome, PaperConfig, PaperState,
    account::{Origin, PaperAccount},
};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_with::skip_serializing_none;
use std::{collections::BTreeMap, path::PathBuf};

/// Validated input for workspace creation; storage owns the timestamp.
#[derive(Debug, Clone)]
pub struct CreateSpec {
    pub name: WorkspaceName,
    pub capital: Decimal,
    pub currency: String,
    pub mode: WorkspaceMode,
    pub fee_rate: Decimal,
    pub slippage_rate: Decimal,
    /// `None` = unrestricted; `Some([])` = deny-all.
    pub allowed_pairs: Option<Vec<String>>,
}

/// Reset overrides written atomically with the new journal epoch.
#[derive(Debug, Clone, Default)]
pub struct ResetOverrides {
    pub capital: Option<Decimal>,
    pub currency: Option<String>,
    pub fee_rate: Option<Decimal>,
    pub slippage_rate: Option<Decimal>,
}

impl ResetOverrides {
    fn is_empty(&self) -> bool {
        self.capital.is_none()
            && self.currency.is_none()
            && self.fee_rate.is_none()
            && self.slippage_rate.is_none()
    }
}

/// Which account a workspace command addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The real Kraken account on the master credentials — never created,
    /// never locally funded; its balance is the venue's.
    Default,
    Named(WorkspaceName),
}

impl Target {
    /// Resolve an optional user-supplied name: `default` (or nothing) is the
    /// real account, anything else must be a valid workspace name.
    pub fn resolve(name: Option<&str>) -> Result<Self> {
        match name {
            None => Ok(Self::Default),
            Some(DEFAULT_WORKSPACE) => Ok(Self::Default),
            Some(named) => Ok(Self::Named(named.parse()?)),
        }
    }
}

/// One `workspace list` row. The synthesized `default` row carries no local
/// contract fields — its account lives at the venue.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct ListedWorkspace {
    pub name: String,
    pub mode: WorkspaceMode,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub capital: Option<Decimal>,
    pub currency: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    /// Why the contract could not be read — and only then.
    pub damaged: Option<String>,
}

/// The `workspace show` payload.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct WorkspaceOverview {
    pub name: String,
    pub mode: WorkspaceMode,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub capital: Option<Decimal>,
    pub currency: Option<String>,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub fee_rate: Option<Decimal>,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub slippage_rate: Option<Decimal>,
    pub allowed_pairs: Option<Vec<String>>,
    pub created_at: Option<DateTime<Utc>>,
    pub workspace_version: Option<String>,
}

impl WorkspaceOverview {
    fn synthesized_default() -> Self {
        Self {
            name: DEFAULT_WORKSPACE.to_string(),
            mode: WorkspaceMode::Live,
            capital: None,
            currency: None,
            fee_rate: None,
            slippage_rate: None,
            allowed_pairs: None,
            created_at: None,
            workspace_version: None,
        }
    }
}

impl From<WorkspaceManifest> for WorkspaceOverview {
    fn from(manifest: WorkspaceManifest) -> Self {
        Self {
            name: manifest.name,
            mode: manifest.mode,
            capital: Some(manifest.capital),
            currency: Some(manifest.currency),
            fee_rate: Some(manifest.fee_rate),
            slippage_rate: Some(manifest.slippage_rate),
            allowed_pairs: manifest.allowed_pairs,
            created_at: Some(manifest.created_at),
            workspace_version: Some(manifest.workspace_version),
        }
    }
}

/// One asset's balance, folded from the journal.
#[derive(Debug, Serialize)]
pub struct AssetBalance {
    #[serde(with = "rust_decimal::serde::str")]
    pub total: Decimal,
    /// Held against resting limit orders.
    #[serde(with = "rust_decimal::serde::str")]
    pub reserved: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub available: Decimal,
}

/// The locally projected `workspace balance` payload.
#[derive(Debug, Serialize)]
pub struct WorkspaceBalances {
    pub workspace: String,
    pub mode: WorkspaceMode,
    pub balances: BTreeMap<String, AssetBalance>,
}

/// Application service over the workspace store. `base` is the outer config
/// dir, never a scoped root: the facade derives each workspace's tree itself.
#[derive(Debug)]
pub struct Workspaces {
    base: PathBuf,
}

impl Workspaces {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// The gate for scoping into a named workspace: a typo, a never-created
    /// name, or a damaged contract must refuse rather than silently trade the
    /// wrong account.
    pub fn ensure_exists(&self, name: &WorkspaceName) -> Result<()> {
        store::ensure_exists(&self.base, name)
    }

    /// Creates and funds as one logical operation; failed funding removes the
    /// contract.
    pub fn create(&self, spec: CreateSpec, origin: Origin) -> Result<WorkspaceManifest> {
        if spec.name.is_default() {
            return Err(WorkspaceError::CapitalManaged(format!(
                "'{DEFAULT_WORKSPACE}' is the real Kraken account; its capital is managed by the \
                 venue. Create a named workspace instead."
            )));
        }
        crate::policy::ensure_mode_creatable(spec.mode, spec.name.as_ref())?;
        kraken_paper::validate_money(spec.capital, "capital")?;
        ensure_rate(spec.fee_rate, "fee rate")?;
        ensure_rate(spec.slippage_rate, "slippage rate")?;
        // Persist canonical pairs so order-time policy cannot diverge from engine
        // parsing.
        let spec = CreateSpec {
            allowed_pairs: spec
                .allowed_pairs
                .as_deref()
                .map(crate::policy::canonicalize_pairs)
                .transpose()?,
            ..spec
        };

        let manifest = store::create(&self.base, &spec)?;
        let funded = self.fund(&spec, origin);
        if let Err(err) = funded {
            store::discard_created(&self.base, &spec.name);
            return Err(err);
        }
        Ok(manifest)
    }

    fn fund(&self, spec: &CreateSpec, origin: Origin) -> Result<()> {
        let mut account = PaperAccount::open_at(self.journal_path(&spec.name), origin)?;
        let config = PaperConfig {
            balance: spec.capital,
            currency: spec.currency.clone(),
            fee_rate: spec.fee_rate,
            slippage_rate: spec.slippage_rate,
        };
        let records = account.stamp(vec![AccountEvent::Initialized(config)]);
        account.append_if_uninitialized(&records)?;
        Ok(())
    }

    /// Every account, the real one first: `default` is synthesized (mode
    /// live, venue-held capital), then the named workspaces sorted by name.
    pub fn list(&self) -> Result<Vec<ListedWorkspace>> {
        let mut rows = vec![ListedWorkspace {
            name: DEFAULT_WORKSPACE.to_string(),
            mode: WorkspaceMode::Live,
            capital: None,
            currency: None,
            created_at: None,
            damaged: None,
        }];
        for (name, manifest) in store::list(&self.base)? {
            rows.push(match manifest {
                Ok(manifest) => ListedWorkspace {
                    name: manifest.name,
                    mode: manifest.mode,
                    capital: Some(manifest.capital),
                    currency: Some(manifest.currency),
                    created_at: Some(manifest.created_at),
                    damaged: None,
                },
                Err(err) => ListedWorkspace {
                    name: name.to_string(),
                    // The mode is unreadable; paper is the only creatable
                    // mode, so it is the honest placeholder.
                    mode: WorkspaceMode::Paper,
                    capital: None,
                    currency: None,
                    created_at: None,
                    damaged: Some(err.to_string()),
                },
            });
        }
        Ok(rows)
    }

    /// What live workspaces other than `exclude` already claim of `asset`.
    ///
    /// Every live workspace draws on the same Kraken balance, so a claim is
    /// only meaningful net of the other claims against it.
    pub fn allocated_elsewhere(&self, exclude: &WorkspaceName, asset: &str) -> Result<Claims> {
        let mut claims = Claims::known(Decimal::ZERO);
        for (name, manifest) in store::list(&self.base)? {
            let Ok(manifest) = manifest else {
                // An unreadable neighbour's claim is unknown, so the total
                // becomes a floor. Skipping it silently would under-count and
                // let an over-allocation through — the unsafe direction.
                claims.complete = false;
                continue;
            };
            if manifest.mode != WorkspaceMode::Live
                || name.as_ref() == exclude.as_ref() as &str
                || !manifest.currency.eq_ignore_ascii_case(asset)
            {
                continue;
            }
            claims.total += manifest.capital;
        }
        Ok(claims)
    }

    pub fn show(&self, target: &Target) -> Result<WorkspaceOverview> {
        match target {
            Target::Default => Ok(WorkspaceOverview::synthesized_default()),
            Target::Named(name) => Ok(store::load(&self.base, name)?.into()),
        }
    }

    /// The named workspace's contract, verified. `default` has none.
    pub fn manifest(&self, name: &WorkspaceName) -> Result<WorkspaceManifest> {
        store::load(&self.base, name)
    }

    /// Fold the journal into per-asset balances. Holds the journal writer
    /// lock for the read, like every account operation.
    pub fn balances(&self, name: &WorkspaceName, origin: Origin) -> Result<WorkspaceBalances> {
        let manifest = store::load(&self.base, name)?;
        Ok(WorkspaceBalances {
            workspace: manifest.name,
            mode: manifest.mode,
            balances: journal_balances(self.journal_path(name), origin)?,
        })
    }

    /// Return a paper account to its starting capital: a `Reset(capital)`
    /// epoch on the journal. History survives — a reset is a new epoch, not
    /// an erasure.
    pub fn reset(
        &self,
        name: &WorkspaceName,
        origin: Origin,
        overrides: ResetOverrides,
    ) -> Result<WorkspaceManifest> {
        let mut manifest = store::load(&self.base, name)?;
        if manifest.mode != WorkspaceMode::Paper {
            return Err(WorkspaceError::CapitalManaged(format!(
                "workspace '{name}' is mode {}; only a paper account can be reset to its capital",
                manifest.mode
            )));
        }
        // A reset epoch inside an open session window would compete with the
        // window's own anchor and skew every score of that session.
        if let Some((run, _)) = crate::session::active(&crate::workspace_dir(&self.base, name))? {
            return Err(WorkspaceError::SessionActive {
                session: run.to_string(),
            });
        }
        // Re-parameterization updates the contract BEFORE the epoch is
        // written: validated first, persisted after the epoch lands, so a
        // failed journal append leaves both untouched.
        let reparameterized = !overrides.is_empty();
        if let Some(capital) = overrides.capital {
            kraken_paper::validate_money(capital, "capital")?;
            manifest.capital = capital;
        }
        if let Some(currency) = overrides.currency {
            manifest.currency = currency.to_uppercase();
        }
        if let Some(fee_rate) = overrides.fee_rate {
            ensure_rate(fee_rate, "fee rate")?;
            manifest.fee_rate = fee_rate;
        }
        if let Some(slippage_rate) = overrides.slippage_rate {
            ensure_rate(slippage_rate, "slippage rate")?;
            manifest.slippage_rate = slippage_rate;
        }
        let mut account = PaperAccount::open_at(self.journal_path(name), origin)?;
        let config = PaperConfig {
            balance: manifest.capital,
            currency: manifest.currency.clone(),
            fee_rate: manifest.fee_rate,
            slippage_rate: manifest.slippage_rate,
        };
        let entry = CommandEntry {
            name: "workspace reset".to_string(),
            balance: Some(manifest.capital),
            currency: Some(manifest.currency.clone()),
            fee_rate: Some(manifest.fee_rate),
            slippage_rate: Some(manifest.slippage_rate),
            outcome: CommandOutcome::Ok {
                order_ids: Vec::new(),
                trade_ids: Vec::new(),
            },
            ..CommandEntry::default()
        };
        account.commit(vec![AccountEvent::Reset(config)], entry)?;
        if reparameterized {
            kraken_recording::write_json_atomic(&store::manifest_path(&self.base, name), &manifest)
                .map_err(WorkspaceError::Recording)?;
        }
        Ok(manifest)
    }

    /// Promote a qualified paper workspace to a live allocation of the account.
    pub fn promote(
        &self,
        name: &WorkspaceName,
        mut evidence: crate::promote::PromotionInputs,
        origin: Origin,
    ) -> Result<WorkspaceManifest> {
        let manifest = store::load(&self.base, name)?;
        if manifest.mode == WorkspaceMode::Live {
            return Ok(manifest);
        }
        // The caller's blockers were measured before its confirmation prompt
        // and go stale while the user reads it. Re-measure them under the
        // journal's writer lock — a session or order that slipped in must
        // refuse the promotion BEFORE anything lands on the record.
        let mut account = PaperAccount::open_at(self.journal_path(name), origin)?;
        evidence.active_session =
            crate::session::active(&crate::workspace_dir(&self.base, name))?.is_some();
        evidence.open_paper_orders = account.state()?.open_orders.len();
        let checklist = crate::promote::evaluate(&manifest, evidence);
        if !checklist.promotable {
            return Err(WorkspaceError::NotPromotable {
                workspace: manifest.name,
                checklist: Box::new(checklist),
            });
        }

        // One journal now spans both eras. The divide marker (a bare reset
        // epoch is indistinguishable from a user-invoked reset) and the epoch
        // that starts live accounting from the allocation rather than
        // simulated P&L land as ONE durable batch: a half-landed promotion
        // would leave a still-paper journal claiming venue fills.
        let config = PaperConfig {
            balance: manifest.capital,
            currency: manifest.currency.clone(),
            fee_rate: manifest.fee_rate,
            slippage_rate: manifest.slippage_rate,
        };
        let entry = CommandEntry {
            name: "workspace promote".to_string(),
            balance: Some(manifest.capital),
            currency: Some(manifest.currency.clone()),
            fee_rate: Some(manifest.fee_rate),
            slippage_rate: Some(manifest.slippage_rate),
            outcome: CommandOutcome::Ok {
                order_ids: Vec::new(),
                trade_ids: Vec::new(),
            },
            ..CommandEntry::default()
        };
        account.commit(
            vec![
                AccountEvent::FillSourceChanged {
                    from: kraken_paper::FillSource::Simulated,
                    to: kraken_paper::FillSource::Venue,
                },
                AccountEvent::Reset(config),
            ],
            entry,
        )?;
        // Journal before manifest: a crash here leaves marker + epoch down
        // with the mode still paper, and the retry appends a fresh batch
        // (readers take the LAST marker). The reverse order would let real
        // fills land with no divide on the record at all.
        drop(account);
        self.activate_live(name)
    }

    /// Switch the existing workspace contract to live mode.
    fn activate_live(&self, name: &WorkspaceName) -> Result<WorkspaceManifest> {
        let mut manifest = store::load(&self.base, name)?;
        manifest.mode = WorkspaceMode::Live;
        kraken_recording::write_json_atomic(&store::manifest_path(&self.base, name), &manifest)
            .map_err(WorkspaceError::Recording)?;
        Ok(manifest)
    }

    /// The account journal: `<base>/workspaces/<name>/journal.jsonl`.
    pub fn journal_path(&self, name: &WorkspaceName) -> PathBuf {
        store::journal_path(&self.base, name)
    }
}

/// Fold any account journal into per-asset balances — one shaping shared by
/// workspace reads and (until runs replace sessions) session reads.
pub fn journal_balances(
    journal: PathBuf,
    origin: Origin,
) -> Result<BTreeMap<String, AssetBalance>> {
    let account = PaperAccount::open_at(journal, origin)?;
    Ok(asset_balances(account.state()?))
}

fn ensure_rate(rate: Decimal, what: &str) -> Result<()> {
    if rate < Decimal::ZERO || rate > Decimal::ONE {
        return Err(WorkspaceError::Spec(format!(
            "{what} must be between 0 and 1, got {rate}"
        )));
    }
    Ok(())
}

fn asset_balances(state: &PaperState) -> BTreeMap<String, AssetBalance> {
    let mut rows = BTreeMap::new();
    for (asset, &total) in &state.balances {
        let reserved = state.reserved.get(asset).copied().unwrap_or(Decimal::ZERO);
        rows.insert(
            asset.clone(),
            AssetBalance {
                total,
                reserved,
                available: total - reserved,
            },
        );
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn spec(name: &str) -> CreateSpec {
        CreateSpec {
            name: name.parse().expect("valid name"),
            capital: dec!(10000),
            currency: "USD".to_string(),
            mode: WorkspaceMode::Paper,
            fee_rate: dec!(0.0026),
            slippage_rate: dec!(0),
            allowed_pairs: None,
        }
    }

    #[test]
    fn create_funds_the_journal_with_the_capital_epoch() {
        let base = tempfile::tempdir().expect("tempdir");
        let workspaces = Workspaces::new(base.path());
        workspaces
            .create(spec("btc-momentum"), Origin::Cli)
            .expect("create");

        let name: WorkspaceName = "btc-momentum".parse().expect("valid name");
        let balances = workspaces.balances(&name, Origin::Cli).expect("balances");
        assert_eq!(balances.balances["USD"].total, dec!(10000));
        assert_eq!(balances.balances["USD"].available, dec!(10000));
    }

    #[test]
    fn create_refuses_the_default_name_as_capital_managed() {
        let base = tempfile::tempdir().expect("tempdir");
        let err = Workspaces::new(base.path())
            .create(spec("default"), Origin::Cli)
            .expect_err("must refuse");
        assert!(matches!(err, WorkspaceError::CapitalManaged(_)), "{err}");
        assert_eq!(err.category(), "validation");
    }

    #[test]
    fn create_accepts_a_live_contract_with_an_allocation() {
        let base = tempfile::tempdir().expect("tempdir");
        let mut live = spec("scalper");
        live.mode = WorkspaceMode::Live;
        let workspaces = Workspaces::new(base.path());
        let manifest = workspaces
            .create(live, Origin::Cli)
            .expect("live create should be local");
        assert_eq!(manifest.mode, WorkspaceMode::Live);
        let name: WorkspaceName = "scalper".parse().expect("valid name");
        let balances = workspaces.balances(&name, Origin::Cli).expect("balances");
        assert_eq!(balances.balances["USD"].total, dec!(10000));
    }

    #[test]
    fn activation_switches_the_existing_workspace_to_live() {
        let base = tempfile::tempdir().expect("tempdir");
        let workspaces = Workspaces::new(base.path());
        workspaces
            .create(spec("scalper"), Origin::Cli)
            .expect("paper create");
        let name: WorkspaceName = "scalper".parse().expect("valid name");
        let activated = workspaces.activate_live(&name).expect("activate");

        assert_eq!(activated.mode, WorkspaceMode::Live);
        let reloaded = workspaces.manifest(&name).expect("reload");
        assert_eq!(reloaded.mode, WorkspaceMode::Live);
    }

    #[test]
    fn promotion_preserves_history_and_starts_a_new_logical_epoch() {
        let base = tempfile::tempdir().expect("tempdir");
        let workspaces = Workspaces::new(base.path());
        workspaces
            .create(spec("scalper"), Origin::Cli)
            .expect("paper create");
        let name: WorkspaceName = "scalper".parse().expect("valid name");
        let promoted = workspaces
            .promote(
                &name,
                crate::promote::PromotionInputs {
                    experiments: vec![crate::promote::ExperimentEvidence {
                        experiment: "momentum".into(),
                        total_sessions: 2,
                        passing_sessions: 2,
                        passing_live_sessions: 1,
                    }],
                    ..crate::promote::PromotionInputs::default()
                },
                Origin::Cli,
            )
            .expect("promote");

        assert_eq!(promoted.mode, WorkspaceMode::Live);
        let journal =
            std::fs::read_to_string(workspaces.journal_path(&name)).expect("journal exists");
        assert!(journal.lines().next().unwrap().contains("\"initialized\""));
        assert!(journal.lines().any(|line| line.contains("\"reset\"")));
        assert!(
            journal
                .lines()
                .any(|line| line.contains("\"fill_source_changed\"")),
            "the era divide is on the record"
        );
    }

    /// A blocker that appears after the caller measured its evidence (the
    /// confirmation prompt is an open race window) must refuse the promotion
    /// with nothing on the record: a durable divide marker on a still-paper
    /// journal would mislabel later simulated fills as venue fills.
    #[test]
    fn promotion_re_measures_blockers_and_a_refusal_writes_nothing() {
        let base = tempfile::tempdir().expect("tempdir");
        let workspaces = Workspaces::new(base.path());
        workspaces
            .create(spec("scalper"), Origin::Cli)
            .expect("paper create");
        let name: WorkspaceName = "scalper".parse().expect("valid name");

        // A resting order slips in between evidence gathering and promote.
        let mut account =
            PaperAccount::open_at(workspaces.journal_path(&name), Origin::Cli).expect("open");
        let order = account
            .state()
            .expect("state")
            .decide_limit_order(
                kraken_paper::OrderSide::Buy,
                "BTC/USD",
                dec!(0.1),
                dec!(100),
            )
            .expect("decide");
        account
            .commit(
                vec![AccountEvent::OrderSubmitted { order }],
                CommandEntry::default(),
            )
            .expect("submit");
        drop(account);

        let stale_evidence = crate::promote::PromotionInputs {
            experiments: vec![crate::promote::ExperimentEvidence {
                experiment: "momentum".into(),
                total_sessions: 2,
                passing_sessions: 2,
                passing_live_sessions: 1,
            }],
            open_paper_orders: 0, // measured before the order landed
            ..crate::promote::PromotionInputs::default()
        };
        let err = workspaces
            .promote(&name, stale_evidence, Origin::Cli)
            .expect_err("must refuse");
        assert!(matches!(err, WorkspaceError::NotPromotable { .. }), "{err}");

        let journal =
            std::fs::read_to_string(workspaces.journal_path(&name)).expect("journal exists");
        assert!(
            !journal.contains("\"fill_source_changed\""),
            "a refused promotion leaves no divide marker"
        );
        assert_eq!(
            workspaces.manifest(&name).expect("manifest").mode,
            WorkspaceMode::Paper
        );
    }

    #[test]
    fn reset_returns_balances_to_capital_and_history_survives_in_the_journal() {
        let base = tempfile::tempdir().expect("tempdir");
        let workspaces = Workspaces::new(base.path());
        workspaces.create(spec("w1"), Origin::Cli).expect("create");
        let name: WorkspaceName = "w1".parse().expect("valid name");

        workspaces
            .reset(&name, Origin::Cli, ResetOverrides::default())
            .expect("reset");
        let balances = workspaces.balances(&name, Origin::Cli).expect("balances");
        assert_eq!(balances.balances["USD"].total, dec!(10000));

        let journal =
            std::fs::read_to_string(workspaces.journal_path(&name)).expect("journal exists");
        let lines: Vec<&str> = journal.lines().collect();
        assert!(lines[0].contains("\"initialized\""), "epoch survives");
        assert!(
            lines.iter().any(|line| line.contains("\"reset\"")),
            "reset epoch appended"
        );
    }

    #[test]
    fn list_synthesizes_the_default_row_first() {
        let base = tempfile::tempdir().expect("tempdir");
        let workspaces = Workspaces::new(base.path());
        workspaces
            .create(spec("alpha"), Origin::Cli)
            .expect("create");

        let rows = workspaces.list().expect("list");
        assert_eq!(rows[0].name, "default");
        assert_eq!(rows[0].mode, WorkspaceMode::Live);
        assert!(rows[0].capital.is_none(), "venue-held, never local");
        assert_eq!(rows[1].name, "alpha");
        assert_eq!(rows[1].capital, Some(dec!(10000)));
    }

    #[test]
    fn show_default_is_the_synthesized_live_account() {
        let base = tempfile::tempdir().expect("tempdir");
        let overview = Workspaces::new(base.path())
            .show(&Target::Default)
            .expect("show");
        assert_eq!(overview.name, "default");
        assert_eq!(overview.mode, WorkspaceMode::Live);
        assert!(overview.capital.is_none());
    }
}
