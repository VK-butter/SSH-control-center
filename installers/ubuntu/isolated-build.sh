#!/usr/bin/env bash
# Test/build helper: only run in a disposable container, never on a deployed host.
set -euo pipefail
[[ -f /.dockerenv ]] || { echo 'Disposable Docker container required' >&2; exit 2; }
export DEBIAN_FRONTEND=noninteractive
mkdir -p /build
tar -C /source --exclude=target --exclude=node_modules --exclude=output --exclude=artifacts --exclude=dist --exclude=.git -cf - Cargo.toml Cargo.lock crates desktop scripts installers docs | tar -C /build -xf -
export CARGO_HOME=/cache/cargo
export RUSTUP_HOME=/cache/rustup
export CARGO_TARGET_DIR=/cache/target
export PATH="$CARGO_HOME/bin:$PATH"
if ! command -v rustup >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.90.0
fi
cd /build
rustc --version
cargo test --locked -p sshcc-core -p sshcc | tee /cache/core-tests.log
cargo build --release --locked -p sshcc -p sshcc-desktop | tee /cache/release-build.log
bash scripts/build-deb.sh --desktop --skip-build | tee /cache/package-build.log
version="$(python3 -c 'import tomllib;print(tomllib.load(open("Cargo.toml","rb"))["workspace"]["package"]["version"])')"
cp "dist/sshcc_${version}_amd64.deb" "dist/sshcc-desktop_${version}_amd64.deb" dist/sshcc-client-ubuntu-template.deb /dist/
cp "$CARGO_TARGET_DIR/release/sshcc" "$CARGO_TARGET_DIR/release/sshcc-desktop" "$CARGO_TARGET_DIR/release/sshcc-window" /cache/
python3 installers/ubuntu/test-setup.py | tee /cache/setup-tests.log
