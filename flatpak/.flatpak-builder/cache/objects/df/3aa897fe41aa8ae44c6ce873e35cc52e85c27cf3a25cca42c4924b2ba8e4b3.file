#!/bin/sh
# Launcher installed as /app/bin/gifdeck.
#
# gtk4-layer-shell must be loaded *before* libwayland-client, otherwise layer
# surfaces silently turn into ordinary windows. Linking order usually takes
# care of it, but preloading makes it independent of how the toolchain orders
# libraries (see gtk4-layer-shell's linking.md). On X11 it is simply unused.
LAYER_SHELL=/app/lib/libgtk4-layer-shell.so
if [ -f "$LAYER_SHELL" ]; then
    export LD_PRELOAD="$LAYER_SHELL${LD_PRELOAD:+:$LD_PRELOAD}"
fi
exec /app/libexec/gifdeck "$@"
