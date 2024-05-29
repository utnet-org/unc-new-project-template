#!/bin/bash
TARGET="${CARGO_TARGET_DIR:-../target}"
set -e

cd "$(dirname $0)"
# mkdir -p res
# cd contract
rustup target add wasm32-unknown-unknown
# no abi
RUSTFLAGS='-C link-arg=-s' cargo build --package contract --target wasm32-unknown-unknown --release
# with abi
# unc dev-tool build
cp $TARGET/wasm32-unknown-unknown/release/contract.wasm  ../res/social_web3.wasm