# Disclaimer

## Experimental Software

This software is experimental and under active development. It is provided "as is" without warranty of any kind. Use it at your own risk.

## Not Financial Advice

This tool is a command-line interface for the Kraken cryptocurrency exchange. It does not provide financial advice, trading recommendations, or investment guidance. It executes commands exactly as instructed. Any trading decisions are yours alone.

## Real Money, Real Risk

Commands executed through this CLI interact with the live Kraken exchange and can result in real financial transactions. Orders, withdrawals, and transfers are irreversible once processed. Incorrect commands, software bugs, or agent errors can result in financial loss.

Before using this tool with real funds:
- Test your workflows using paper trading (`kraken paper`), which uses live prices but no real money.
- Validate orders with `--validate` before submitting.
- Use restricted API keys with only the permissions you need.
- Start with small amounts.

## AI Agent Use

When used by AI agents or automated systems, the same risks apply. The agent executes commands based on its programming and the instructions it receives. Neither the CLI nor the agent validates whether a trade is financially sound.

If you grant an AI agent access to your API credentials:
- You are responsible for all actions the agent takes on your behalf.
- The agent can place orders, cancel orders, and (if permitted by the API key) withdraw funds.
- Use the `dangerous` field in `agents/tool-catalog.json` to identify high-risk commands.
- Use Kraken's API key permission system to limit what the agent can do.
- Enable the dead man's switch (`kraken order cancel-after`) for unattended sessions.

## Liability

The authors and contributors of this software accept no liability for financial losses, missed trades, incorrect executions, or any other damages resulting from the use of this tool, whether used manually or by an automated agent.

## Support and Responsible Disclosure

This tool is open-sourced under the MIT license by Payward, Inc. (Kraken). Bug reports and feature requests are handled through [GitHub Issues](https://github.com/krakenfx/kraken-cli/issues). For exchange account support, visit [support.kraken.com](https://support.kraken.com).

## API Key Security

Your API key and secret grant access to your Kraken account. Treat them like passwords:
- Never share them in public repositories, logs, or chat messages.
- Never pass `--api-secret` on the command line in shared environments (use environment variables or `--api-secret-stdin`).
- Rotate keys regularly.
- Use the most restrictive permissions possible for your use case.
- Expose the CLI and its MCP tools only to the people (and agents) who should be able to operate the Kraken account. A shared machine, multi-user server, or shared agent session with credentials in the environment gives everyone on it full account access.

## Data Collection

`kraken feedback` uploads the feedback body you provide, plus metadata (`instance_id`, `agent_client`, CLI version, and related fields such as OS/arch and optional `--llm-model`), to Kraken over HTTPS so the product can be improved. Do not put API keys, secrets, passwords, or personal data in the feedback body. Separately, client-identity headers (`instance_id`, agent client, CLI version) already accompany ordinary Kraken API calls made by this CLI; they are not unique to feedback. Not running `kraken feedback` is the opt-out for uploading free-form feedback text. See `docs/feedback-ingest.md` for retention and limits.
