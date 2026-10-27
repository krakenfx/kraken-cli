//! `kraken tape`: the read-only catalog of market memory — every standalone
//! `kraken record` tape plus the active scope's session tapes, as one list of
//! replayable refs (`tape:<name>`, `session:s<n>`). The printed ref is the
//! string `session start --from` and sealed lab session plans accept.

use super::Execute;
use crate::{cli::AppContext, config, errors::Result, output::CommandOutput};
use kraken_contract::tape::{List, TapeCommand};
use kraken_recording::{
    TapeBackend, TapeEntry, TapeRef, TapeState, describe_tape, list_recordings,
};
use std::path::Path;

impl Execute for List {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        // The shared library is global (never workspace-scoped); run tapes
        // belong to the active scope's journal, so its sessions ride along as
        // `session:s<n>` refs replayable from inside that scope.
        let base = config::config_dir()?;
        let mut tapes = list_recordings(&base)?;
        // Session rows ride along best-effort: an unresolvable scope (a ghost
        // workspace, a live refusal) must never hide the global library.
        if let Ok((scope, _)) = super::session::scope_dir(ctx) {
            tapes.extend(session_tapes(&scope)?);
        }
        // Re-sort the merged catalog (session tapes were appended unsorted),
        // keyed the same way list_recordings orders its own rows.
        tapes.sort_unstable_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        output(tapes)
    }
}

/// Session tapes are tapes too — composed here, so kraken-recording never
/// learns the session directory layout it writes into. Ordinal-ordered so
/// `session:s10` follows `session:s2`.
fn session_tapes(scope: &Path) -> Result<Vec<TapeEntry>> {
    let mut rows = Vec::new();
    for record in kraken_workspace::session::list(scope)? {
        let dir = kraken_session::session::session_dir(scope, record.session.ordinal());
        for &backend in TapeBackend::supported() {
            let path = dir.join(format!("tape.{}", backend.extension()));
            if path.exists() {
                let tape = TapeRef::Session {
                    name: record.session.to_string(),
                };
                rows.push(describe_tape(&tape, &path, backend));
            }
        }
    }
    Ok(rows)
}

fn output(tapes: Vec<TapeEntry>) -> Result<CommandOutput> {
    let rows: Vec<Vec<String>> = tapes.iter().map(row).collect();
    let data = serde_json::json!({ "group": "tape", "type": "list", "tapes": tapes });
    if rows.is_empty() {
        return Ok(CommandOutput::key_value(
            vec![("Tapes".to_string(), "none".to_string())],
            data,
        ));
    }
    let headers = [
        "Tape", "Backend", "State", "Symbols", "Channels", "Events", "Started", "In use",
    ]
    .map(String::from)
    .to_vec();
    Ok(CommandOutput::new(data, headers, rows))
}

fn row(entry: &TapeEntry) -> Vec<String> {
    let state = match entry.state {
        TapeState::Finalized => "finalized",
        TapeState::Unfinalized => "unfinalized",
        TapeState::Damaged => "damaged",
    };
    vec![
        entry.tape.clone(),
        entry.backend.to_string(),
        state.to_string(),
        entry.symbols.join(","),
        entry.channels.join(","),
        entry
            .events
            .map_or_else(|| "—".to_string(), |n| n.to_string()),
        entry
            .window_start
            .map_or_else(|| "—".to_string(), |at| at.to_rfc3339()),
        if entry.in_use { "yes" } else { "no" }.to_string(),
    ]
}

delegate_execute!(TapeCommand { List });
