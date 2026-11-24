#!/usr/bin/env bash
# Type-checks the workspace for Windows (x86_64-pc-windows-msvc) from Linux.
# Nothing is linked: it catches what does not compile there, the CI builds
# and runs it (.github/workflows/build.yml).
#   tools/check-windows.sh [cargo check args…]
# The Windows standard library is downloaded once from static.rust-lang.org
# into target/win-sysroot (the Flatpak's Rust has no rustup to add targets).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
triple=x86_64-pc-windows-msvc
ver=$(rustc -V | awk '{print $2}')
sysroot="$root/target/win-sysroot"
if [ ! -d "$sysroot/lib/rustlib/$triple" ] || [ "$(cat "$sysroot/version" 2>/dev/null)" != "$ver" ]; then
    rm -rf "$sysroot" && mkdir -p "$sysroot/lib/rustlib"
    tmp=$(mktemp -d)
    curl -sSfL "https://static.rust-lang.org/dist/rust-std-$ver-$triple.tar.xz" | tar xJ -C "$tmp"
    mv "$tmp"/rust-std-*/rust-std-$triple/lib/rustlib/$triple "$sysroot/lib/rustlib/"
    rm -rf "$tmp"
    host=$(rustc -vV | sed -n 's/^host: //p')
    ln -sfn "$(rustc --print sysroot)/lib/rustlib/$host" "$sysroot/lib/rustlib/$host"
    echo "$ver" > "$sysroot/version"
fi
# Only the target's crates take the sysroot; build scripts and proc macros
# are built for Linux with the toolchain's own.
RUSTFLAGS="--sysroot $sysroot" CARGO_TARGET_DIR="$root/target/win" \
    exec cargo check --target $triple --workspace --all-targets "$@"
