#!/usr/bin/env bash
# Regenerate cargo-sources.json after changing dependencies (Flatpak builds are offline).
# Needs: python3, aiohttp, tomlkit  (pip install aiohttp tomlkit)
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
cargo update --offline 2>/dev/null || true
curl -fsSL -o /tmp/flatpak-cargo-generator.py \
  https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
python3 /tmp/flatpak-cargo-generator.py Cargo.lock -o flatpak/cargo-sources.json
echo "Updated flatpak/cargo-sources.json"
