/// The trading engine that a key is for. Spot and Futures keys are separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Spot,
    Futures,
}

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Self::Spot => "Spot",
            Self::Futures => "Futures",
        }
    }

    /// The `(key, secret)` env var names. clap reads them; these names only
    /// appear in messages.
    pub fn env_names(self) -> (&'static str, &'static str) {
        match self {
            Self::Spot => ("KRAKEN_API_KEY", "KRAKEN_API_SECRET"),
            Self::Futures => ("KRAKEN_FUTURES_API_KEY", "KRAKEN_FUTURES_API_SECRET"),
        }
    }
}
