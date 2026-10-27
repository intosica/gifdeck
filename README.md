# GifDeck

Animated **GIF / PNG widgets** for your desktop. One small manager window, **one process**,
any number of independent transparent widgets. Runs on **X11/Xorg** and on **Wayland
compositors with layer-shell, including COSMIC**. Packaged as a **Flatpak**.

## Features

| Requirement | How it is done |
|---|---|
| One process, many widgets | `Manager` owns every widget `gtk::Window` inside the single GTK application. Nothing is ever spawned. |
| GIF + PNG | `media.rs`: own GIF decoder loop (correct disposal + transparency), `png` for stills. Files are copied into the app's data folder on import. |
| Alpha transparency | Straight-alpha `gdk::MemoryTexture`s, windows with a transparent CSS background.<br><br>https://github.com/user-attachments/assets/0eeb0b56-9a7b-4158-9d78-cd174a15eab1 |
| Light on CPU/GPU | Frames are decoded once (on a worker thread) and uploaded once as textures. Each frame change is one `queue_draw()` driven by a one-shot timer, so a 10 fps GIF wakes the process 10×/s. Identical files share textures. |
| Decorations on/off per widget | `decorated` flag → normal titled window, or borderless + transparent. |
| Always on top / desktop layer per widget | X11: EWMH `_NET_WM_STATE_ABOVE` / `_BELOW`. Wayland: layer-shell `Top` / `Bottom` layer. |
| Layer priority by list order | Top of the list = front. X11: windows raised back→front inside their EWMH layer. Wayland: all widgets of a layer are drawn in **one shared layer surface** (`layer_host.rs`) that stacks them in exactly list order. |
| Presets | A preset is a named, ordered set of widgets; one is active; editing edits the active preset. |
| Weekday / time-of-day schedule | Rules `preset + days + (all day or from–until)`, first match wins, overnight windows supported (`22:00–06:00` belongs to the day it starts on). |
| Start at login | Flatpak: Background portal. Native: `~/.config/autostart` entry. UI switch reflects the real outcome. |
| COSMIC vs X11 detection | `env.rs` inspects the GDK display type, `XDG_CURRENT_DESKTOP`, layer-shell support and compositing; the Settings page shows it and the widget UI warns when a combination is limited. |

### New in this version
- **Click-through** instead of locking; per-widget *snap to edges* / *keep inside screen* (layer-shell).

- **Rotation**: any widget can be turned (`Rotation (°)` in its settings); the window grows to the rotated bounding box, so nothing is cropped. Masks rotate too, from degrees or by their green on-screen handle.

- **Folder widgets**: rotate sequentially, randomly (min/max seconds) or on a schedule.

- **Drag & drop** files/folders onto the window to import (onto the Masks tab: PNG masks).

- **Masks** tab: GIF or PNG alpha masks (animated GIFs work); apply per widget. Each widget can show its mask at an opacity (0% = clip only) and transform it on screen with Photoshop-style handles (size, stretch, skew, rotation) plus hue / contrast / brightness; the on-screen edit switch turns itself off when you click anything else.

- **Files** setting: use the source path, or keep organized copies in Home or Documents.

- **Single-row schedule** with 24h/12h toggle.

- **Application blacklist** (Settings) and per-widget *only when app open* / *minimize when app open*.

- **Quick Settings**: `data/gnome-shell-extension/` adds a toggle-all-widgets button (works with Quick Settings Tweaks). Install: copy to `~/.local/share/gnome-shell/extensions/gifdeck@gifdeck.dev/` and enable it.

- App detection is X11-only (no Wayland protocol exists for it); in Flatpak only exported apps are listed.


## Why GTK4 + gtk4-layer-shell (and not libcosmic)

The hard part of this app is **window layering and placement**, not the look of the
manager window. Needs: transparent borderless windows, per-window "above" / "below",
absolute placement, on **both** X11 and Wayland, from one binary.

- **GTK4** picks Wayland or X11 at runtime, so one binary and one code path for rendering.
  GTK apps run natively on COSMIC.
- **gtk4-layer-shell** gives wlr-layer-shell surfaces (the protocol COSMIC's compositor, sway,
  Hyprland, … implement). On X11 the same windows get EWMH hints instead (`x11.rs`).
- **libcosmic** (iced) would give the native COSMIC look. I did **not** build against it, so treat the
  following as an informed caution rather than a tested fact: as far as I know its layer-surface support
  lives in the Wayland-only (sctk) runtime, separate from the winit runtime that X11 needs, which makes
  "one binary, both X11 and layer-shell" awkward. Check current libcosmic before choosing it.
- Trade-off accepted: the manager window uses libadwaita, so it looks GNOME-ish on COSMIC.
  The widgets themselves (the part you look at all day) are just your images.

## What a widget becomes, per session

| Session | Borderless, Always-on-top | Borderless, Desktop | Normal layer / decorated |
|---|---|---|---|
| **X11 / Xorg** | window + `ABOVE` | window + `BELOW` | window (+ WM frame) |
| **Wayland + layer-shell** (COSMIC, sway, Hyprland) | layer surface, `Top` | layer surface, `Bottom` | ordinary toplevel (compositor places it) |
| **Wayland, no layer-shell** (GNOME) | ordinary window, layer ignored | ordinary window, layer ignored | ordinary toplevel |

Position can be set/remembered on X11 and for layer-shell widgets. Wayland places ordinary
windows itself, so the UI says so instead of pretending.

### Stacking order inside a layer (a gotcha worth knowing)

The layer-shell protocol only orders *layers* (background < bottom < top < overlay). The order of surfaces
**within** a layer is up to the compositor, and I measured it: with one surface per widget, sway stacked two
overlapping widgets differently from run to run and inverted my re-map attempts. So a user-defined priority order
cannot be built on top of "one surface per widget".

GifDeck therefore creates **one layer surface per layer** (at most two: Top and Bottom), covers the output, and draws the
widgets inside it as GTK children in list order. Its *input region* is only the union of the widgets' rectangles, so everything
between widgets still reaches the desktop below. Side benefits: no re-map flicker, fewer surfaces, and dragging is just moving
a child (the surface never moves, so the pointer coordinates stay valid for the whole grab).

Still compositor-defined: the order of *ordinary* Wayland windows (decorated / "Normal layer" widgets), which no client can control.

## Project layout
