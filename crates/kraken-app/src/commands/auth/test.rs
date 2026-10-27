use crate::{AppContext, commands::Execute, errors::Result, output::CommandOutput};
use kraken_contract::auth::Test;

impl Execute for Test {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        let source = ctx.spot.source()?;
        let balances = ctx
            .spot_account()?
            .sign(&kraken_rest::spot::account::BalanceQuery::default())
            .await?;

        let pairs = vec![
            ("Status".into(), "Authentication successful".to_string()),
            ("Source".into(), source.to_string()),
        ];
        let json = serde_json::json!({
            "status": "success",
            "source": source.to_string(),
            "balances": balances,
        });
        Ok(CommandOutput::key_value(pairs, json))
    }
}
