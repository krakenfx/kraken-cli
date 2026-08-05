/// Interactive shell using the same context and dispatch path as one-shot commands.
use std::path::PathBuf;

use clap::parser::ValueSource;
use clap::{CommandFactory, FromArgMatches};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::AppContext;
use crate::commands::dispatch;
use crate::errors::{KrakenError, Result};

/// Run the interactive shell session.
pub(crate) async fn run(ctx: &AppContext) -> Result<()> {
    let history_path = history_file()?;
    let mut rl = DefaultEditor::new()
        .map_err(|e| KrakenError::Config(format!("Failed to initialize shell: {e}")))?;

    let _ = rl.load_history(&history_path);

    println!("Kraken CLI interactive shell. Type 'help' or 'exit'.");
    println!();

    // Global tracing cannot change mid-session, so explain ignored flags once.
    let mut logging_flag_notice_shown = false;

    let prompt = scope_prompt(ctx);
    loop {
        match rl.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(line);

                match line {
                    "exit" | "quit" => break,
                    "help" => {
                        print_shell_help();
                        continue;
                    }
                    _ => {}
                }

                let args: Vec<String> = std::iter::once("kraken".to_string())
                    .chain(shell_words(line))
                    .collect();

                // Only an explicit per-line flag may override the shell's resolved scope.
                let matches = match crate::Cli::command().try_get_matches_from(&args) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("{e}");
                        continue;
                    }
                };
                let workspace_from_flag =
                    matches.value_source("workspace") == Some(ValueSource::CommandLine);
                let cli = match crate::Cli::from_arg_matches(&matches) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("{e}");
                        continue;
                    }
                };

                if !logging_flag_notice_shown && (cli.verbose || cli.log_format.is_some()) {
                    eprintln!(
                        "note: logging is configured once when the shell starts; \
                         per-command --verbose/--log-format are ignored. Restart with \
                         `kraken shell -v --log-format <fmt>` or set RUST_LOG instead."
                    );
                    logging_flag_notice_shown = true;
                }
                // Inherit startup values while preserving one-shot validation and precedence.
                let shell_ctx =
                    match AppContext::from_cli_with_fallback(&cli, ctx, workspace_from_flag) {
                        Ok(c) => c,
                        Err(e) => {
                            crate::output::render_error(cli.output.unwrap_or(ctx.format), &e);
                            continue;
                        }
                    };
                if let Some(command) = cli.command
                    && let Err(e) = Box::pin(dispatch(&shell_ctx, command)).await
                {
                    crate::output::render_error(shell_ctx.format, &e);
                }
            }
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("Shell error: {e}");
                break;
            }
        }
    }

    if rl.save_history(&history_path).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&history_path, std::fs::Permissions::from_mode(0o600));
        }
    }
    Ok(())
}

fn history_file() -> Result<PathBuf> {
    let dir = crate::config::config_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("history"))
}

fn print_shell_help() {
    println!("Available commands (same as CLI):");
    println!("  status, server-time, assets, pairs, ticker, ohlc, orderbook, ...");
    println!("  balance, trade-balance, open-orders, closed-orders, ...");
    println!("  order buy/sell, futures instruments/tickers, ...");
    println!("  auth set/show/test/reset, setup");
    println!("  exit / quit - Exit the shell");
    println!();
    println!("Use --help on any command for details.");
    println!(
        "Logging (--verbose/--log-format) is set once at startup; \
         start with `kraken shell -v --log-format <fmt>` or set RUST_LOG to change it."
    );
}

/// Split a shell line into words, handling simple quoting.
fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escape_next = false;

    for ch in line.chars() {
        if escape_next {
            current.push(ch);
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if !in_single_quote => {
                escape_next = true;
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            ' ' | '\t' if !in_single_quote && !in_double_quote => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// The prompt says which account this shell trades: the scoped
/// workspace and its mode, or the real account. A workspace whose contract
/// cannot be read still shows its name — the first command will surface the
/// real error.
fn scope_prompt(ctx: &AppContext) -> String {
    match ctx.workspace.as_deref() {
        None => "kraken [live]> ".to_string(),
        Some(name) => match crate::commands::workspace_guard::active_manifest(ctx) {
            Ok(Some(manifest)) => format!("kraken [{name}:{}]> ", manifest.mode),
            _ => format!("kraken [{name}]> "),
        },
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;

    /// The prompt is the shell's always-on mode visibility — pinned.
    #[test]
    fn prompt_names_the_scope_and_mode() {
        let cli = crate::Cli::try_parse_from(["kraken"]).expect("bare invocation parses");
        let ctx = AppContext::from_cli(&cli).expect("context resolves");
        assert_eq!(scope_prompt(&ctx), "kraken [live]> ");

        let scoped = ctx.rescope(Some("ghost".to_string()));
        assert_eq!(
            scope_prompt(&scoped),
            "kraken [ghost]> ",
            "an unreadable contract still names the scope"
        );
    }

    #[test]
    fn shell_words_basic() {
        assert_eq!(shell_words("foo bar baz"), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn shell_words_quoted() {
        assert_eq!(
            shell_words("ticker \"XBT USD\" --verbose"),
            vec!["ticker", "XBT USD", "--verbose"]
        );
    }

    #[test]
    fn shell_words_single_quoted() {
        assert_eq!(shell_words("ticker 'XBT USD'"), vec!["ticker", "XBT USD"]);
    }
}
