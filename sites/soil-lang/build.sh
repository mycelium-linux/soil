#!/usr/bin/env sh
# Build the static soil-lang.org page and every example through the Soil stdlib.
# Cargo is frozen: this script only uses an existing soil-lang binary.
set -eu
cd "$(dirname "$0")/../.."

BIN=${SOIL_LANG:-target/debug/soil-lang}
if [ ! -x "$BIN" ]; then
  echo "no soil-lang binary at $BIN; cargo is frozen" >&2
  exit 1
fi

mkdir -p sites/soil-lang/dist/examples

"$BIN" \
  crates/soil-lang/stdlib/prelude.soil \
  crates/soil-lang/stdlib/ui.soil \
  crates/soil-lang/stdlib/highlight.soil \
  sites/soil-lang/soil-lang.soil \
  sites/soil-lang/build.soil \
  > sites/soil-lang/dist/index.html

for slug in counter todo calculator tictactoe tabs form; do
  "$BIN" \
    crates/soil-lang/stdlib/prelude.soil \
    crates/soil-lang/stdlib/ui.soil \
    crates/soil-lang/stdlib/highlight.soil \
    examples/ui/prelude.soil \
    "examples/ui/$slug.soil" \
    "sites/soil-lang/pages/$slug.soil" \
    > "sites/soil-lang/dist/examples/$slug.html"
done

echo "wrote sites/soil-lang/dist/"
