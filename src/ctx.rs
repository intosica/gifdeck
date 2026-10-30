//! Application context: the one place that mutates state.
//!
//! Every user action goes through here: it edits `AppState`, saves it,
//! tells the `Manager` to reconcile the windows, and asks the UI to refresh.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::env::Env;
use crate::manager::Manager;
use crate::media;
use crate::model::{self, AppRef, AppState, Mask, MaskXf, Preset, Rule, Storage, Win, WidgetConfig};

/// What the UI should redraw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refresh {
    /// The widget list was restructured (add / remove / reorder / preset switch).
    Widgets,
    /// Only x/y changed (drag, window manager move): update the number fields.
    Positions,
    /// A preset was renamed: update labels only (the user may be typing in one).
    Names,
    /// Mask list changed.
    Masks,
    /// A mask transform changed by dragging: update the number fields.
    MaskXf,
    /// Which widget is in mask-edit mode changed.
    MaskEdit,
    /// The rule list was restructured.
    Schedule,
    /// Master switch changed.
    Enabled,
    /// Blacklist changed.
    Blacklist,
    /// Only the "right now" status needs recomputing.
    Status,
    /// X11: the current workspace changed; show only its widgets.
    Workspace,
    All,
}

pub struct AppCtx {
    pub app: adw::Application,
    pub env: Env,
    pub state: RefCell<AppState>,
    pub manager: Rc<Manager>,
    sync_timer: RefCell<Option<glib::SourceId>>,
    on_refresh: RefCell<Option<Rc<dyn Fn(Refresh)>>>,
    on_toast: RefCell<Option<Rc<dyn Fn(&str)>>>,
    /// Outcome of the previous schedule evaluation (`None` = never evaluated).
    /// A preset is only auto-applied when this *changes*, so a manual
    /// choice sticks until the schedule moves on to something else.
    last_scheduled: RefCell<Option<Option<String>>>,
    /// Preset that was active before an "only if application is open" rule took over; comes back when the app closes.
    app_return: RefCell<Option<String>>,
    /// X11: the workspace the user was on at the last look.
    seen_workspace: std::cell::Cell<Option<u32>>,
    /// Other applications' windows as of the last poll (X11).
    wins: RefCell<Vec<Win>>,
    /// Widget whose mask is being transformed on screen.
    pub edit_mask: RefCell<Option<String>>,
}

/// Put `src` (file or folder) where `storage` says; returns the path the widget should use.
fn store(src: &Path, kind: &str, storage: Storage) -> std::io::Result<PathBuf> {
    let Some(dir) = storage.dir(kind) else { return Ok(src.to_path_buf()) };
    fs::create_dir_all(&dir)?;
    let name = src.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !src.is_dir() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    let mut dest = dir.join(&name);
    for n in 2.. {
        if !dest.exists() {
            break;
        }
        dest = dir.join(format!("{stem} ({n}){ext}"));
    }
    if src.is_dir() {
        fs::create_dir_all(&dest)?;
        for f in media::list_folder(src) {
            fs::copy(&f, dest.join(f.file_name().unwrap_or_default()))?;
        }
    } else {
        fs::copy(src, &dest)?;
    }
    Ok(dest)
}

impl AppCtx {
    pub fn new(app: adw::Application, env: Env) -> Rc<AppCtx> {
        Rc::new_cyclic(|weak: &Weak<AppCtx>| {
            let w = weak.clone();
            let on_moved: crate::widget::MovedCb = Rc::new(move |id, x, y| {
                if let Some(ctx) = w.upgrade() {
                    ctx.widget_dragged(id, x, y);
                }
            });
            let w = weak.clone();
            let on_closed: crate::widget::ClosedCb = Rc::new(move |id| {
                if let Some(ctx) = w.upgrade() {
                    ctx.widget_closed(id);
                }
            });
            let w = weak.clone();
            let on_error: Rc<dyn Fn(&str, &str)> = Rc::new(move |name, msg| {
                if let Some(ctx) = w.upgrade() {
                    ctx.toast(&format!("Could not load {name}: {msg}"));
                }
            });
            let w = weak.clone();
            let on_mask_xf: Rc<dyn Fn(&str, MaskXf)> = Rc::new(move |id, xf| {
                if let Some(ctx) = w.upgrade() {
                    ctx.edit_widget(id, |c| c.mask_xf = xf);
                    ctx.refresh(Refresh::MaskXf);
                }
            });
            let w = weak.clone();
            let on_mask_exit: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(ctx) = w.upgrade() {
                    ctx.set_mask_edit(None);
                }
            });
            AppCtx {
                edit_mask: RefCell::new(None),
                manager: Manager::new(app.clone(), env.clone(), on_moved, on_closed, on_error, on_mask_xf, on_mask_exit),
                app,
                env,
                state: RefCell::new(AppState::load()),
                sync_timer: RefCell::new(None),
                on_refresh: RefCell::new(None),
                on_toast: RefCell::new(None),
                last_scheduled: RefCell::new(None),
                app_return: RefCell::new(None),
                seen_workspace: std::cell::Cell::new(None),
                wins: RefCell::new(Vec::new()),
            }
        })
    }

    pub fn set_ui_hooks(&self, refresh: Rc<dyn Fn(Refresh)>, toast: Rc<dyn Fn(&str)>) {
        *self.on_refresh.borrow_mut() = Some(refresh);
        *self.on_toast.borrow_mut() = Some(toast);
    }

    pub fn refresh(&self, what: Refresh) {
        let cb = self.on_refresh.borrow().clone();
        if let Some(cb) = cb {
            cb(what);
        }
    }

    pub fn toast(&self, msg: &str) {
        let cb = self.on_toast.borrow().clone();
        match cb {
            Some(cb) => cb(msg),
            None => eprintln!("gifdeck: {msg}"),
        }
    }

    // ---------------------------------------------------------------- persistence / sync

    pub fn save(&self) {
        self.state.borrow().save();
    }

    pub fn widget_cfg(&self, id: &str) -> Option<WidgetConfig> {
        self.state.borrow().active_preset().widgets.iter().find(|w| w.id == id).cloned()
    }

    /// Save + reconcile windows right now.
    pub fn flush(&self) {
        self.save();
        self.apply();
    }

    /// Reconcile windows with the state, the master switch and the application rules.
    fn apply(&self) {
        self.manager.set_per_workspace(self.state.borrow().per_workspace);
        let widgets = self.state.borrow().effective_widgets(&self.wins.borrow());
        self.manager.sync(&widgets);
    }

    /// Look at the other applications' windows (every second); re-apply only when something changed.
    pub fn poll_apps(self: &Rc<Self>) {
        if self.env.backend != crate::env::Backend::X11 {
            return;
        }
        if self.state.borrow().per_workspace {
            let now = crate::x11::current_desktop();
            if self.seen_workspace.replace(now) != now {
                self.refresh(Refresh::Workspace);
            }
        }
        let wins = crate::x11::windows();
        if wins != *self.wins.borrow() {
            *self.wins.borrow_mut() = wins;
            self.apply();
            self.tick_schedule(); // "only if application is open" rules follow the windows
        }
    }

    pub fn set_widgets_enabled(self: &Rc<Self>, on: bool) {
        if self.state.borrow().widgets_enabled != on {
            self.state.borrow_mut().widgets_enabled = on;
            self.flush();
            self.refresh(Refresh::Enabled);
        }
    }

    pub fn add_blacklist(self: &Rc<Self>, app: AppRef) {
        {
            let mut st = self.state.borrow_mut();
            if st.blacklist.iter().any(|a| a.id == app.id) {
                return;
            }
            st.blacklist.push(app);
        }
        self.flush();
        self.refresh(Refresh::Blacklist);
    }

    pub fn remove_blacklist(self: &Rc<Self>, id: &str) {
        self.state.borrow_mut().blacklist.retain(|a| a.id != id);
        self.flush();
        self.refresh(Refresh::Blacklist);
    }

    pub fn set_time_24h(self: &Rc<Self>, on: bool) {
        self.state.borrow_mut().time_24h = on;
        self.save();
        self.refresh(Refresh::Schedule);
    }

    /// X11 master switch: widgets belong to a workspace (or to all).
    pub fn set_per_workspace(self: &Rc<Self>, on: bool) {
        self.state.borrow_mut().per_workspace = on;
        self.seen_workspace.set(crate::x11::current_desktop());
        self.flush();
        self.refresh(Refresh::Widgets);
    }

    /// The workspace a new widget should belong to (only when the master switch is on).
    pub fn new_widget_workspace(&self) -> Option<u32> {
        if self.state.borrow().per_workspace { crate::x11::current_desktop() } else { None }
    }

    pub fn set_storage(&self, storage: Storage) {
        self.state.borrow_mut().storage = storage;
        self.save();
    }

    /// Same, but coalesced: dragging a slider should not rebuild windows 60 times a second.
    pub fn commit(self: &Rc<Self>) {
        if let Some(id) = self.sync_timer.borrow_mut().take() {
            id.remove();
        }
        let this = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_millis(180), move || {
            if let Some(ctx) = this.upgrade() {
                ctx.sync_timer.borrow_mut().take();
                ctx.flush();
            }
        });
        *self.sync_timer.borrow_mut() = Some(id);
    }

    // ---------------------------------------------------------------- widgets

    pub fn edit_widget(self: &Rc<Self>, id: &str, f: impl FnOnce(&mut WidgetConfig)) {
        {
            let mut st = self.state.borrow_mut();
            if let Some(w) = st.active_preset_mut().widgets.iter_mut().find(|w| w.id == id) {
                f(w);
            }
        }
        self.commit();
    }

    /// Turn the on-screen mask handles on for widget `id`, or off for all.
    pub fn set_mask_edit(&self, id: Option<String>) {
        if *self.edit_mask.borrow() == id {
            return;
        }
        self.manager.set_mask_edit(id.as_deref());
        *self.edit_mask.borrow_mut() = id;
        self.refresh(Refresh::MaskEdit);
    }

    /// Open a widget for each picked file or folder (a folder becomes one rotating widget).
    /// Where the file lives afterwards follows the storage setting.
    pub fn import_files(self: &Rc<Self>, files: Vec<gio::File>) {
        self.import(files, false);
    }

    /// Save the picked GIFs / PNGs / WebPs as alpha masks.
    pub fn import_masks(self: &Rc<Self>, files: Vec<gio::File>) {
        self.import(files, true);
    }

    fn import(self: &Rc<Self>, files: Vec<gio::File>, masks: bool) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let storage = this.state.borrow().storage;
            let workspace = this.new_widget_workspace();
            let mut added = 0;
            for file in files {
                let Some(src) = file.path() else {
                    this.toast("That location cannot be read as a file.");
                    continue;
                };
                let name = src.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let folder = src.is_dir();
                let ok = if masks { !folder && media::is_supported(&src) } else if folder { !media::list_folder(&src).is_empty() } else { media::is_supported(&src) };
                if !ok {
                    this.toast(&match (masks, folder) {
                        (true, _) => format!("{name} is not a GIF, PNG or WebP image."),
                        (false, true) => format!("{name} has no GIF, PNG or WebP files."),
                        (false, false) => format!("{name} is not a GIF, PNG or WebP image."),
                    });
                    continue;
                }
                let kind = if masks { "Masks" } else if folder { "Widgets" } else { media::kind_dir(&src) };
                let s = src.clone();
                let stored = gio::spawn_blocking(move || store(&s, kind, storage)).await;
                let Ok(Ok(dest)) = stored else {
                    this.toast(&format!("Could not import {name}."));
                    continue;
                };
                let mut st = this.state.borrow_mut();
                if masks {
                    let name = dest.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    st.masks.push(Mask { id: model::new_id(), name, path: dest });
                } else {
                    let preset = st.active_preset_mut();
                    let slot = preset.widgets.len();
                    // New widgets start in front of the others.
                    let mut cfg = WidgetConfig::new(dest, slot);
                    cfg.workspace = workspace;
                    preset.widgets.insert(0, cfg);
                }
                added += 1;
            }
            if added > 0 {
                this.flush();
                this.refresh(if masks { Refresh::Masks } else { Refresh::Widgets });
            }
        });
    }

    pub fn remove_mask(self: &Rc<Self>, id: &str) {
        {
            let mut st = self.state.borrow_mut();
            let Some(path) = st.masks.iter().find(|m| m.id == id).map(|m| m.path.clone()) else { return };
            st.masks.retain(|m| m.id != id);
            for w in st.presets.iter_mut().flat_map(|p| p.widgets.iter_mut()) {
                if w.mask.as_ref() == Some(&path) {
                    w.mask = None;
                }
            }
        }
        self.gc_media();
        self.flush();
        self.refresh(Refresh::Masks);
        self.refresh(Refresh::Widgets);
    }

    pub fn remove_widget(self: &Rc<Self>, id: &str) {
        self.state.borrow_mut().active_preset_mut().widgets.retain(|w| w.id != id);
        self.gc_media();
        self.flush();
        self.refresh(Refresh::Widgets);
    }

    /// `delta` -1 = one step toward the front, +1 = one step back.
    pub fn move_widget(self: &Rc<Self>, id: &str, delta: i32) {
        {
            let mut st = self.state.borrow_mut();
            let list = &mut st.active_preset_mut().widgets;
            if let Some(i) = list.iter().position(|w| w.id == id) {
                let j = (i as i32 + delta).clamp(0, list.len() as i32 - 1) as usize;
                list.swap(i, j);
            }
        }
        self.flush();
        self.refresh(Refresh::Widgets);
    }

    /// The user closed a decorated widget window with its close button.
    fn widget_closed(self: &Rc<Self>, id: &str) {
        self.manager.forget(id);
        if let Some(w) = self.state.borrow_mut().active_preset_mut().widgets.iter_mut().find(|w| w.id == id) {
            w.visible = false;
        }
        self.save();
        self.refresh(Refresh::Widgets);
    }

    /// A layer-shell widget was dragged to a new place.
    fn widget_dragged(self: &Rc<Self>, id: &str, x: i32, y: i32) {
        if let Some(w) = self.state.borrow_mut().active_preset_mut().widgets.iter_mut().find(|w| w.id == id) {
            w.x = x;
            w.y = y;
        }
        self.manager.note_position(id, x, y);
        self.save();
        self.refresh(Refresh::Positions);
    }

    /// X11: pick up moves done through the window manager.
    pub fn poll_positions(self: &Rc<Self>) {
        let moved = self.manager.poll_positions();
        if moved.is_empty() {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            for (id, monitor, x, y) in moved {
                if let Some(w) = st.active_preset_mut().widgets.iter_mut().find(|w| w.id == id) {
                    w.monitor = monitor;
                    w.x = x;
                    w.y = y;
                }
            }
        }
        self.save();
        self.refresh(Refresh::Positions);
    }

    /// Delete imported files that no preset uses any more.
    fn gc_media(&self) {
        let dir = AppState::media_dir();
        let st = self.state.borrow();
        let used: Vec<PathBuf> = st.referenced_media().iter().map(|p| p.to_path_buf()).collect();
        if let Ok(entries) = fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file() && !used.iter().any(|u| Path::new(u) == p) {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }

    // ---------------------------------------------------------------- presets

    pub fn activate_preset(self: &Rc<Self>, id: &str) {
        {
            let mut st = self.state.borrow_mut();
            if st.preset(id).is_none() || st.active == id {
                return;
            }
            st.active = id.to_string();
        }
        self.flush();
        self.refresh(Refresh::All);
    }

    pub fn new_preset(self: &Rc<Self>) {
        let n = self.state.borrow().presets.len() + 1;
        let preset = Preset { name: format!("Preset {n}"), ..Default::default() };
        let id = preset.id.clone();
        self.state.borrow_mut().presets.push(preset);
        self.activate_preset(&id);
        self.refresh(Refresh::All);
    }

    pub fn duplicate_preset(self: &Rc<Self>, id: &str) {
        let copy = {
            let st = self.state.borrow();
            st.preset(id).map(|p| {
                let mut c = p.clone();
                c.id = model::new_id();
                c.name = format!("{} copy", p.name);
                for w in &mut c.widgets {
                    w.id = model::new_id();
                }
                c
            })
        };
        if let Some(copy) = copy {
            let new_id = copy.id.clone();
            self.state.borrow_mut().presets.push(copy);
            self.activate_preset(&new_id);
            self.refresh(Refresh::All);
        }
    }

    pub fn delete_preset(self: &Rc<Self>, id: &str) {
        {
            let mut st = self.state.borrow_mut();
            if st.presets.len() <= 1 {
                return;
            }
            st.presets.retain(|p| p.id != id);
            st.repair(); // fixes `active` and drops rules that pointed at it
        }
        self.gc_media();
        self.flush();
        self.refresh(Refresh::All);
    }

    pub fn rename_preset(&self, id: &str, name: &str) {
        if let Some(p) = self.state.borrow_mut().presets.iter_mut().find(|p| p.id == id) {
            p.name = name.to_string();
        }
        self.save();
        self.refresh(Refresh::Names);
    }

    // ---------------------------------------------------------------- schedule

    pub fn add_rule(self: &Rc<Self>) {
        let rule = Rule { preset: self.state.borrow().active.clone(), ..Default::default() };
        self.state.borrow_mut().rules.push(rule);
        self.rules_changed();
        self.refresh(Refresh::Schedule);
    }

    pub fn remove_rule(self: &Rc<Self>, id: &str) {
        self.state.borrow_mut().rules.retain(|r| r.id != id);
        self.rules_changed();
        self.refresh(Refresh::Schedule);
    }

    pub fn move_rule(self: &Rc<Self>, id: &str, delta: i32) {
        {
            let mut st = self.state.borrow_mut();
            if let Some(i) = st.rules.iter().position(|r| r.id == id) {
                let j = (i as i32 + delta).clamp(0, st.rules.len() as i32 - 1) as usize;
                st.rules.swap(i, j);
            }
        }
        self.rules_changed();
        self.refresh(Refresh::Schedule);
    }

    pub fn edit_rule(self: &Rc<Self>, id: &str, f: impl FnOnce(&mut Rule)) {
        if let Some(r) = self.state.borrow_mut().rules.iter_mut().find(|r| r.id == id) {
            f(r);
        }
        self.rules_changed();
    }

    pub fn set_schedule_enabled(self: &Rc<Self>, on: bool) {
        self.state.borrow_mut().schedule_enabled = on;
        self.rules_changed();
    }

    fn rules_changed(self: &Rc<Self>) {
        self.save();
        self.refresh(Refresh::Status); // conflict markers follow every edit
        *self.last_scheduled.borrow_mut() = None; // re-evaluate against the new rules
        self.tick_schedule();
    }

    /// Name of the preset the rules select right now (ignores the on/off switch).
    pub fn scheduled_now(&self) -> Option<String> {
        let (day, minute) = model::now_slot();
        let st = self.state.borrow();
        let rule = model::evaluate(&st.rules, &self.wins.borrow(), day, minute)?;
        st.preset(&rule.preset).map(|p| p.name.clone())
    }

    /// Called every few seconds. Applies the scheduled preset when the
    /// schedule's answer changes (not on every tick, so manual choices stick).
    pub fn tick_schedule(self: &Rc<Self>) {
        let target = {
            let st = self.state.borrow();
            if !st.schedule_enabled {
                *self.last_scheduled.borrow_mut() = None;
                drop(st);
                self.refresh(Refresh::Status);
                return;
            }
            let (day, minute) = model::now_slot();
            model::evaluate(&st.rules, &self.wins.borrow(), day, minute).map(|r| (r.preset.clone(), r.only_if_open))
        };
        if matches!(target, Some((_, false))) {
            *self.app_return.borrow_mut() = None; // a plain schedule rule is in charge again
        }
        let target_id = target.as_ref().map(|t| t.0.clone());
        let changed = self.last_scheduled.borrow().as_ref() != Some(&target_id);
        if changed {
            *self.last_scheduled.borrow_mut() = Some(target_id.clone());
            match target {
                Some((id, gated)) => {
                    if gated && self.app_return.borrow().is_none() {
                        *self.app_return.borrow_mut() = Some(self.state.borrow().active.clone());
                    }
                    self.activate_preset(&id);
                }
                None => {
                    // The application closed: go back to what was showing before it opened.
                    let back = self.app_return.borrow_mut().take();
                    if let Some(id) = back {
                        self.activate_preset(&id);
                    }
                }
            }
            self.refresh(Refresh::Status);
        }
    }
}
