//! The run-window grammar shared by lab plans (`live:<window>`) and
//! `session start --for`. Pure grammar: it returns the failure *reason*
//! without a flag name, so each caller frames the error against its own flag.

use std::time::Duration;

/// Parse a human duration (`20s`, `5m`, `1h`, `2h 30m`) into a `Duration`, or
/// return the human reason it is malformed. Delegates to [`humantime`] so the
/// grammar is a maintained, documented spec rather than hand-rolled; a sealed
/// `live:<window>` and a `--for` stay in lock-step because both route here.
/// Zero is rejected: a zero window seals a session that recorded nothing.
pub fn parse_window(window: &str) -> Result<Duration, String> {
    let window = humantime::parse_duration(window.trim()).map_err(|why| why.to_string())?;
    if window.is_zero() {
        return Err("a window must be longer than zero".to_string());
    }
    Ok(window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_units_humantime_documents() {
        assert_eq!(parse_window("20s").unwrap(), Duration::from_secs(20));
        assert_eq!(parse_window("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_window("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_window("24h").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_window("1d").unwrap(), Duration::from_secs(86_400));
        // Compound spans are the reason to lean on the crate rather than a
        // single number+unit split.
        assert_eq!(parse_window("2h 30m").unwrap(), Duration::from_secs(9_000));
    }

    #[test]
    fn rejects_junk_and_bare_numbers() {
        for bad in ["24", "h", "", "soon", "10x"] {
            assert!(parse_window(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn rejects_a_zero_window() {
        for zero in ["0s", "0m", "0h 0m"] {
            assert!(parse_window(zero).is_err(), "{zero}");
        }
    }
}
