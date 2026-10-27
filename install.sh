#!/bin/sh
# Kept so existing links keep working. The release installer is the supported
# one: it puts kraken, kraken-mcp and kraken-shell in ~/.cargo/bin.
set -eu

url="https://github.com/krakenfx/kraken-cli/releases/latest/download/kraken-cli-installer.sh"
installer=$(mktemp)
trap 'rm -f "$installer"' EXIT

# Downloaded first, not piped: a failed download must not run an empty script.
curl --proto '=https' --tlsv1.2 -LsSf "$url" -o "$installer"
sh "$installer" "$@"

# The old version of this script put only `kraken` in /usr/local/bin, and that
# copy can shadow the new one on PATH. The installer always puts kraken-mcp next
# to kraken, so a kraken there without it is the old copy, not this install. The
# old script moved a file there; a symlink is the user's own.
if [ -f /usr/local/bin/kraken ] && [ ! -L /usr/local/bin/kraken ] && [ ! -e /usr/local/bin/kraken-mcp ]; then
  echo "note: an older kraken is still at /usr/local/bin/kraken; remove it with: sudo rm /usr/local/bin/kraken" >&2
fi
