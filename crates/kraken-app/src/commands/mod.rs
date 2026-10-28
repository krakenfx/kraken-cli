//! The CLI command tree: [`Command`] composes the per-domain subcommand enums
//! declared here. [`Execute`] is the work contract every leaf fulfils, and
//! [`Admission`] is the only way to reach it — the gates live there, so no
//! command can execute unjudged. The entry points live in [`executor`].

use crate::{cli::AppContext, errors::Result, output::CommandOutput};

/// Uniform execution: each command resolves the resources it needs from the
/// context (clients, credentials, OTP, force) and runs, returning structured
/// output. Every enum-level impl comes from `delegate_execute!`, so the only
/// hand-written code per command is its own work.
///
/// Consumes the command: leaves move their parsed args into request payloads,
/// which a `&self` receiver would force to clone.
///
/// [`execute`](Self::execute) is the leaf hook, deliberately not the entry
/// point: execution starts at [`Admission::admit`], which applies the gates
/// first. A leaf calls another leaf's `execute` directly when it reuses that
/// command's work (the `paper` aliases, `workspace report`), so one user
/// invocation yields exactly one gate decision.
///
/// Crate-internal + static-dispatch only, so native `async fn` in a trait
/// needs no `async-trait` crate.
pub(crate) trait Execute {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput>;

    /// True for the commands that stream and render their own output, so their
    /// [`CommandOutput`] is an empty receipt ([`CommandOutput::inline`]).
    /// Asked *before* execution by the render-free MCP path, which has nowhere
    /// to put a stream — hence a property of the command, not of its outcome.
    fn renders_inline(&self) -> bool {
        false
    }
}

/// Delegate [`Execute`] from a command enum to the leaf impls of its variants.
///
/// Written rather than derived because the tree lives in [`kraken_contract`]:
/// a derive emits the enum-level impl at the enum's *definition* site, and that
/// impl names [`AppContext`] and [`CommandOutput`], which the contract crate
/// must never depend on.
///
/// Naming the variants here is not a second source of truth: one added in the
/// contract and omitted here fails to compile as a non-exhaustive match.
macro_rules! delegate_execute {
    ($enum:ty { $($variant:ident),+ $(,)? }) => {
        impl $crate::commands::Execute for $enum {
            async fn execute(
                self,
                ctx: &$crate::cli::AppContext,
            ) -> $crate::errors::Result<$crate::output::CommandOutput> {
                match self {
                    $(Self::$variant(cmd) => cmd.execute(ctx).await,)+
                }
            }

            fn renders_inline(&self) -> bool {
                match self {
                    $(Self::$variant(cmd) => cmd.renders_inline(),)+
                }
            }
        }
    };
}

/// The gates every command passes through, and the only entry into execution.
///
/// Implemented for [`Command`] alone, deliberately: a gate judges a whole
/// invocation, so it needs the root's typed tree (`{"order":{"buy":…}}`), not
/// the leaf fragment a defaulted [`Execute`] method would see. Leaves expose
/// only [`Execute::execute`], so "gated" is not a rule callers must remember:
/// it is the only shape the type system offers them.
pub(crate) trait Admission: Sized {
    /// Judge without running. Only the REPL calls this directly — it hosts the
    /// command tree instead of living in it (see [`executor::dispatch`]), so it
    /// needs the verdict without the execution.
    async fn gate(&self, ctx: &AppContext) -> Result<()>;

    async fn admit(self, ctx: &AppContext) -> Result<CommandOutput>;
}

impl Admission for Command {
    async fn gate(&self, ctx: &AppContext) -> Result<()> {
        workspace_guard::enforce(ctx, self)
    }

    async fn admit(self, ctx: &AppContext) -> Result<CommandOutput> {
        self.gate(ctx).await?;
        match self {
            Command::Market(kraken_contract::market::MarketCommand::Ticker(ticker))
                if ticker.watch.is_some() =>
            {
                market::watch(ticker, ctx).await?;
                Ok(CommandOutput::inline())
            }
            cmd => cmd.execute(ctx).await,
        }
    }
}

pub(crate) mod account;
pub(crate) mod auth;
pub(crate) mod domain_bridge;
pub(crate) mod earn;
mod executor;
pub(crate) mod explain;
pub(crate) mod feedback;
pub(crate) mod funding;
pub(crate) mod futures;
pub(crate) mod futures_paper;
pub(crate) mod futures_ws;
pub(crate) mod journal_mirror;
pub(crate) mod lab;
pub(crate) mod market;
pub(crate) mod order;
pub(crate) mod paper;
pub(crate) mod playground;
pub(crate) mod plugin;
pub(crate) mod recipe;
pub(crate) mod record;
pub(crate) mod replay;
pub(crate) mod replay_venue;
mod rest_bridge;
pub(crate) mod session;
pub(crate) mod setup;
pub(crate) mod streamd;
mod streaming;
pub(crate) mod subaccount;
pub(crate) mod subscription;
pub(crate) mod tape;
pub(crate) mod workspace;
pub(crate) mod workspace_guard;
pub(crate) mod ws;

#[cfg(test)]
mod rest_contract;
#[cfg(test)]
pub(crate) mod test_support;

pub use self::{
    executor::{dispatch, execute_command, run},
    workspace_guard::active_manifest,
};
pub(crate) use kraken_contract::command::Command;

#[cfg(test)]
mod tests {
    use super::{Command, Execute};
    use crate::cli::Cli;
    use clap::Parser;
    use kraken_contract::tool::{CommandPath, Entry, Reach, Surface};

    /// The gates live in [`Admission`], so the only way to reach a command's
    /// work is through it. This pins the one thing the type system cannot: that
    /// the entry points ask for admission rather than calling the leaf hook
    /// directly. `dispatch` may call `gate` (it hosts the REPL); neither entry
    /// may call `run`.
    #[test]
    fn the_entry_points_execute_only_through_admission() {
        let source = include_str!("executor.rs");
        assert!(
            !source.contains(".execute(ctx)"),
            "executor.rs must reach commands through Admission, never Execute::execute directly"
        );
        assert_eq!(
            source.matches(".admit(ctx)").count(),
            2,
            "both entry points (dispatch, execute_command) must admit"
        );
    }

    /// A command family lives in exactly two halves that share a module name:
    /// its shapes in `kraken-contract`, its work here. A family is either fully
    /// moved or not started — never half, because a family with declarations on
    /// both sides is two sources of truth for one command.
    ///
    /// Walks the contract crate rather than a written list, so the invariant
    /// covers each family the moment it moves.
    #[test]
    fn a_command_family_declares_its_shapes_on_exactly_one_side() {
        let engine = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let commands = engine.join("src/commands");
        let contract = engine.join("../kraken-contract/src");

        for entry in std::fs::read_dir(&contract).expect("the contract crate's src is readable") {
            let path = entry.expect("a readable dir entry").path();
            let Some(family) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // Not families. `value_types`/`domain_values` are values shared
            // across families rather than owned by one, `error` is how building
            // a request fails, `command` is the root — whose executing half is
            // this module — and `tool` says how commands are published to an
            // agent, which nothing here executes.
            if matches!(
                family,
                "lib" | "value_types" | "domain_values" | "error" | "command" | "tool"
            ) {
                continue;
            }

            // A family is one file or a directory of them (`ws`).
            let work = [
                commands.join(format!("{family}.rs")),
                commands.join(family).join("mod.rs"),
            ]
            .into_iter()
            .find(|p| p.exists())
            .unwrap_or_else(|| {
                panic!("kraken_contract::{family} has no src/commands/{family} to execute it")
            });
            let work = std::fs::read_to_string(&work).expect("the executing half is readable");
            // The derive, not a mention of it: an import or a doc reference is
            // not a declaration.
            let declares_shapes = work.lines().any(|line| {
                line.trim_start().starts_with("#[derive(")
                    && (line.contains("Args") || line.contains("Subcommand"))
            });
            assert!(
                !declares_shapes,
                "src/commands/{family} still declares argument structs; a moved family keeps its \
                 shapes in kraken-contract alone"
            );
        }
    }

    /// The paper surface must stay auth-free: it trades no real funds, so a
    /// credentials reference appearing in it is a wiring bug. Scanned from a
    /// sibling module so the assertion needles never match themselves.
    #[test]
    fn test_paper_commands_no_auth_imports() {
        let source = include_str!("paper.rs");
        assert!(
            !source.contains("Credentials"),
            "paper.rs must not reference Credentials"
        );
        assert!(
            !source.contains("private_post"),
            "paper.rs must not reference private_post"
        );
        assert!(
            !source.contains("build_spot_authed"),
            "paper.rs must not reference build_spot_authed"
        );
        assert!(
            !source.contains("kraken_auth"),
            "paper.rs must not reference kraken_auth"
        );
        assert!(
            !source.contains("credential_store"),
            "paper.rs must not reference credential_store"
        );
    }
    /// The published examples, parsed through the real CLI parser: one
    /// `(classification, command)` pair per command the contract declares.
    ///
    /// This is the bridge the two halves of the tree otherwise lack. A leaf's
    /// [`Reach`] is a type-level const in `kraken-contract`;
    /// [`Execute::renders_inline`] is a method on a *value* here — so nothing
    /// could compare them without one command of every kind to hand.
    /// `EXAMPLE` is the only place the contract holds a whole invocation, and
    /// `every_command_offers_an_example` is what makes this total.
    fn examples() -> Vec<(&'static std::sync::Arc<Entry>, Command)> {
        let mut parsed = Vec::new();
        let mut rejected = Vec::new();
        for entry in Surface::shared().iter() {
            let Some(example) = entry.example else {
                continue;
            };
            match Cli::try_parse_from(words(example)) {
                Ok(Cli {
                    command: Some(command),
                    ..
                }) => parsed.push((entry, command)),
                Ok(_) => rejected.push(format!("{example} — parsed to no subcommand")),
                Err(e) => rejected.push(format!("{example} — {}", e.kind())),
            }
        }
        assert!(
            rejected.is_empty(),
            "a published example the CLI does not parse: {rejected:#?}"
        );
        parsed
    }

    /// Splits a published example into argv the way a pasted line splits.
    ///
    /// Test-local on purpose: turning input into a [`Cli`] is a front-end's own
    /// job, so the REPL owns its copy (`kraken-shell`'s `shell_words`) and this
    /// crate must not grow a dependency on a front-end to reach it. Correctness
    /// in general is not required and not claimed — only correctness on the
    /// examples, which is self-policing: one this mis-splits fails to parse and
    /// [`examples`] names it.
    fn words(line: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut current = String::new();
        let mut single = false;
        let mut double = false;
        let mut escaped = false;
        for ch in line.chars() {
            match ch {
                _ if escaped => {
                    current.push(ch);
                    escaped = false;
                }
                '\\' if !single => escaped = true,
                '\'' if !double => single = !single,
                '"' if !single => double = !double,
                ' ' | '\t' if !single && !double => {
                    if !current.is_empty() {
                        words.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(ch),
            }
        }
        if !current.is_empty() {
            words.push(current);
        }
        words
    }

    /// Every example parses as the command it is filed under.
    ///
    /// This is what lets the two tests below trust their pairing: an example
    /// that drifted onto another leaf would compare one command's
    /// classification against another command's behaviour, and agree. Asked
    /// of the parsed command's own tag chain rather than of the text — a
    /// leading substring says only that the invocation starts the same way.
    #[test]
    fn every_published_example_parses_as_the_command_it_documents() {
        let wrong: Vec<&str> = examples()
            .iter()
            .filter(|(entry, command)| !is_command_at(command, entry.path()))
            .map(|(entry, _)| entry.command.as_str())
            .collect();
        assert!(
            wrong.is_empty(),
            "an example that parses as some other command than its own: {wrong:#?}"
        );
    }

    /// Whether `command`'s wire form sits under `path`'s tag chain — the shape
    /// [`Command`] deserializes from, read back out.
    fn is_command_at(command: &Command, path: CommandPath) -> bool {
        let wire = serde_json::to_value(command).expect("the command tree serializes");
        let mut level = &wire;
        for tag in path.tags() {
            let Some(inner) = level.get(tag) else {
                return false;
            };
            level = inner;
        }
        true
    }

    /// A command the contract publishes as [`Reach::Stream`] must be one this
    /// engine answers for as inline. The contract's word is what an agent reads
    /// ("prints its own output as it runs"), so a leaf that no longer does has
    /// to lose the classification, not keep it.
    #[test]
    fn a_command_classified_as_streaming_renders_its_own_output() {
        let wrong: Vec<&str> = examples()
            .iter()
            .filter(|(entry, command)| entry.reach == Reach::Stream && !command.renders_inline())
            .map(|(entry, _)| entry.command.as_str())
            .collect();
        assert!(
            wrong.is_empty(),
            "classified `Stream` but not inline — the contract says it streams and the engine \
             does not: {wrong:#?}"
        );
    }

    /// The mirror, and the direction that ships a broken tool: a command that
    /// writes its own stdout has no structured payload a tool call could carry,
    /// so `execute_command` refuses it. Advertising it anyway registers a tool
    /// that fails every call — silently, because discovery has no way to
    /// notice.
    #[test]
    fn an_inline_command_is_never_advertised_as_a_tool() {
        let wrong: Vec<&str> = examples()
            .iter()
            .filter(|(entry, command)| command.renders_inline() && entry.reach.is_tool())
            .map(|(entry, _)| entry.command.as_str())
            .collect();
        assert!(
            wrong.is_empty(),
            "inline but classified callable — every tool call would be refused by \
             `execute_command`: {wrong:#?}"
        );
    }
}

delegate_execute!(Command {
    Market,
    Account,
    Auth,
    Tape,
    Deposit,
    Earn,
    Explain,
    Feedback,
    Futures,
    Lab,
    Order,
    Paper,
    Plugin,
    Recipe,
    Subaccount,
    WalletTransfer,
    Withdraw,
    Withdrawal,
    Workspace,
    Streaming,
    Session,
    Playground
});
