#!/usr/bin/env bash
# Vercel build: the image has Node and pnpm but no Rust, so install a minimal toolchain with the
# wasm32 target and wasm-pack, build wasm/ into src/lib/dos, then the static site into dist/.
set -euo pipefail

WASM_PACK=0.13.1

# The image may ship a system cargo without rustup, which cannot add the wasm32 target.
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null; then
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path --target wasm32-unknown-unknown
fi
rustup target add wasm32-unknown-unknown

if ! command -v wasm-pack >/dev/null; then
  curl -sSfL "https://github.com/rustwasm/wasm-pack/releases/download/v$WASM_PACK/wasm-pack-v$WASM_PACK-x86_64-unknown-linux-musl.tar.gz" \
    | tar xz -C /tmp
  install -D -t "$HOME/.cargo/bin" "/tmp/wasm-pack-v$WASM_PACK-x86_64-unknown-linux-musl/wasm-pack"
fi

pnpm wasm
pnpm build
