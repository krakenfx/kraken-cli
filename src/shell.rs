/// Interactive REPL shell with command history.
///
/// Dispatches commands through the same `AppContext` path used by one-shot CLI
/// invocations, ensuring output parity.
use std::path::PathBuf;

use clap::Parser;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::errors::{KrakenError, Result};
use crate::{client, dispatch, AppContext};

/// Run the interactive shell session.
pub(crate) async fn run(ctx: &AppContext) -> Result<()> {
    let history_path = history_file()?;
    let mut rl = DefaultEditor::new()
        .map_err(|e| KrakenError::Config(format!("Failed to initialize shell: {e}")))?;

    let _ = rl.load_history(&history_path);

    println!("Kraken CLI interactive shell. Type 'help' or 'exit'.");
    println!();

    loop {
        let prompt = "kraken> ";
        match rl.readline(prompt) {
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

                // Parse the line as if it were CLI args (prepend "kraken")
                let args: Vec<String> = std::iter::once("kraken".to_string())
                    .chain(shell_words(line))
                    .collect();

                match crate::Cli::try_parse_from(&args) {
                    Ok(cli) => {
                        let fmt = cli.output.unwrap_or(ctx.format);
                        let api_url = match cli.api_url {
                            Some(ref url) => match client::validate_url_scheme(url) {
                                Ok(()) => Some(url.clone()),
                                Err(e) => {
                                    crate::output::render_error(fmt, &e);
                                    continue;
                                }
                            },
                            None => ctx.api_url.clone(),
                        };
                        let futures_url = match cli.futures_url {
                            Some(ref url) => match client::validate_url_scheme(url) {
                                Ok(()) => Some(url.clone()),
                                Err(e) => {
                                    crate::output::render_error(fmt, &e);
                                    continue;
                                }
                            },
                            None => ctx.futures_url.clone(),
                        };
                        let shell_ctx = AppContext {
                            format: fmt,
                            verbose: cli.verbose || ctx.verbose,
                            api_url,
                            futures_url,
                            ws_public_url: ctx.ws_public_url.clone(),
                            ws_auth_url: ctx.ws_auth_url.clone(),
                            ws_l3_url: ctx.ws_l3_url.clone(),
                            api_key: cli.api_key.or_else(|| ctx.api_key.clone()),
                            api_secret: cli.api_secret.or_else(|| ctx.api_secret.clone()),
                            otp: cli.otp.or_else(|| ctx.otp.clone()),
                            force: cli.yes || ctx.force,
                            secret_from_flag: false,
                            mcp_mode: false,
                        };
                        if let Some(command) = cli.command {
                            if let Err(e) = Box::pin(dispatch(&shell_ctx, command)).await {
                                crate::output::render_error(shell_ctx.format, &e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("{e}");
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
