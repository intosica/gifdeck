# AGENT HANDOFF: GifDeck

Read this first. Update it before you stop.

## What this is
Rust + GTK4 + libadwaita desktop app: animated GIF/PNG/WebP widgets, one process, presets, schedules.
Map: `src/ctx.rs` (the only place that mutates state), `src/model.rs` (state + pure logic, unit-tested),
`src/ui/*` (manager window; `clock.rs` = analog/digital schedule pickers), `src/hotkeys.rs` (X11 global key grabs),
`src/manager.rs` + `widget.rs` + `layer_host.rs` (the widget windows). Check with `cargo check --offline` and `cargo test --offline`.

## Done recently
Per-preset hotkeys (+ `gifdeck --preset NAME`), analog/digital schedule clocks, schedule end action, swap behaviour
(global or per preset), mask hover preview, preset-rename focus fix. README has simulated screenshots
(`docs/screens/make_mockups.py`: no real captures; if a real one is ever needed, use only the `Default` preset).

## Never commit
Personal paths, usernames, real widget files, `.pi/`, screenshots of the user's own presets.

## GNOME UI lesson: dropdown values cut off by "…" (do not repeat)
**Symptom:** `adw::ComboRow` / `gtk::DropDown` options show as `Free: the swap stays until the sch…`.

**Why it keeps happening:** an agent writes each option as a full sentence (and adds a subtitle to the row) because it
reads well in code. But a `ComboRow` shows its selected value in a label *beside* the title, ellipsized at the end, and that
label gets whatever width is left after the title and subtitle. In the default 480 px window there is almost none, so GTK
truncates it. The agent never opens the window at the real width, so the cut-off is never seen; code that compiles looks finished.

**Rules:**
1. Option labels are 1-3 words ("Random preset", "Lock"). Explanations go in the group `description`, the row subtitle of a row *without* a value, or a tooltip.
2. For a `ComboRow` whose options are longer than a word or two, call `.use_subtitle(true)`: the value then sits under the title with the full row width.
3. Do not "fix" this with `ellipsize(None)` or `max_width_chars`; shorten the text instead.
4. A `gtk::DropDown` is as wide as its longest option, so do not put one in a narrow container (FlowBox row, small popover) with long text.
5. Check every new row at the default window width (480 px) before calling the UI done.
