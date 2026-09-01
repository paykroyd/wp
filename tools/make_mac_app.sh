#!/bin/bash
# Build a universal wp binary and assemble a double-clickable wp.app.
#
#   tools/make_mac_app.sh [--seed-google] [out-dir]
#
# --seed-google copies the [google] client from your own
# ~/.config/wp/config.toml into the bundle's first-run config template
# (alongside keymap/theme = classic). Use it only for a zip you hand to
# someone you'd share that client with — never for a public release.
set -euo pipefail
cd "$(dirname "$0")/.."
SEED=0
[ "${1:-}" = "--seed-google" ] && { SEED=1; shift; }
OUT="${1:-dist}"
VER="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --target aarch64-apple-darwin -p wp
cargo build --release --target x86_64-apple-darwin -p wp
mkdir -p "$OUT"
lipo -create -output "$OUT/wp" \
  target/aarch64-apple-darwin/release/wp \
  target/x86_64-apple-darwin/release/wp

APP="$OUT/wp.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
sed "s/WP_VERSION/$VER/g" tools/mac-app/Info.plist > "$APP/Contents/Info.plist"
install -m 755 tools/mac-app/launch "$APP/Contents/MacOS/launch"
install -m 755 "$OUT/wp" "$APP/Contents/Resources/wp"

{
  echo 'keymap = "classic"'
  echo 'theme = "classic"'
  if [ "$SEED" = 1 ]; then
    CFG="${XDG_CONFIG_HOME:-$HOME/.config}/wp/config.toml"
    echo
    echo '[google]'
    grep -E '^\s*(client_id|client_secret)\s*=' "$CFG"
  fi
} > "$APP/Contents/Resources/config.toml"

codesign --force --deep -s - "$APP"
ditto -c -k --keepParent "$APP" "$OUT/wp-$VER-macos.app.zip"
zip -qj "$OUT/wp-$VER-macos-universal.zip" "$OUT/wp"
echo "built: $OUT/wp-$VER-macos.app.zip and $OUT/wp-$VER-macos-universal.zip (v$VER, seed=$SEED)"
