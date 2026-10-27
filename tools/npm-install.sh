#!/usr/bin/env sh
# Installs the pinned npm formatter profiles under tools/npm/.
#
# No argument installs every profile, which is what a developer wants.
# `--ci <tool>` installs exactly one, which is all a CI job needs.
#
# Regenerate a lockfile with `npm install --registry https://registry.npmjs.org/`:
# a plain `npm install` writes the configured registry's host into every
# resolved URL, and this tree is published.
set -eu

PROFILES="markdownlint prettier"

# Resolved from the script's location so a source tarball without .git works.
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

if [ "${1:-}" = "--ci" ]; then
  [ $# -eq 2 ] || { echo "usage: $0 --ci <tool>" >&2; exit 2; }
  case " $PROFILES " in
    *" $2 "*) PROFILES=$2 ;;
    *) echo "$0: unknown profile '$2'; expected one of: $PROFILES" >&2; exit 2 ;;
  esac
fi

# An exact version pin says nothing about where the bytes came from. HTTPS is
# always required; set NPM_REGISTRY to also pin the exact host.
registry=$(npm config get registry)
case "$registry" in
  https://*) ;;
  *) echo "$0: npm registry '$registry' is not HTTPS" >&2; exit 1 ;;
esac
if [ -n "${NPM_REGISTRY:-}" ] && [ "$registry" != "$NPM_REGISTRY" ]; then
  echo "$0: npm registry is '$registry', expected '$NPM_REGISTRY'" >&2
  exit 1
fi

for profile in $PROFILES; do
  dir="tools/npm/$profile"
  [ -f "$dir/package.json" ] || { echo "$0: $dir/package.json is missing" >&2; exit 1; }
  if [ ! -f "$dir/package-lock.json" ]; then
    echo "$0: $dir/package-lock.json is missing; commit the lockfile" >&2
    exit 1
  fi
  echo "==> npm ci in $dir"
  # A formatter has no legitimate install hook.
  npm ci --prefix "$dir" --ignore-scripts --audit=false --fund=false
done
