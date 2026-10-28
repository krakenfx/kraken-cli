use super::active_key::{active_cell, apply_choice, can_prompt, choose, report_override};
use crate::{
    AppContext,
    commands::Execute,
    config,
    errors::{KrakenError, Result},
    output::CommandOutput,
};
use kraken_auth::KeySlot;
use kraken_contract::auth::{SpotKey, Switch};

impl Execute for Switch {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let store = config::credential_store()?;
        let choice = match self.key {
            Some(SpotKey::ApiKey) => Some(KeySlot::ApiKey),
            Some(SpotKey::SignIn) => Some(KeySlot::SignIn),
            None if can_prompt() => {
                let stored = store.load()?;
                choose(&stored, stored.active().unwrap_or(KeySlot::ApiKey))?
            }
            None => {
                return Err(KrakenError::Validation(
                    "Name the key to activate: `kraken auth switch api-key` or `kraken auth \
                     switch sign-in`."
                        .into(),
                ));
            }
        };
        let stored = apply_choice(&store, choice)?;
        let active = stored.active();

        if stored.is_active_key_expired() {
            tracing::warn!(run = "kraken auth login", "the active sign-in key expired");
        }
        let overridden_by = report_override(active, ctx.spot.source_in_use());

        let pairs = vec![("Active Spot Key".into(), active_cell(active))];
        let json = serde_json::json!({
            "active_spot_key": active,
            "overridden_by": overridden_by,
        });
        Ok(CommandOutput::key_value(pairs, json))
    }
}
