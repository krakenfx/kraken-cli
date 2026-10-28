//! `kraken replay`: arguments for replaying a recorded session.

#[derive(Debug, clap::Args, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ReplayCommand {
    /// Which session: `latest`, an ordinal (`s3`), or a label.
    #[arg(long, default_value = "latest")]
    #[serde(default = "crate::value_types::latest_session")]
    pub session: String,
    /// Playback speed multiplier: 1 = real time (recorded gaps reproduced),
    /// 2 = twice as fast, 0.5 = half speed. Accepts 0.01 to 1000.
    #[arg(long, default_value_t = 1.0, value_parser = parse_speed)]
    #[serde(default = "crate::value_types::real_time")]
    pub speed: f64,
}

/// Replay speed bounds. Mirrored from `kraken_replay`, which this crate must
/// not depend on; `kraken-cli` pins the two against each other in a test.
const MIN_SPEED: f64 = 0.01;
const MAX_SPEED: f64 = 1000.0;

/// The bounds above, for the executing crate to pin against the replay
/// engine's.
pub const SPEED_BOUNDS: (f64, f64) = (MIN_SPEED, MAX_SPEED);

/// Reject a speed outside the playback clock's range at parse time, so the
/// error names the bound instead of surfacing from the replay engine.
fn parse_speed(raw: &str) -> std::result::Result<f64, String> {
    let speed: f64 = raw
        .parse()
        .map_err(|_| format!("'{raw}' is not a number"))?;
    if !speed.is_finite() || !(MIN_SPEED..=MAX_SPEED).contains(&speed) {
        return Err(format!("speed must be within {MIN_SPEED} to {MAX_SPEED}"));
    }
    Ok(speed)
}
