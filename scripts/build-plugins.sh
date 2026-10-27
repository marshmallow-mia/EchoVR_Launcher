#!/usr/bin/env bash
# Builds the first-party launcher plugins (plugins/, schema 2) as WebAssembly components
# and puts each plugin.wasm beside its plugin.json in docs/plugins/<id>/ (the example's
# into src/core/plugin_host/testdata/, the launcher's test fixture).
# Needs: rustup target add wasm32-unknown-unknown; cargo install wasm-tools.
# Usage: scripts/build-plugins.sh [plugin ...]   (default: all)
set -euo pipefail
cd "$(dirname "$0")/../plugins"
all=(example event-lobbies controllers game-server server-host)
names=("${@:-${all[@]}}")
for n in "${names[@]}"; do
  crate=${n//-/_}
  cargo build --release --target wasm32-unknown-unknown -p "$n"
  core="target/wasm32-unknown-unknown/release/$crate.wasm"
  if [ "$n" = example ]; then
    out=../src/core/plugin_host/testdata/example.wasm
  else
    out="../docs/plugins/$n/plugin.wasm"
  fi
  mkdir -p "$(dirname "$out")"
  wasm-tools component new "$core" -o "$out"
  wasm-tools validate --features component-model "$out"
  echo "$n: $(wc -c < "$out" | tr -d ' ') bytes -> $out"
done
