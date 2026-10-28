//! Draft `SKILL.md` generation: a provenance record an agent can follow,
//! never authored strategy logic — the unresolved-rule marker keeps that
//! distinction explicit in the artifact itself.

use crate::{digest::DecisionDigest, manifest::Recipe};

/// Renders the recipe's draft skill. Pure and deterministic: the output is
/// pinned by a snapshot test, so a wording change is a deliberate diff.
pub fn draft_skill(recipe: &Recipe, digest: &DecisionDigest) -> String {
    let skill = recipe.driver.skill();
    let mut lines: Vec<String> = vec![
        "---".to_string(),
        format!("name: {}", recipe.name),
        "version: 0.1.0".to_string(),
        format!(
            "description: \"Generated from session {} using {}.\"",
            recipe.origin.session, skill
        ),
        "metadata:".to_string(),
        "  openclaw:".to_string(),
        "    category: \"recipe\"".to_string(),
        "  requires:".to_string(),
        "    bins: [\"kraken\"]".to_string(),
        format!("    skills: [\"{skill}\"]"),
        "---".to_string(),
        String::new(),
        format!("# Recipe: {}", recipe.name),
        String::new(),
        "> **DRAFT — UNRESOLVED RULES.** This file records what a finished session ran;"
            .to_string(),
        format!("> it is not authored strategy logic. The trading rules live in the `{skill}`"),
        "> skill — review this draft deliberately before trusting it.".to_string(),
    ];
    if let Some(description) = &recipe.description {
        lines.push(String::new());
        lines.push(description.clone());
    }

    lines.push(String::new());
    lines.push("## Provenance".to_string());
    lines.push(String::new());
    let label = recipe
        .origin
        .label
        .as_deref()
        .map(|label| format!(" (label `{label}`)"))
        .unwrap_or_default();
    lines.push(format!(
        "- Source session: `{}`{label}",
        recipe.origin.session
    ));
    lines.push(match &recipe.origin.experiment {
        Some(experiment) => format!(
            "- Lab experiment: `{experiment}` — grade evidence with `kraken lab score` and \
             `kraken lab compare`"
        ),
        None => {
            "- Lab evidence: unscored — no experiment stamped on the source session".to_string()
        }
    });
    let ended = recipe
        .origin
        .ended_at
        .map(|at| at.to_rfc3339())
        .unwrap_or_else(|| "open".to_string());
    lines.push(format!(
        "- Window: {} → {ended}",
        recipe.origin.started_at.to_rfc3339()
    ));
    lines.push(match &recipe.origin.tape {
        Some(tape) => {
            let hash = tape.content_hash.as_deref().unwrap_or("unhashed");
            format!("- Replay source: `{}` ({hash})", tape.source)
        }
        None => "- Market data: live stream (no replay source)".to_string(),
    });

    lines.push(String::new());
    lines.push("## Frozen defaults".to_string());
    lines.push(String::new());
    match &recipe.params {
        Some(params) => {
            lines.push(
                "Strategy params, verbatim from the source session — tuning them is the intended \
                 workflow:"
                    .to_string(),
            );
            lines.push(String::new());
            lines.push("```json".to_string());
            // A `serde_json::Value` cannot fail to serialize; the compact
            // Display form is the same JSON, so the fallback loses nothing.
            lines.push(serde_json::to_string_pretty(params).unwrap_or_else(|_| params.to_string()));
            lines.push("```".to_string());
        }
        None => {
            lines.push(format!(
                "The source session recorded no strategy params; the `{skill}` skill's defaults \
                 apply."
            ));
        }
    }

    lines.push(String::new());
    lines.push("## Observed decisions".to_string());
    lines.push(String::new());
    lines.push(format!("- total: {}", digest.total));
    for (kind, count) in &digest.counts {
        lines.push(format!("- {kind}: {count}"));
    }
    lines.push(match digest.median_gap_secs {
        Some(gap) => format!("- median gap between decisions: {gap}s"),
        None => "- fewer than two decisions — no cadence observed".to_string(),
    });

    lines.push(String::new());
    lines.push("## Start the next session".to_string());
    lines.push(String::new());
    lines.push(format!(
        "Inside a paper workspace funded with {} {}:",
        recipe.account.capital, recipe.account.currency
    ));
    lines.push(String::new());
    lines.push("```bash".to_string());
    lines.push("kraken session start \\".to_string());
    lines.push(format!(
        "  --symbols {} \\",
        recipe.market.symbols.join(",")
    ));
    lines.push(format!(
        "  --channels {} \\",
        recipe.market.channels.join(",")
    ));
    lines.push(format!("  --to {} \\", recipe.market.sinks.join(",")));
    lines.push(format!("  --strategy {skill} \\"));
    if let Some(params) = &recipe.params {
        // POSIX single-quote escaping (close, escaped quote, reopen), so a
        // quote inside a param value cannot break the generated command.
        let shell_safe = params.to_string().replace('\'', r"'\''");
        lines.push(format!("  --strategy-params '{shell_safe}' \\"));
    }
    lines.push(format!("  --label {}", recipe.name));
    lines.push("```".to_string());

    lines.push(String::new());
    lines.push("## Review the run".to_string());
    lines.push(String::new());
    lines.push("```bash".to_string());
    lines.push("kraken session show --session latest -o json".to_string());
    lines.push("kraken session decisions --session latest -o json".to_string());
    lines.push("kraken explain pnl --session latest -o json".to_string());
    lines.push("kraken lab score --session latest -o json".to_string());
    lines.push("```".to_string());

    lines.push(String::new());
    lines.push("## Safety boundary".to_string());
    lines.push(String::new());
    lines.push(
        "Paper only. This draft never places live orders; the paper→live gate stays".to_string(),
    );
    lines.push("`kraken workspace promote`, backed by lab evidence on this scope.".to_string());

    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use crate::{app::CreateSpec, digest::digest, manifest::from_session};
    use kraken_session::{manifest::StrategyRef, timeline::fixtures};

    /// A single quote inside a param value must not terminate the generated
    /// `--strategy-params '...'` argument.
    #[test]
    fn params_with_a_single_quote_stay_one_shell_argument() {
        let mut session = fixtures::manifest(vec![fixtures::jsonl_recording()]);
        session.window.ended_at = Some("2026-01-01T00:30:00Z".parse().expect("valid instant"));
        session.strategy = Some(StrategyRef {
            name: "my-skill".to_string(),
            params: Some(serde_json::json!({"note": "don't"})),
        });
        let spec = CreateSpec {
            name: "quoted".to_string(),
            description: None,
            driver: None,
            overwrite: false,
        };
        let recipe = from_session(
            &session,
            &spec,
            "2026-02-01T12:00:00Z".parse().expect("valid instant"),
        )
        .expect("derive");
        let draft = super::draft_skill(&recipe, &digest(&[]));
        assert!(
            draft.contains(r#"--strategy-params '{"note":"don'\''t"}'"#),
            "{draft}"
        );
    }
}
