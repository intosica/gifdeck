#!/usr/bin/env bash
# Build and install GifDeck as a Flatpak for the current user.
#   ./flatpak/build.sh            build + install
#   ./flatpak/build.sh --bundle   also write gifdeck.flatpak (single file you can send to others)
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

command -v flatpak >/dev/null || { echo "flatpak is not installed" >&2; exit 1; }
command -v flatpak-builder >/dev/null || { echo "flatpak-builder is not installed (flatpak install flathub org.flatpak.Builder)" >&2; exit 1; }

if ! flatpak-builder --user --install --force-clean --disable-cache --install-deps-from=flathub \
  --repo=repo build-dir dev.gifdeck.GifDeck.yml; then
  cat >&2 <<'MSG'

Build failed. To see what the first module installed into /app, run:
  flatpak-builder --run build-dir dev.gifdeck.GifDeck.yml sh -c \
    'find /app -name "*layer-shell*"; echo PKG_CONFIG_PATH=$PKG_CONFIG_PATH; pkg-config --modversion gtk4-layer-shell-0'
and send the first error from the "gtk4-layer-shell" module's output.
MSG
  exit 1
fi

if [[ "${1:-}" == "--bundle" ]]; then
  flatpak build-bundle repo gifdeck.flatpak dev.gifdeck.GifDeck
  echo "Wrote $(pwd)/gifdeck.flatpak"
fi
echo "Run it with: flatpak run dev.gifdeck.GifDeck"
