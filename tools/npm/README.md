# Pinned npm formatters

Two npm-owned formatters run against this tree. Each lives in its own profile
so a CI job installs one and nothing else, and each is pinned to an exact
version — `run-npm-tool.mjs` refuses to execute when the installed bytes differ
from the pin.

## Vetting

Recorded when a dependency is added or its pin moves, because the pin is what
CI executes.

| Package            | Version | Licence | Released     | Transitive | Audit                  |
| ------------------ | ------- | ------- | ------------ | ---------- | ---------------------- |
| `markdownlint-cli` | 0.49.1  | MIT     | 2026-07-14   | 73         | clean at 2026-09-23    |
| `prettier`         | 3.9.10  | MIT     | 2026-10-10   | 0          | clean at 2026-10-27    |

Both are actively maintained, MIT (compatible), and standard tooling with broad
adoption. `markdownlint-cli` brings 73 transitive packages, which is the whole
supply-chain cost of this directory; `prettier` vendors its dependencies and
brings none.

`markdownlint-cli` is pinned at 0.49.1 rather than 0.49.0 deliberately: 0.49.0
resolves `js-yaml` and `smol-toml` versions carrying four high-severity
denial-of-service advisories (GHSA-52cp-r559-cp3m, GHSA-5p4m-2wfm-xmqj,
GHSA-2883-xcg3-v3hh, GHSA-7w5x-hrqm-74c2). The patch release clears all four.

## Re-auditing

Audit against the public registry explicitly:

```bash
npm audit --package-lock-only --registry https://registry.npmjs.org/
```

If your npm registry is a mirror, keep the flag: a mirror that cannot audit
packages it does not host reports one "Component not found [low]" per package
instead. That is not a vulnerability count, and reading it as one hides real
findings: the four advisories above were invisible until this flag was added.

## Changing a pin

1. Edit `<profile>/package.json` to the new exact version.
2. `npm install --package-lock-only --registry https://registry.npmjs.org/`. A
   plain `npm install` writes your configured registry's host into every
   resolved URL, and this tree is published.
3. Re-audit as above and update the table.
