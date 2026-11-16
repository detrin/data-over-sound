#!/usr/bin/env bash
# Vercel build: the image has Node and pnpm but no Rust, so install a minimal toolchain with the
# wasm32 target and wasm-pack, build wasm/ into src/lib/dos, then the static site into dist/.
set -euo pipefail

WASM_PACK=0.13.1

if ! command -v cargo >/dev/null; then
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --target wasm32-unknown-unknown
fi
source "$HOME/.cargo/env"
rustup target add wasm32-unknown-unknown

if ! command -v wasm-pack >/dev/null; then
  curl -sSfL "https://github.com/rustwasm/wasm-pack/releases/download/v$WASM_PACK/wasm-pack-v$WASM_PACK-x86_64-unknown-linux-musl.tar.gz" \
    | tar xz -C /tmp
  install "/tmp/wasm-pack-v$WASM_PACK-x86_64-unknown-linux-musl/wasm-pack" "$HOME/.cargo/bin/"
fi

pnpm wasm
pnpm build
