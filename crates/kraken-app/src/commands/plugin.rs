//! Scaffold CLI-client extensions. There is no plugin ABI: the CLI's
//! versioned JSON output is the extension API, so this family only removes
//! the activation energy — it writes a runnable client and never executes
//! one. The manifest is `extension.json`, NOT `plugin.json`: that exact
//! name is taken by the agent-host manifests describing kraken-cli itself
//! as a plugin, and the release check version-locks them.

use super::Execute;
use crate::{
    cli::AppContext,
    config,
    errors::{KrakenError, Result},
    output::CommandOutput,
};
use kraken_contract::plugin::{Init, Kind, Lang, List, PluginCommand};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The library root: `<config_dir>/plugins/`.
fn plugins_root() -> Result<PathBuf> {
    Ok(config::config_dir()?.join("plugins"))
}

const MANIFEST_FILE: &str = "extension.json";

/// Descriptive identity for developers and future CLI versions; the CLI
/// never loads or executes it.
#[derive(Debug, Serialize, Deserialize)]
struct ExtensionManifest {
    name: String,
    version: String,
    language: String,
    kind: String,
    entrypoint: String,
    /// The wire the entrypoint consumes: kraken-cli's JSON output contract.
    input: String,
}

impl Execute for Init {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        // The safe name grammar guards the path join, same as recipes.
        if self.name.parse::<kraken_session::SessionName>().is_err() {
            return Err(KrakenError::Validation(format!(
                "extension name '{}' is not a safe directory name — use letters, digits, '.', \
                 '-', '_'",
                self.name
            )));
        }
        let dir = match &self.path {
            Some(path) => path.clone(),
            None => plugins_root()?.join(&self.name),
        };
        if dir.exists() && std::fs::read_dir(&dir)?.next().is_some() {
            return Err(KrakenError::Validation(format!(
                "'{}' already holds files — remove it, choose another name, or pass --path",
                dir.display()
            )));
        }
        std::fs::create_dir_all(&dir)?;
        // Canonicalize BEFORE the run line lands in SKILL.md/README.md: those
        // files get symlinked elsewhere, where a relative --path would lie.
        let dir = dir.canonicalize()?;

        let entrypoint = match self.lang {
            Lang::Python => "main.py",
            Lang::Bash => "main.sh",
        };
        let manifest = ExtensionManifest {
            name: self.name.clone(),
            version: "0.1.0".to_string(),
            language: self.lang.to_string(),
            kind: self.kind.to_string(),
            entrypoint: entrypoint.to_string(),
            input: "kraken-json".to_string(),
        };
        kraken_recording::write_json_atomic(&dir.join(MANIFEST_FILE), &manifest)?;

        let entrypoint_path = dir.join(entrypoint);
        std::fs::write(
            &entrypoint_path,
            entrypoint_source(&self.name, self.lang, self.kind),
        )?;
        mark_executable(&entrypoint_path)?;
        // Quoted like the install line: the macOS library path holds a space
        // ("Application Support"), so an unquoted paste would break.
        let run_line = match self.lang {
            Lang::Python => format!("python3 \"{}\" BTC/USD", entrypoint_path.display()),
            Lang::Bash => format!("bash \"{}\" BTC/USD", entrypoint_path.display()),
        };
        std::fs::write(
            dir.join("SKILL.md"),
            skill_source(&self.name, self.kind, &run_line, entrypoint),
        )?;
        std::fs::write(
            dir.join("README.md"),
            readme_source(&self.name, self.kind, &run_line),
        )?;

        let install = format!("ln -s \"{}\" ~/.openclaw/skills/", dir.display());
        let pairs = vec![
            ("Extension".to_string(), self.name.clone()),
            ("Kind".to_string(), self.kind.to_string()),
            ("Language".to_string(), self.lang.to_string()),
            ("Path".to_string(), dir.display().to_string()),
            ("Run".to_string(), run_line.clone()),
            ("Install skill".to_string(), install.clone()),
        ];
        let data = serde_json::json!({
            "group": "plugin",
            "type": "init",
            "extension": manifest,
            "path": dir.display().to_string(),
            "run": run_line,
            "install": install,
        });
        Ok(CommandOutput::key_value(pairs, data))
    }
}

/// The entrypoint must be directly runnable; a scaffold the user first has
/// to chmod reads as broken.
fn mark_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// One `plugin list` row; damage is described, never hidden.
#[derive(Debug, Serialize)]
struct ListedExtension {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entrypoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    damaged: Option<String>,
}

impl Execute for List {
    async fn execute(self, _ctx: &AppContext) -> Result<CommandOutput> {
        let root = plugins_root()?;
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => Some(entries),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        let mut rows: Vec<ListedExtension> = Vec::new();
        for entry in entries.into_iter().flatten() {
            let entry = entry?;
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.parse::<kraken_session::SessionName>().is_err() {
                continue;
            }
            let manifest_path = entry.path().join(MANIFEST_FILE);
            let row = match std::fs::read_to_string(&manifest_path) {
                Ok(data) => match serde_json::from_str::<ExtensionManifest>(&data) {
                    Ok(manifest) => ListedExtension {
                        name,
                        kind: Some(manifest.kind),
                        language: Some(manifest.language),
                        entrypoint: Some(manifest.entrypoint),
                        damaged: None,
                    },
                    Err(err) => ListedExtension {
                        name,
                        kind: None,
                        language: None,
                        entrypoint: None,
                        damaged: Some(format!("{MANIFEST_FILE} cannot be decoded: {err}")),
                    },
                },
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => ListedExtension {
                    name,
                    kind: None,
                    language: None,
                    entrypoint: None,
                    damaged: Some(format!("directory holds no {MANIFEST_FILE}")),
                },
                Err(err) => return Err(err.into()),
            };
            rows.push(row);
        }
        rows.sort_unstable_by(|a, b| a.name.cmp(&b.name));

        let table: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                vec![
                    row.name.clone(),
                    row.kind.clone().unwrap_or_else(|| "—".to_string()),
                    row.language.clone().unwrap_or_else(|| "—".to_string()),
                    row.entrypoint.clone().unwrap_or_else(|| "—".to_string()),
                    row.damaged.clone().unwrap_or_default(),
                ]
            })
            .collect();
        let data = serde_json::json!({
            "group": "plugin",
            "type": "list",
            "plugins": rows,
        });
        let headers = ["Extension", "Kind", "Language", "Entrypoint", "Note"]
            .map(String::from)
            .to_vec();
        Ok(CommandOutput::new(data, headers, table))
    }
}

/// The kraken command each kind demonstrates; every one is public market
/// data, so the scaffold runs with no credentials.
fn kind_command(kind: Kind) -> &'static str {
    match kind {
        Kind::Indicator => "ohlc",
        Kind::Alert | Kind::Driver => "ticker",
        Kind::Exporter => "trades",
    }
}

fn entrypoint_source(name: &str, lang: Lang, kind: Kind) -> String {
    match lang {
        Lang::Python => {
            let (extra_imports, body) = match kind {
                Kind::Indicator => ("", PY_INDICATOR_BODY),
                Kind::Alert => ("", PY_ALERT_BODY),
                Kind::Exporter => ("import csv\n", PY_EXPORTER_BODY),
                Kind::Driver => ("", PY_DRIVER_BODY),
            };
            PY_TEMPLATE
                .replace("__NAME__", name)
                .replace("__KIND__", &kind.to_string())
                .replace("__EXTRA_IMPORTS__", extra_imports)
                .replace("__BODY__", body)
        }
        Lang::Bash => BASH_TEMPLATE
            .replace("__NAME__", name)
            .replace("__KIND__", &kind.to_string())
            .replace("__COMMAND__", kind_command(kind)),
    }
}

const PY_TEMPLATE: &str = r#"#!/usr/bin/env python3
"""__NAME__: a kraken-cli __KIND__ extension.

The CLI's versioned JSON output is the extension API: shell out to
`kraken ... -o json`, parse, compute, print. No SDK, no ABI, no rebuild of
kraken-cli — see README.md.
"""
__EXTRA_IMPORTS__import json
import subprocess
import sys


def kraken(*args):
    """One kraken call: JSON on stdout, a JSON error envelope on failure."""
    result = subprocess.run(
        ["kraken", *args, "-o", "json"],
        capture_output=True,
        text=True,
        check=False,
    )
    payload = json.loads(result.stdout)
    if result.returncode != 0:
        raise SystemExit(f"kraken failed: {payload.get('error')}: {payload.get('message')}")
    return payload


def main():
    pair = sys.argv[1] if len(sys.argv) > 1 else "BTC/USD"
__BODY__

if __name__ == "__main__":
    main()
"#;

// The bodies consume the CLI's post-schema payloads (named fields), never
// Kraken's raw wire arrays — `-o json` reshapes through `schema::*`, and
// the shapes below are pinned by that module's tests.

const PY_INDICATOR_BODY: &str = r#"    payload = kraken("ohlc", pair, "--interval", "5")
    closes = [float(candle["close"]) for candle in payload["candles"]]
    window = closes[-20:]
    print(json.dumps({"pair": pair, "candles": len(closes), "sma20": sum(window) / len(window)}))
"#;

const PY_ALERT_BODY: &str = r#"    threshold = float(sys.argv[2]) if len(sys.argv) > 2 else 0.0
    payload = kraken("ticker", pair)
    book = next(v for v in payload.values() if isinstance(v, dict))
    last = float(book["last_price"])
    print(json.dumps({"pair": pair, "last": last, "above_threshold": last >= threshold}))
"#;

const PY_EXPORTER_BODY: &str = r#"    payload = kraken("trades", pair, "--count", "20")
    writer = csv.writer(sys.stdout)
    writer.writerow(["price", "volume", "time", "side"])
    for trade in payload["trades"]:
        writer.writerow([trade["price"], trade["volume"], trade["time"], trade["side"]])
"#;

const PY_DRIVER_BODY: &str = r#"    payload = kraken("ticker", pair)
    book = next(v for v in payload.values() if isinstance(v, dict))
    last = float(book["last_price"])
    # A signal, never an order: the agent driving the session stays
    # responsible for action (see SKILL.md).
    print(json.dumps({"pair": pair, "last": last, "signal": "hold"}))
"#;

const BASH_TEMPLATE: &str = r#"#!/usr/bin/env bash
# __NAME__: a kraken-cli __KIND__ extension.
#
# The CLI's versioned JSON output is the extension API: call kraken,
# transform with your tools of choice, emit JSON — see README.md.
set -euo pipefail
pair="${1:-BTC/USD}"
exec kraken __COMMAND__ "$pair" -o json
"#;

fn skill_source(name: &str, kind: Kind, run_line: &str, entrypoint: &str) -> String {
    SKILL_TEMPLATE
        .replace("__NAME__", name)
        .replace("__KIND__", &kind.to_string())
        .replace("__RUN_LINE__", run_line)
        .replace("__ENTRYPOINT__", entrypoint)
}

const SKILL_TEMPLATE: &str = r#"---
name: __NAME__
version: 0.1.0
description: "A kraken-cli __KIND__ extension: __NAME__."
metadata:
  openclaw:
    category: "extension"
  requires:
    bins: ["kraken"]
---

# __NAME__

A __KIND__ extension over kraken-cli's JSON contract. Run it with:

```bash
__RUN_LINE__
```

The CLI's versioned JSON output is the extension API: the entrypoint shells
out to `kraken ... -o json`, transforms, and prints. Edit `__ENTRYPOINT__`
to change what it computes; kraken-cli itself never needs a rebuild.

## Safety boundary

This extension reads public market data and prints. It places no orders;
order placement stays behind kraken-cli's own confirmation gates.
"#;

fn readme_source(name: &str, kind: Kind, run_line: &str) -> String {
    README_TEMPLATE
        .replace("__NAME__", name)
        .replace("__KIND__", &kind.to_string())
        .replace("__RUN_LINE__", run_line)
}

const README_TEMPLATE: &str = r#"# __NAME__

A kraken-cli __KIND__ extension. The contract:

- kraken-cli's versioned JSON output IS the extension API. The entrypoint
  shells out to `kraken ... -o json` (or consumes recorded JSONL) and prints;
  there is no plugin ABI, no dynamic loading, and kraken-cli never executes
  this code.
- The entrypoint is dependency-free (standard library only), so it runs
  wherever kraken-cli does.

Run it:

```bash
__RUN_LINE__
```

`extension.json` is this extension's identity for `kraken plugin list` and
future CLI versions. It is deliberately NOT named `plugin.json` — that exact
filename belongs to the agent-host manifests that describe kraken-cli itself
as a plugin, and the release check version-locks those.
"#;

delegate_execute!(PluginCommand { Init, List });
