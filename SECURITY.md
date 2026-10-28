# Security Policy

`kraken-cli` handles Kraken API credentials and can place live orders,
cancellations, and withdrawals. We take security reports seriously.

## Reporting a Vulnerability

Do not open a public GitHub issue or pull request for a security vulnerability.

Report suspected vulnerabilities privately to <bugbounty@kraken.com>. Reports
are handled through Kraken's bug bounty program.

Please include:

- A description of the vulnerability and its impact.
- Steps to reproduce, or a proof of concept.
- The `kraken-cli` version (`kraken --version`) and your OS and architecture.
- Any relevant configuration, with all API keys, secrets, and account
  identifiers redacted.

Do not include live credentials in a report. If you need to demonstrate
credential handling, use a test key or paper trading.

Please give us reasonable time to release a fix before you disclose the issue
publicly.

## Supported Versions

This project is pre-1.0 and under active development. Only the latest release
receives security fixes, so check that you are on it before reporting.

## Scope

This policy covers the `kraken-cli` code in this repository. Report security
issues in the Kraken exchange or the Kraken API to <bugbounty@kraken.com> as
well. For help with your Kraken account, visit
[support.kraken.com](https://support.kraken.com).

## Verifying Release Binaries

Release binaries are signed with
[minisign](https://jedisct1.github.io/minisign/). Verify a download against its
signature before you run it, as described in
[Verifying Binaries](README.md#verifying-binaries). Treat a binary that fails
verification as untrusted, and report it.

## Credential Safety

Your API key and secret grant access to your Kraken account. See
[API Key Security](DISCLAIMER.md#api-key-security) for how to handle them.
