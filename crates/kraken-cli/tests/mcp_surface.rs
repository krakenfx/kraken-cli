//! Pins for this binary's MCP surface: what the real tool surface produces, and
//! the danger-policy rules that a snapshot cannot enforce.
//!
//! Two layers, deliberately different in kind:
//! - **Snapshots** freeze the produced surface — every tool's group, gate, and
//!   acknowledgment shape. A diff is a deliberate contract change, reviewed via
//!   `cargo insta review`; a `DANGEROUS` marker disappearing is a money-safety
//!   change and must be reviewed as one.
//! - **Rules** guard what a snapshot cannot: forcing functions over commands
//!   that do not exist yet, and the cross-check against the workspace guard's
//!   own refusal set.
//!
//! What is *no longer* here: the four tests that held a hand-written catalog in
//! step with the clap tree. There is one surface now, derived from the command
//! tree's types, and `kraken_contract::tool`'s own tests pin it against clap.

#![allow(clippy::unwrap_used, clippy::panic)]

use kraken_contract::tool::{Effect, Entry, Service, Surface};
use kraken_mcp::{GateMode, ToolEntry, ToolRegistry};
use serde_json::Value;
use std::collections::BTreeSet;

/// A path relative to the repository root.
///
/// These tests assert on repository documents — the published catalog and the
/// agent-facing guides — not on crate assets, so they read the files that
/// actually ship rather than a copy vendored per crate.
macro_rules! repo_file {
    ($path:literal) => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../", $path)
    };
}

fn catalog() -> Value {
    serde_json::from_str(include_str!(repo_file!("agents/tool-catalog.json"))).unwrap()
}

/// The full service set a `--services all` server exposes, resolved the same
/// way the server resolves it.
fn all_services() -> Vec<Service> {
    kraken_mcp::parse_services("all").unwrap()
}

fn full_registry(mode: GateMode) -> ToolRegistry {
    ToolRegistry::build(Surface::shared(), &all_services(), false, mode).unwrap()
}

/// One snapshot line per tool: name, service group, and its gate posture.
/// Asserts on the way that discovery and the gate tell one story — the danger
/// prefix, the destructive annotation, and the required acknowledgment appear
/// together exactly on armed tools.
fn surface_line(tool: &ToolEntry, mode: GateMode) -> String {
    let armed = tool.armed_under(mode);
    let schema = tool.input_schema();
    let ack_required = schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|names| names.iter().any(|name| name == "acknowledged"));
    let ack_optional = !ack_required
        && schema
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| properties.contains_key("acknowledged"));

    assert_eq!(
        armed,
        tool.description().starts_with("[DANGEROUS"),
        "{}: danger prefix out of step with the gate",
        tool.name()
    );
    assert_eq!(
        armed,
        tool.destructive_hint(),
        "{}: destructive annotation out of step with the gate",
        tool.name()
    );
    assert_eq!(
        armed,
        ack_required,
        "{}: required acknowledgment out of step with the gate",
        tool.name()
    );

    let marker = match (armed, ack_optional) {
        (true, _) => "DANGEROUS ack-required",
        (false, true) => "ack-optional",
        (false, false) => "-",
    };
    format!("{:<40} {:<12} {marker}", tool.name(), tool.group())
}

// --- Surface snapshots ------------------------------------------------------

/// The complete guarded tool surface against the real account: every tool a
/// `--services all` server advertises, with its group and gate posture.
#[test]
fn guarded_tool_surface_is_pinned() {
    let registry = full_registry(GateMode::Master);
    for tool in registry.tools() {
        assert!(
            tool.group() != Service::Websocket,
            "{}: streaming groups are excluded from MCP v1",
            tool.name()
        );
    }
    let mut lines: Vec<String> = registry
        .tools()
        .map(|t| surface_line(t, GateMode::Master))
        .collect();
    lines.sort();
    insta::assert_snapshot!(lines.join("\n"));
}

/// Exactly the mode-routed trading verbs relax inside a paper workspace — the
/// contract's `PaperSafe` set as the gate actually applies it. Anything
/// appearing here executes UNPROMPTED in a paper workspace; anything leaving
/// here re-arms against the real account.
#[test]
fn paper_gate_relaxes_exactly_the_mode_routed_verbs() {
    let registry = full_registry(GateMode::Master);
    let mut relaxed: Vec<String> = registry
        .tools()
        .filter(|t| t.armed_under(GateMode::Master) && !t.armed_under(GateMode::Paper))
        .map(|t| t.name().to_string())
        .collect();
    relaxed.sort();
    let armed_paper: BTreeSet<&str> = registry
        .tools()
        .filter(|t| t.armed_under(GateMode::Paper))
        .map(ToolEntry::name)
        .collect();
    let armed_master: BTreeSet<&str> = registry
        .tools()
        .filter(|t| t.armed_under(GateMode::Master))
        .map(ToolEntry::name)
        .collect();
    assert!(
        armed_paper.is_subset(&armed_master),
        "the paper gate must never arm a tool the master gate leaves ungated"
    );
    insta::assert_snapshot!(relaxed.join("\n"));
}

// --- Danger-policy rules ----------------------------------------------------

/// Forcing function the surface snapshot cannot be: the snapshot only pins
/// commands as they are classified today, and a snapshot diff can be
/// rubber-stamped. Here every tool whose words match a mutating verb must be
/// dangerous or consciously exempted — including commands that do not exist
/// yet. Do not weaken MUTATING_VERBS to make this pass; investigate every hit.
/// `set`/`create`/`delete` are in the list because a settings mutation is
/// destructive with no money verb anywhere in its name.
#[test]
fn mutating_commands_are_classified_dangerous() {
    const MUTATING_VERBS: &[&str] = &[
        "order",
        "buy",
        "sell",
        "cancel",
        "withdraw",
        "transfer",
        "allocate",
        "deallocate",
        "amend",
        "edit",
        "batch",
        "reset",
        "stake",
        "unstake",
        "set",
        "create",
        "delete",
    ];
    // Adding an entry here is the explicit "reviewed, mutates nothing on the
    // real account" decision. Each one needs a justification.
    const SAFE_EXCEPTIONS: &[&str] = &[
        // Local credential keystore, and no client can call it.
        "auth set",
        // Read-only status queries on prior allocations/orders.
        "earn allocate-status",
        "earn deallocate-status",
        "futures order-status",
        // Local manifest. `--mode live` is armed per call by the LocalOrLive
        // effect; real-account routing is gated at `workspace promote`.
        "workspace create",
        // Local session cursor, unauthenticated.
        "session state set",
        // Local artifact derivation from an already-recorded session; running
        // one goes through the normal session/order gates.
        "recipe create",
    ];

    let surface = Surface::shared();
    for tool in surface.iter() {
        let command = tool.path().to_string();
        let words: Vec<&str> = command.split([' ', '-']).collect();
        // Paper engines simulate against live prices; no real money moves.
        if words.contains(&"paper") {
            continue;
        }
        if !words.iter().any(|w| MUTATING_VERBS.contains(w)) {
            continue;
        }
        if SAFE_EXCEPTIONS.contains(&command.as_str()) {
            continue;
        }
        assert!(
            tool.effect.is_dangerous(),
            "`{command}` matches a mutating verb but is classified {:?}; either declare it \
             Dangerous in its `tools!` line (a reviewed surface-snapshot change) or add it to \
             SAFE_EXCEPTIONS with a justification",
            tool.effect
        );
    }
}

/// Cross-check with `workspace_guard::no_paper_equivalent`: every surface the
/// guard refuses inside a paper workspace must be classified always-dangerous —
/// a new money command that reaches neither list fails
/// `mutating_commands_are_classified_dangerous`, and one that reaches only the
/// surface fails here by omission review.
///
/// Every refusal must resolve: `Surface` carries the streaming `ws` leaves too,
/// so a name that no longer matches is a renamed command, not a stream.
#[test]
fn guard_refusal_set_is_always_dangerous() {
    const REFUSALS: &[&str] = &[
        "order amend",
        "order edit",
        "order batch",
        "order cancel-batch",
        "order cancel-after",
        "withdraw",
        "wallet-transfer",
        "withdrawal cancel",
        "earn allocate",
        "earn deallocate",
        "subaccount transfer",
        "futures order buy",
        "futures order sell",
        "futures edit-order",
        "futures cancel",
        "futures cancel-all",
        "futures cancel-after",
        "futures batch-order",
        "futures transfer",
        "futures wallet-transfer",
        "futures set-subaccount-status",
        "ws add-order",
        "ws amend-order",
        "ws cancel-order",
        "ws cancel-all",
        "ws cancel-after",
        "ws batch-add",
        "ws batch-cancel",
    ];
    let surface = Surface::shared();
    let by_command: std::collections::BTreeMap<String, &Entry> = surface
        .iter()
        .map(|tool| (tool.path().to_string(), tool.as_ref()))
        .collect();

    for command in REFUSALS {
        let tool = by_command
            .get(*command)
            .unwrap_or_else(|| panic!("guard refusal `{command}` names no command in the surface"));
        assert_eq!(
            tool.effect,
            Effect::Dangerous,
            "guard refusal `{command}` must be dangerous in every mode"
        );
    }
}

// --- Docs surface pins ------------------------------------------------------

/// The catalog is generated, not written: it is exactly what `--emit-catalog`
/// produces from the command tree. A failure here means the tree changed and
/// the checked-in file was not regenerated — not that the file is wrong to
/// differ.
///
/// Compared as JSON values, so formatting is not part of the contract.
#[test]
fn the_published_catalog_is_the_generated_one() {
    let generated = serde_json::to_value(kraken_mcp::Catalog::generate(
        env!("CARGO_PKG_VERSION").to_owned(),
    ))
    .unwrap();
    assert_eq!(
        generated,
        catalog(),
        "agents/tool-catalog.json is stale; regenerate it with `just catalog`"
    );
}

/// Pin the numeric surface claims in the agent-facing docs to the catalogs,
/// so adding a command (or error category) can't silently strand the
/// "N commands" prose again. Any number written before "commands" must be
/// the catalog total or its dangerous count; "N groups" must match the
/// catalog's group map; "N error categories" must match the error catalog.
#[test]
fn doc_surface_count_claims_match_the_catalogs() {
    const DOCS: &[(&str, &str)] = &[
        ("README.md", include_str!(repo_file!("README.md"))),
        ("CLAUDE.md", include_str!(repo_file!("CLAUDE.md"))),
        ("CONTEXT.md", include_str!(repo_file!("CONTEXT.md"))),
        ("AGENTS.md", include_str!(repo_file!("AGENTS.md"))),
        (
            "agents/README.md",
            include_str!(repo_file!("agents/README.md")),
        ),
        // The two catalogs no other test covered until their counts
        // drifted (134 and 173 against a catalog of 180): the LLM
        // index and the Codex plugin manifest both claim a command
        // count, so they pin here with the rest.
        ("llms.txt", include_str!(repo_file!("llms.txt"))),
        (
            ".codex-plugin/plugin.json",
            include_str!(repo_file!(".codex-plugin/plugin.json")),
        ),
        (
            "skills/README.md",
            include_str!(repo_file!("skills/README.md")),
        ),
        (
            "skills/INDEX.md",
            include_str!(repo_file!("skills/INDEX.md")),
        ),
    ];

    /// Numbers written immediately before `unit`, e.g. "173 commands".
    fn count_claims(text: &str, unit: &str) -> Vec<usize> {
        text.match_indices(unit)
            .filter_map(|(idx, _)| {
                let digits: String = text[..idx]
                    .chars()
                    .rev()
                    .take_while(char::is_ascii_digit)
                    .collect();
                digits.chars().rev().collect::<String>().parse().ok()
            })
            .collect()
    }

    let catalog = catalog();
    let commands = catalog["commands"].as_array().unwrap();
    let total = commands.len();
    let dangerous = commands
        .iter()
        .filter(|c| c["dangerous"].as_bool() == Some(true))
        .count();
    let groups = catalog["groups"].as_object().unwrap().len();
    let errors: Value =
        serde_json::from_str(include_str!(repo_file!("agents/error-catalog.json"))).unwrap();
    let categories = errors["categories"].as_array().unwrap().len();
    let tools = commands.iter().filter(|c| c["reach"] == "tool").count();
    // `[Unreleased]` becomes the release notes; older sections keep their own
    // release's numbers.
    let changelog = include_str!(repo_file!("CHANGELOG.md"));
    let after_heading = changelog
        .find("## [Unreleased]")
        .map_or(0, |at| at + "## [Unreleased]".len());
    let unreleased = &changelog[..changelog[after_heading..]
        .find("\n## [")
        .map_or(changelog.len(), |end| after_heading + end)];
    let skills = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills"))
        .unwrap()
        .filter(|entry| entry.as_ref().unwrap().path().join("SKILL.md").is_file())
        .count();

    let mut command_claims = 0;
    let mut tool_claims = 0;
    for (name, text) in DOCS
        .iter()
        .copied()
        .chain([("CHANGELOG.md [Unreleased]", unreleased)])
    {
        // Markdown re-wraps prose, which can split a number from its unit.
        let text = &text.split_whitespace().collect::<Vec<_>>().join(" ");
        for n in count_claims(text, " commands") {
            command_claims += 1;
            assert!(
                n == total || n == dangerous,
                "{name} claims {n} commands; the catalog has {total} (dangerous: {dangerous})"
            );
        }
        for n in count_claims(text, " groups") {
            assert_eq!(
                n, groups,
                "{name} claims {n} groups; the catalog has {groups}"
            );
        }
        for n in count_claims(text, " error categories") {
            assert_eq!(
                n, categories,
                "{name} claims {n} error categories; the error catalog has {categories}"
            );
        }
        // What an MCP client can call: "180 request/response commands" drifted
        // because the number does not sit right before "commands".
        for unit in [
            " request/response commands",
            " MCP tools",
            " of them are callable as MCP tools",
            " of the CLI's commands as MCP tools",
        ] {
            for n in count_claims(text, unit) {
                tool_claims += 1;
                assert_eq!(
                    n, tools,
                    "{name} claims {n}{unit}; the catalog has {tools} tools"
                );
            }
        }
        for unit in [" agent skills", " goal-oriented"] {
            for n in count_claims(text, unit) {
                assert_eq!(
                    n, skills,
                    "{name} claims {n}{unit}; the repo has {skills} skills"
                );
            }
        }
        let claims = every_command_as_a_tool(text);
        assert!(
            claims.is_empty(),
            "{name} says every command is an MCP tool ({claims:?}); kraken-mcp serves {tools} of \
             the {total}"
        );
    }
    assert!(
        tool_claims >= 1,
        "no MCP tool-count claim found — if the docs dropped it, retire this pin consciously"
    );
    assert!(
        command_claims >= 5,
        "only {command_claims} command-count claims found — if the docs dropped their numbers, \
         retire this pin consciously"
    );
}

/// Phrases such as "exposes all CLI commands as tools": false at any total,
/// because streams and interactive commands are not MCP tools.
fn every_command_as_a_tool(text: &str) -> Vec<String> {
    fn bare(word: &str) -> String {
        word.trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase()
    }
    fn ends_clause(word: &str) -> bool {
        word.ends_with([',', '.', ';', ':'])
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut claims = Vec::new();
    for (at, noun) in words.iter().enumerate() {
        if !matches!(bare(noun).as_str(), "command" | "commands") || ends_clause(noun) {
            continue;
        }
        let Some(opener) = (at.saturating_sub(5)..at)
            .rev()
            .take_while(|&i| !ends_clause(words[i]))
            .find(|&i| matches!(bare(words[i]).as_str(), "all" | "every"))
        else {
            continue;
        };
        if opener > 0 && bare(words[opener - 1]) == "not" {
            continue;
        }
        // Within one clause:
        // "commands [are|is] [callable] as [a|an] [structured] [MCP] tools".
        let mut tail = Vec::new();
        for word in &words[at + 1..words.len().min(at + 8)] {
            tail.push(bare(word));
            if ends_clause(word) {
                break;
            }
        }
        let mut next = 0;
        if matches!(tail.first().map(String::as_str), Some("are" | "is")) {
            next += 1;
        }
        if tail.get(next).is_some_and(|w| w != "as") {
            next += 1;
        }
        if tail.get(next).is_none_or(|w| w != "as") {
            continue;
        }
        next += 1;
        if matches!(tail.get(next).map(String::as_str), Some("a" | "an")) {
            next += 1;
        }
        if tail
            .get(next)
            .is_some_and(|w| w != "mcp" && !w.starts_with("tool"))
        {
            next += 1;
        }
        if tail.get(next).is_some_and(|w| w == "mcp") {
            next += 1;
        }
        if tail.get(next).is_some_and(|w| w.starts_with("tool")) {
            claims.push(words[opener..=at + 1 + next].join(" "));
        }
    }
    claims
}

#[test]
fn every_command_as_a_tool_is_recognised() {
    for claim in [
        "Built-in MCP server that exposes all CLI commands as tools for LLM agents.",
        "kraken-mcp serves all 188 commands as MCP tools.",
        "All 188 commands are callable as MCP tools.",
        "It exposes every command as an MCP tool.",
        "Expose all Kraken CLI commands as tools for AI agents",
        "It exposes all 188 CLI commands as structured tools that any MCP client can call.",
    ] {
        assert_eq!(every_command_as_a_tool(claim).len(), 1, "{claim}");
    }
    for fine in [
        "The catalog lists all 188 commands; kraken-mcp serves 152 MCP tools.",
        "It serves 152 of the CLI's commands as MCP tools.",
        "Not all commands are callable as MCP tools.",
        "All 188 commands, each callable one with a generated argument schema.",
        "every command as a JSON object",
    ] {
        assert!(every_command_as_a_tool(fine).is_empty(), "{fine}");
    }
}

/// The per-group tables in README.md and AGENTS.md must match the catalog.
/// Their counts drifted (websocket, workspace, auth, utility) and AGENTS.md
/// lost a group, which no total-count claim could notice. A row counts when
/// its first cell is a catalog group and a later cell is a plain number;
/// the permission tables carry no number and are skipped.
#[test]
fn doc_group_tables_match_the_catalog() {
    let mut groups = std::collections::BTreeMap::<String, usize>::new();
    for command in catalog()["commands"].as_array().unwrap() {
        *groups
            .entry(command["group"].as_str().unwrap().to_owned())
            .or_default() += 1;
    }
    for (name, text) in [
        ("README.md", include_str!(repo_file!("README.md"))),
        ("AGENTS.md", include_str!(repo_file!("AGENTS.md"))),
    ] {
        let mut seen = BTreeSet::new();
        for line in text.lines().filter(|l| l.trim_start().starts_with('|')) {
            let cells: Vec<&str> = line
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            let Some(&expected) = cells.first().and_then(|group| groups.get(*group)) else {
                continue;
            };
            let Some(count) = cells[1..].iter().find_map(|c| c.parse::<usize>().ok()) else {
                continue;
            };
            assert_eq!(
                count, expected,
                "{name}: group `{}` claims {count} commands; the catalog has {expected}",
                cells[0]
            );
            seen.insert(cells[0]);
        }
        let missing: Vec<&String> = groups
            .keys()
            .filter(|g| !seen.contains(g.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "{name}'s group table misses {missing:?}"
        );
    }
}

/// `/plugin install kraken-cli@kraken` resolves `kraken` as the marketplace and
/// `kraken-cli` as a plugin it lists from this repo; a rename on either side
/// breaks every new install while the build stays green.
#[test]
fn the_plugin_marketplace_lists_this_repos_plugin() {
    let marketplace: Value =
        serde_json::from_str(include_str!(repo_file!(".claude-plugin/marketplace.json"))).unwrap();
    let plugin: Value =
        serde_json::from_str(include_str!(repo_file!(".claude-plugin/plugin.json"))).unwrap();
    assert_eq!(marketplace["name"], "kraken");
    let entries = marketplace["plugins"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e["source"] == "./" && e["name"] == plugin["name"]),
        "no marketplace entry points at this repo's plugin `{}`",
        plugin["name"]
    );
}

/// Each plugin manifest and the hand-written error catalog publish a version,
/// and the release bump edits `Cargo.toml`: a file left behind ships the
/// previous number. The tool catalog's `cli_version` is generated, so
/// `the_published_catalog_is_the_generated_one` already holds it.
#[test]
fn published_manifests_carry_the_workspace_version() {
    const MANIFESTS: &[(&str, &str, &str)] = &[
        (
            ".claude-plugin/plugin.json",
            "version",
            include_str!(repo_file!(".claude-plugin/plugin.json")),
        ),
        (
            ".codex-plugin/plugin.json",
            "version",
            include_str!(repo_file!(".codex-plugin/plugin.json")),
        ),
        (
            ".cursor-plugin/plugin.json",
            "version",
            include_str!(repo_file!(".cursor-plugin/plugin.json")),
        ),
        (
            "gemini-extension.json",
            "version",
            include_str!(repo_file!("gemini-extension.json")),
        ),
        (
            "agents/error-catalog.json",
            "cli_version",
            include_str!(repo_file!("agents/error-catalog.json")),
        ),
    ];
    for (name, field, text) in MANIFESTS {
        let manifest: Value =
            serde_json::from_str(text).unwrap_or_else(|e| panic!("{name} is valid JSON: {e}"));
        assert_eq!(
            manifest[*field],
            env!("CARGO_PKG_VERSION"),
            "{name}'s `{field}` is not the workspace version; bump it with Cargo.toml"
        );
    }
}

/// The `kraken` binary must link neither the MCP protocol library nor the
/// REPL's line editor.
///
/// That is what the [`ToolHost`](kraken_mcp::ToolHost) seam is for: only the
/// `kraken-mcp` binary carries rmcp, and only `kraken-shell` carries rustyline.
/// All three binaries ship from this package so one installer carries them, so
/// this manifest names both crates and a manifest walk cannot tell; the built
/// files can. A linked crate leaves its registry directory (`rmcp-3.5.1/src/…`)
/// in its panic locations. The siblings are the positive controls: a scan that
/// no longer finds the crate in them fails instead of passing blind.
#[test]
fn the_kraken_binary_links_neither_rmcp_nor_rustyline() {
    let read =
        |path: &str| std::fs::read(path).unwrap_or_else(|e| panic!("{path} is readable: {e}"));
    let kraken = read(env!("CARGO_BIN_EXE_kraken"));
    let mcp = read(env!("CARGO_BIN_EXE_kraken-mcp"));
    let shell = read(env!("CARGO_BIN_EXE_kraken-shell"));

    assert!(
        links(&mcp, "rmcp"),
        "no rmcp path in kraken-mcp: the scan has gone blind"
    );
    assert!(
        links(&shell, "rustyline"),
        "no rustyline path in kraken-shell: the scan has gone blind"
    );
    assert!(
        !links(&kraken, "rmcp"),
        "kraken links rmcp; the protocol belongs behind the ToolHost seam"
    );
    assert!(
        !links(&kraken, "rustyline"),
        "kraken links rustyline; the line editor belongs to kraken-shell"
    );
}

/// The engine never depends on a front-end, so no front-end library can reach
/// `kraken` through it. Walked from `kraken-app`'s manifest outward, workspace
/// crates only: no registry crate depends on ours.
#[test]
fn the_engine_ships_no_front_end_dependency() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("this crate sits in the workspace's crates/ directory")
        .to_owned();

    let mut pending = vec!["kraken-app".to_owned()];
    let mut walked = BTreeSet::new();
    while let Some(name) = pending.pop() {
        if !walked.insert(name.clone()) {
            continue;
        }
        let manifest = crates.join(&name).join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest)
            .unwrap_or_else(|e| panic!("{name}'s manifest is readable: {e}"));
        let shipped = shipped_dependencies(&text);
        for front_end in [
            "rmcp",
            "rustyline",
            "kraken-mcp",
            "kraken-shell",
            "kraken-cli",
        ] {
            assert!(
                !shipped.contains(front_end),
                "{name} ships {front_end}; the engine never depends on a front-end"
            );
        }
        pending.extend(
            shipped
                .into_iter()
                .filter(|dep| crates.join(dep).is_dir())
                .map(str::to_owned),
        );
    }
    assert!(
        walked.len() > 1,
        "the walk followed nothing out of kraken-app — it has stopped reading manifests"
    );
}

/// Whether a binary carries code from `krate`: a path segment
/// `<krate>-<digit>` (its registry directory), which a crate named
/// `<krate>-macros` or a mention in prose does not match.
fn links(binary: &[u8], krate: &str) -> bool {
    let needle = format!("{krate}-");
    binary.windows(needle.len() + 2).any(|w| {
        matches!(w[0], b'/' | b'\\')
            && w[1..].starts_with(needle.as_bytes())
            && w[needle.len() + 1].is_ascii_digit()
    })
}

/// The dependency names one manifest ships: every `[dependencies]` table,
/// target-specific ones included, and no `dev-` or `build-` table.
///
/// Hand-read because the walk needs one field of the manifest and a TOML parser
/// would be a dependency of its own for it.
fn shipped_dependencies(manifest: &str) -> BTreeSet<&str> {
    let mut shipped = BTreeSet::new();
    let mut inside = false;
    for line in manifest.lines().map(str::trim) {
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            inside = section.ends_with("dependencies")
                && !section.contains("dev-")
                && !section.contains("build-");
            continue;
        }
        if !inside || line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            shipped.insert(name.trim());
        }
    }
    shipped
}
