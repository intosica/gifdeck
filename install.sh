#!/usr/bin/env bash
# Build GifDeck from source with your system Rust toolchain and install it.
#
#   ./install.sh                      # -> ~/.local (no sudo needed)
#   ./install.sh --prefix /usr/local  # -> system-wide (uses sudo when needed)
#   ./install.sh --uninstall          # remove what this script installed
#
# Whatever the distro, the only hard requirements are cargo (>= 1.85), pkg-config
# and the development packages of GTK 4, libadwaita and gtk4-layer-shell.
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
DO_BUILD=1
DO_INSTALL=1

usage() { sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0; }

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) PREFIX="${2:?--prefix needs a path}"; shift 2 ;;
        --prefix=*) PREFIX="${1#*=}"; shift ;;
        --uninstall) DO_BUILD=0; DO_INSTALL=0; shift ;;
        -h|--help) usage ;;
        *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
done

APP=dev.gifdeck.GifDeck
BIN="$PREFIX/bin/gifdeck"
DESKTOP="$PREFIX/share/applications/$APP.desktop"
ICON="$PREFIX/share/icons/hicolor/scalable/apps/$APP.svg"
METAINFO="$PREFIX/share/metainfo/$APP.metainfo.xml"
SUDO=""
[ -w "$(dirname "$PREFIX")" ] || SUDO=sudo

here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

if [ "$DO_INSTALL" = 0 ]; then
    for f in "$BIN" "$DESKTOP" "$ICON" "$METAINFO"; do
        [ -e "$f" ] && $SUDO rm -f "$f" && echo "removed $f"
    done
    if command -v update-desktop-database >/dev/null; then $SUDO update-desktop-database -q "$PREFIX/share/applications" || true; fi
    if command -v gtk-update-icon-cache >/dev/null; then $SUDO gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true; fi
    echo "GifDeck uninstalled from $PREFIX (your config and imported media are kept)."
    exit 0
fi

# ---- dependencies ----------------------------------------------------------
die() { echo "error: $*" >&2; exit 1; }

command -v cargo >/dev/null || die "cargo not found. Install Rust: https://rustup.rs
  Debian/Ubuntu: sudo apt install cargo
  Fedora: sudo dnf install cargo
  Arch: sudo pacman -S rust"
command -v pkg-config >/dev/null || die "pkg-config not found (needed to find GTK)."

missing=""
for mod in gtk4 libadwaita-1 gtk4-layer-shell-0; do
    pkg-config --exists "$mod" 2>/dev/null || missing="$missing $mod"
done
if [ -n "$missing" ]; then
    die "missing development packages:$missing
  Debian/Ubuntu: sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libgtk4-layer-shell-dev
  Fedora:        sudo dnf install gcc pkgconf-pkg-config gtk4-devel libadwaita-devel gtk4-layer-shell-devel
  Arch:          sudo pacman -S base-devel pkgconf gtk4 libadwaita gtk4-layer-shell
  openSUSE:      sudo zypper install gcc pkg-config gtk4-devel libadwaita-devel gtk4-layer-shell-devel
  Nix:           nix-shell -p cargo pkg-config gtk4 libadwaita gtk4-layer-shell
If your distro has no gtk4-layer-shell 1.0, build it from source first:
  meson setup build -Dexamples=false -Ddocs=false -Dtests=false -Dsmoke-tests=false -Dintrospection=false -Dvapi=false"
fi

if [ "$DO_BUILD" = 1 ]; then
    echo "Building GifDeck (release)…"
    cargo build --release --locked
fi

# ---- install ---------------------------------------------------------------
install_file() { # src dest mode
    $SUDO install -Dm"$3" "$1" "$2"
    echo "installed $2"
}

install_file target/release/gifdeck "$BIN" 755
install_file data/icons/hicolor/scalable/apps/"$APP".svg "$ICON" 644
install_file data/"$APP".metainfo.xml "$METAINFO" 644

# Desktop entry with the absolute binary path, so it works without ~/.local/bin in PATH.
sed "s|^Exec=.*|Exec=$BIN|" data/"$APP".desktop > /tmp/"$APP".desktop.$$
install_file /tmp/"$APP".desktop.$$ "$DESKTOP" 644
rm -f /tmp/"$APP".desktop.$$

if command -v update-desktop-database >/dev/null; then $SUDO update-desktop-database -q "$PREFIX/share/applications" || true; fi
if command -v gtk-update-icon-cache >/dev/null; then $SUDO gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true; fi

echo
echo "GifDeck installed to $PREFIX."
case ":$PATH:" in *":$PREFIX/bin:"*) ;; *) echo "Note: $PREFIX/bin is not in your PATH; use the app menu or $BIN." ;; esac
echo "Run '$BIN' for the manager window, or add the widgets to your autostart with '$BIN --background'."
