//! Recipe commands backed by the [`Recipes`] application facade.

use super::Execute;
use crate::{
    cli::AppContext,
    config,
    errors::{KrakenError, Result},
    output::CommandOutput,
};
use kraken_contract::recipe::{Create, List, RecipeCommand, Run, Show};
use kraken_recipe::{CreateSpec, Recipe, RecipeError, Recipes, StoredRecipe};
use kraken_session::timeline::event::EventPayload;

/// The library over the global config dir. Recipes are portable across
/// accounts, so they are never workspace-scoped — only the source session
/// read is.
fn recipes() -> Result<Recipes> {
    Ok(Recipes::new(config::config_dir()?))
}

/// The built-in demo recipe: `recipe list` is never empty on a fresh
/// install, and `recipe run playground` is the one-command demo's twin.
/// An on-disk recipe of the same name shadows it. Provenance is openly
/// synthetic (epoch timestamps, no source session) — this artifact was
/// shipped, not derived.
const BUILTIN_NAME: &str = "playground";

fn builtin_recipe() -> Recipe {
    Recipe {
        recipe_version: kraken_recipe::RECIPE_VERSION.to_string(),
        cli_version: env!("CARGO_PKG_VERSION").to_string(),
        name: BUILTIN_NAME.to_string(),
        description: Some(
            "Built-in: the playground demo as a recipe — a recorded BTC/USD paper session driven \
             by the DCA example skill."
                .to_string(),
        ),
        driver: kraken_recipe::Driver::Skill {
            name: "recipe-playground-dca".to_string(),
        },
        params: None,
        market: kraken_recipe::MarketSpec {
            symbols: vec!["BTC/USD".to_string()],
            channels: vec!["ticker".to_string(), "trade".to_string()],
            sinks: vec!["jsonl".to_string()],
        },
        account: kraken_recipe::AccountSpec {
            capital: rust_decimal::Decimal::from(10_000),
            currency: "USD".to_string(),
        },
        origin: kraken_recipe::Origin {
            session: "built-in".to_string(),
            label: None,
            experiment: None,
            started_at: chrono::DateTime::UNIX_EPOCH,
            ended_at: Some(chrono::DateTime::UNIX_EPOCH),
            tape: None,
        },
        created_at: chrono::DateTime::UNIX_EPOCH,
    }
}

/// Resolve a recipe for `run`, with the built-in fallback: a stored
/// `playground` shadows the shipped one. The path is the draft skill's
/// location, `None` for the built-in (nothing on disk).
fn runnable_recipe(name: &str) -> Result<(Recipe, Option<std::path::PathBuf>)> {
    match recipes()?.show(name) {
        Ok(StoredRecipe {
            recipe, skill_path, ..
        }) => Ok((recipe, Some(skill_path))),
        Err(RecipeError::NotFound(_)) if name == BUILTIN_NAME => Ok((builtin_recipe(), None)),
        Err(err) => Err(err.into()),
    }
}

/// A recipe's `capital` claims to be a paper figure, but a master- or
/// live-scope session freezes the *real* account's equity into
/// `starting_balance` — packaging that into a portable artifact would leak a
/// real balance as if it were sandbox capital.
fn ensure_paper_scope(ctx: &AppContext) -> Result<()> {
    match super::workspace_guard::active_mode(ctx)? {
        super::workspace_guard::ActiveMode::Paper(_) => Ok(()),
        super::workspace_guard::ActiveMode::Master
        | super::workspace_guard::ActiveMode::Live(_) => {
            Err(crate::errors::KrakenError::Validation(
                "recipe create packages a paper session's setup; select a paper workspace \
                 (--workspace or KRAKEN_WORKSPACE) — a real-account session would bake real \
                 equity into a portable artifact"
                    .to_string(),
            ))
        }
    }
}

impl Execute for Create {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        // `recipe run` provisions a paper workspace named after the recipe, so
        // the reserved real-account scope can never back a run — refuse the
        // name here rather than leave a dead recipe in the library.
        if self.name == kraken_workspace::DEFAULT_WORKSPACE {
            return Err(KrakenError::Validation(
                "recipe name 'default' is reserved: 'recipe run' provisions a paper workspace \
                 named after the recipe, and 'default' is the real account scope — pick another \
                 name"
                    .into(),
            ));
        }
        ensure_paper_scope(ctx)?;
        super::session::ensure_session_stopped(ctx, &self.from_session)?;
        let (_, timeline) = super::session::read_session_window(ctx, &self.from_session)?;
        let decisions: Vec<_> = timeline
            .events
            .into_iter()
            .filter_map(|event| match event.payload {
                EventPayload::Decision(decision) => Some(decision),
                _ => None,
            })
            .collect();
        let spec = CreateSpec {
            name: self.name,
            description: self.description,
            driver: self.driver,
            overwrite: self.overwrite,
        };
        let stored = recipes()?.create(&timeline.manifest, &decisions, &spec)?;
        Ok(recipe_output(stored, "create"))
    }
}

impl Execute for List {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        let rows = recipes()?.list()?;
        let mut table: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                vec![
                    row.name.clone(),
                    row.driver.clone().unwrap_or_else(|| "—".to_string()),
                    row.symbols
                        .as_ref()
                        .map_or_else(|| "—".to_string(), |symbols| symbols.join(",")),
                    row.created_at
                        .map_or_else(|| "—".to_string(), |at| at.to_rfc3339()),
                    row.damaged.clone().unwrap_or_default(),
                ]
            })
            .collect();
        let mut json_rows = serde_json::to_value(&rows)?;
        // The shipped demo recipe, unless an on-disk one shadows it — the
        // library is never empty on a fresh install.
        if !rows.iter().any(|row| row.name == BUILTIN_NAME) {
            let recipe = builtin_recipe();
            table.push(vec![
                recipe.name.clone(),
                recipe.driver.skill().to_string(),
                recipe.market.symbols.join(","),
                "—".to_string(),
                "built-in".to_string(),
            ]);
            if let Some(entries) = json_rows.as_array_mut() {
                entries.push(serde_json::json!({
                    "name": recipe.name,
                    "driver": recipe.driver.skill(),
                    "symbols": recipe.market.symbols,
                    "builtin": true,
                }));
            }
        }
        let data = serde_json::json!({
            "group": "recipe",
            "type": "list",
            "recipes": json_rows,
        });
        let headers = ["Recipe", "Driver", "Symbols", "Created", "Note"]
            .map(String::from)
            .to_vec();
        Ok(CommandOutput::new(data, headers, table))
    }
}

impl Execute for Show {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        match recipes()?.show(&self.name) {
            Ok(stored) => Ok(recipe_output(stored, "show")),
            Err(RecipeError::NotFound(_)) if self.name == BUILTIN_NAME => {
                Ok(builtin_output("show"))
            }
            Err(err) => Err(err.into()),
        }
    }
}

/// One payload shape for `create` and `show`: the whole contract in JSON,
/// the load-bearing fields plus artifact paths as the table.
fn recipe_output(stored: StoredRecipe, kind: &str) -> CommandOutput {
    let StoredRecipe {
        recipe,
        recipe_path,
        skill_path,
    } = stored;
    let pairs = vec![
        ("Recipe".to_string(), recipe.name.clone()),
        ("Driver".to_string(), recipe.driver.skill().to_string()),
        (
            "Params".to_string(),
            recipe
                .params
                .as_ref()
                .map_or_else(|| "—".to_string(), ToString::to_string),
        ),
        (
            "Market".to_string(),
            format!(
                "{} ({})",
                recipe.market.symbols.join(","),
                recipe.market.channels.join(",")
            ),
        ),
        (
            "Account".to_string(),
            format!("{} {}", recipe.account.capital, recipe.account.currency),
        ),
        ("Source session".to_string(), recipe.origin.session.clone()),
        ("Skill".to_string(), skill_path.display().to_string()),
    ];
    let data = serde_json::json!({
        "group": "recipe",
        "type": kind,
        "recipe": recipe,
        "recipe_path": recipe_path.display().to_string(),
        "skill_path": skill_path.display().to_string(),
    });
    CommandOutput::key_value(pairs, data)
}

/// The built-in recipe's payload: same shape as [`recipe_output`], with a
/// `builtin` marker in place of artifact paths.
fn builtin_output(kind: &str) -> CommandOutput {
    let recipe = builtin_recipe();
    let pairs = vec![
        ("Recipe".to_string(), recipe.name.clone()),
        ("Driver".to_string(), recipe.driver.skill().to_string()),
        ("Params".to_string(), "—".to_string()),
        (
            "Market".to_string(),
            format!(
                "{} ({})",
                recipe.market.symbols.join(","),
                recipe.market.channels.join(",")
            ),
        ),
        (
            "Account".to_string(),
            format!("{} {}", recipe.account.capital, recipe.account.currency),
        ),
        (
            "Source".to_string(),
            "built-in (nothing on disk)".to_string(),
        ),
    ];
    let data = serde_json::json!({
        "group": "recipe",
        "type": kind,
        "recipe": recipe,
        "builtin": true,
    });
    CommandOutput::key_value(pairs, data)
}

impl Execute for Run {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        run(&self, ctx).await?;
        Ok(CommandOutput::inline())
    }

    /// A session start composed from an artifact: it streams like one.
    fn renders_inline(&self) -> bool {
        true
    }
}

/// Run a recipe: the inverse of `recipe create` — write exactly the flags
/// create read, then delegate to the ordinary session runner. The run
/// produces a normal session, so show/explain/score/compare/report all work
/// on it unchanged.
pub(crate) async fn run(args: &Run, ctx: &AppContext) -> Result<()> {
    let (recipe, skill_path) = runnable_recipe(&args.name)?;

    // Parse the artifact's strings at the CLI boundary, exactly where clap
    // would have: a recipe naming a channel or sink no build knows is a
    // validation refusal, not a stream error. (A known-but-feature-gated
    // sink is checked separately below.)
    let channels: Vec<kraken_core::SubscribableChannel> = recipe
        .market
        .channels
        .iter()
        .map(|channel| {
            channel.parse().map_err(|_| {
                KrakenError::Validation(format!(
                    "recipe '{}' names market channel '{channel}', which this build does not know \
                     — edit recipe.json or re-create the recipe",
                    recipe.name
                ))
            })
        })
        .collect::<Result<_>>()?;
    let mut to: Vec<crate::record::SinkTarget> = recipe
        .market
        .sinks
        .iter()
        .map(|sink| {
            clap::ValueEnum::from_str(sink, true).map_err(|_| {
                KrakenError::Validation(format!(
                    "recipe '{}' names sink '{sink}', which this build does not know — edit \
                     recipe.json or re-create the recipe",
                    recipe.name
                ))
            })
        })
        .collect::<Result<_>>()?;
    // `duckdb` parses on every build; whether this build can WRITE it is the
    // recording crate's one feature gate. Refuse before any state exists —
    // provisioning a workspace and allocating a session for a run that can
    // only fail at the sink would leave a dangling aborted session.
    if to.contains(&crate::record::SinkTarget::Duckdb)
        && !kraken_recording::TapeBackend::supported()
            .iter()
            .any(|backend| matches!(backend, kraken_recording::TapeBackend::Duckdb))
    {
        return Err(KrakenError::Validation(format!(
            "recipe '{}' records to 'duckdb', which this build cannot write — edit the recipe's \
             sinks to 'jsonl' or use a duckdb-enabled build",
            recipe.name
        )));
    }
    // The artifact records only durable backends; echo the live tape to
    // stdout like the demo, so the run is visibly alive.
    to.push(crate::record::SinkTarget::Stdout);

    let symbols = args
        .symbols
        .clone()
        .unwrap_or_else(|| recipe.market.symbols.clone());
    // The trade example must name a pair the session actually records — a
    // fill on an unrecorded pair leaves the window without marks to score
    // it by. On a replay the tape decides, so no example is printed there.
    let example_symbol = symbols.first().cloned();
    // The artifact stores whatever JSON the source session recorded; a session
    // start takes the object the flag accepts. A recipe carrying anything else
    // is refused by name rather than run with its parameters dropped.
    let params = match &recipe.params {
        Some(serde_json::Value::Object(params)) => Some(params.clone()),
        None => None,
        Some(other) => {
            return Err(KrakenError::Validation(format!(
                "recipe '{}' records strategy params as {other}, which is not an object of \
                 parameters — edit recipe.json",
                recipe.name
            )));
        }
    };
    let start = kraken_contract::session::Start::from_recipe(
        symbols,
        channels,
        to,
        recipe.driver.skill().to_string(),
        params,
        args.experiment.clone(),
        args.from.clone(),
        args.speed,
        args.window.clone(),
    );

    let scoped = if args.real_account {
        // Real money: banner first, then the evidence caveats no recipe
        // design fixes, then the deliberate opt-in. `--from` is a clap
        // conflict, and the session guard refuses it again downstream.
        eprintln!("{}", super::order::LIVE_BANNER);
        eprintln!("real-account evidence caveats:");
        eprintln!("  - the session window anchors on the WHOLE portfolio, not this recipe's slice");
        eprintln!(
            "  - 'session stop' reconciles every venue fill in the window, not only this recipe's"
        );
        eprintln!("  - friction reads as zero: reconciled fills carry no reference quote");
        eprintln!(
            "  - fills are invisible until stop — a driver polling 'session show' mid-run sees \
             none"
        );
        ctx.confirm_destructive(&format!(
            "Run recipe '{}' against the REAL Kraken account?",
            recipe.name
        ))?;
        ctx.rescope(None)
    } else {
        // The artifact fully determines the run environment: a workspace
        // named after the recipe, seeded once from its account spec. The
        // ambient workspace is deliberately ignored, like the demo's.
        let workspace: kraken_workspace::WorkspaceName = recipe.name.parse()?;
        super::workspace::provision_paper_workspace(
            ctx,
            &workspace,
            recipe.account.capital,
            &recipe.account.currency,
        )?;
        ctx.rescope(Some(recipe.name.clone()))
    };

    // The driver handoff goes to stderr — stdout stays the session's own
    // machine-parseable stream, exactly like the demo.
    let skill = skill_path.map_or_else(
        || "built-in (load the driver skill by name)".to_string(),
        |path| path.display().to_string(),
    );
    let params = recipe
        .params
        .as_ref()
        .map_or_else(|| "skill defaults".to_string(), ToString::to_string);
    eprintln!(
        "recipe:     {} (driver {})",
        recipe.name,
        recipe.driver.skill()
    );
    eprintln!("params:     {params}");
    eprintln!("skill:      {skill}");
    if args.real_account {
        eprintln!("watch it:   kraken session show       # window, status, valuation");
    } else {
        eprintln!(
            "drive it:   load the '{}' skill and /loop it",
            recipe.driver.skill()
        );
        eprintln!("watch it:   export KRAKEN_WORKSPACE={}", recipe.name);
        eprintln!("            kraken session show       # window, status, valuation");
        eprintln!("            kraken workspace status   # the account itself");
        if let Some(symbol) = &example_symbol
            && args.from.is_none()
        {
            eprintln!("trade it:   kraken order buy {symbol} 0.001 --type market --reason \"...\"");
        }
    }
    eprintln!("review it:  kraken explain pnl && kraken lab score");
    eprintln!("stop early: kraken session stop");

    super::session::start(&start, &scoped).await
}

delegate_execute!(RecipeCommand {
    Create,
    List,
    Show,
    Run
});
