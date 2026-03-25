# Contributing to kraken-cli

Thank you for your interest in contributing. This document explains how to submit changes and report issues.

## How to Contribute

Open PRs and issues here on GitHub. The maintainers review submissions and merge accepted changes into future releases. Response times vary. Not every PR will be accepted, and some may require changes before they can be merged.

## Issues and Feature Requests

Use [GitHub Issues](https://github.com/krakenfx/kraken-cli/issues) for bug reports and feature requests.

**Bug reports.** Include:

- The command you ran (redact any API keys or secrets).
- The output you received (use `-o json` for structured output).
- What you expected instead.
- Your OS and architecture (`uname -a`).
- The CLI version (`kraken --version`).

**Feature requests.** Describe what you want the CLI to do and why. Include a usage example if possible. Prefix the title with `Feature request:` so it is easy to find.

## Submitting Changes

1. Fork the repository.
2. Create a branch from `main`.
3. Make your changes.
4. Run `cargo build` and `cargo test` locally.
5. Open a pull request against `main`.

### PR Guidelines

- Keep PRs focused. One logical change per PR.
- Write clear commit messages. Describe what changed and why.
- Add or update tests for new behavior.
- Do not include unrelated formatting or refactoring changes.
- Enable "Allow edits from maintainers" on your PR so the team can make small adjustments before merging.

### What Makes a Good PR

- Bug fixes with a reproducer or test case.
- New commands or flags that extend existing command groups.
- Documentation improvements.
- Test coverage for untested code paths.

### What Will Likely Be Declined

- Large architectural changes without prior discussion. Open an issue first.
- Changes that break backward compatibility without strong justification.
- Features that require exchange infrastructure changes outside the CLI.

## Code Style

- Follow existing patterns in the codebase.
- Run `cargo clippy -- -D warnings` before submitting.
- No nightly-only features. The crate must build on stable Rust.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).

## Security

If you discover a security vulnerability, do **not** open a public issue. Follow the responsible disclosure process described in [DISCLAIMER.md](DISCLAIMER.md).
