#!/bin/sh
# The notes app in C, Rust and Plumb on wasm-server, in its Linux
# benchmark image (only Linux numbers count; see wasm-server's CLAUDE.md).
# Needs a wasm-server checkout ($WASM_SERVER, default ~/Dev/wasm-server)
# with the image built (`docker build -t wasm-server-bench .` there),
# wasi-sdk for the Plumb and C apps, and wit-bindgen for the C app.
set -e
cd "$(dirname "$0")"
WS=${WASM_SERVER:-$HOME/Dev/wasm-server}
PLUMB=${PLUMB_BIN:-../../compiler/target/release/plumb}
mkdir -p work
$PLUMB build --target wasm-server pnotes.plumb -o work/pl.wasm
(cd "$WS/guests-c/notes" && ./build.sh >/dev/null)
cp "$WS/guests-c/notes/notes.wasm" work/c.wasm
cp "$WS/guests-c/notes/notes.sql" work/notes.sql
cp in-docker.sh work/
docker run --rm -v "$PWD:/cmp" --entrypoint sh wasm-server-bench /cmp/work/in-docker.sh
