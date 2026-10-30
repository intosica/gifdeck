//! The widget manager: owns every widget of the (single) process.
//!
//! `sync()` is the only entry point that changes anything. It reconciles what
//! exists with the list of widgets the active preset wants, so applying a
//! preset, editing a setting or reordering all go through the same code path.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk4_layer_shell::Layer;

use crate::env::{Backend, Env, Role};
use crate::layer_host::LayerHost;
use crate::media;
use crate::model::{self, Level, MaskXf, RotMode, Rotation, WidgetConfig};
use crate::monitors;
use crate::widget::{Backing, ClosedCb, MovedCb, WidgetWindow, Workspace};

/// Which file of a folder widget is showing, and when it changes.
struct Rot {
    /// What this state was made for; a different folder or rotation starts over.
    key: (PathBuf, Rotation),
    files: Vec<PathBuf>,
    current: PathBuf,
    due: Option<Instant>,
}

fn next_due(r: &Rotation) -> Option<Instant> {
    let secs = match r.mode {
        RotMode::Sequential => r.interval.max(1),
        RotMode::Random => {
            let (lo, hi) = (r.min.max(1), r.max.max(r.min.max(1)));
            glib::random_int_range(lo as i32, hi as i32 + 1) as u32
        }
        RotMode::Scheduled => return None,
    };
    Some(Instant::now() + Duration::from_secs(secs as u64))
}

pub struct Manager {
    app: adw::Application,
    env: Env,
    widgets: RefCell<HashMap<String, WidgetWindow>>,
    /// Shared layer-shell surfaces, one per layer and monitor (Wayland only), created on demand.
    hosts: RefCell<HashMap<(Level, Option<String>), Rc<LayerHost>>>,
    /// X11: widgets belong to a workspace instead of showing on all of them.
    per_workspace: Cell<bool>,
    /// What the active preset wants, front-most first.
    desired: RefCell<Vec<WidgetConfig>>,
    /// Widgets whose image is still being decoded.
    pending: RefCell<HashSet<String>>,
    /// Folder widgets' rotation state, by widget id.
    rot: RefCell<HashMap<String, Rot>>,
    on_moved: MovedCb,
    on_closed: ClosedCb,
    on_error: Rc<dyn Fn(&str, &str)>,
    /// A mask transform was dragged (id, new transform, drag finished).
    on_mask_xf: Rc<dyn Fn(&str, MaskXf)>,
    /// The user touched the widget outside its mask: leave mask editing.
    on_mask_exit: Rc<dyn Fn()>,
    /// Widget whose mask handles are showing.
    editing: RefCell<Option<String>>,
}

/// Changes that cannot be applied to an existing widget in place.
fn needs_rebuild(a: &WidgetConfig, b: &WidgetConfig) -> bool {
    a.path != b.path
        || a.scale != b.scale
        || a.decorated != b.decorated
        || a.level != b.level
        || a.click_through != b.click_through
        || a.mask != b.mask
        || a.angle != b.angle
        || a.rotation != b.rotation
        || a.snap_edges != b.snap_edges
        || a.keep_inside != b.keep_inside
}

impl Manager {
    pub fn new(app: adw::Application, env: Env, on_moved: MovedCb, on_closed: ClosedCb, on_error: Rc<dyn Fn(&str, &str)>, on_mask_xf: Rc<dyn Fn(&str, MaskXf)>, on_mask_exit: Rc<dyn Fn()>) -> Rc<Manager> {
        Rc::new(Manager {
            app,
            env,
            widgets: RefCell::new(HashMap::new()),
            hosts: RefCell::new(HashMap::new()),
            per_workspace: Cell::new(false),
            desired: RefCell::new(Vec::new()),
            pending: RefCell::new(HashSet::new()),
            rot: RefCell::new(HashMap::new()),
            on_moved,
            on_closed,
            on_error,
            on_mask_xf,
            on_mask_exit,
            editing: RefCell::new(None),
        })
    }

    /// Show the mask handles on widget `id` only (or none).
    pub fn set_mask_edit(&self, id: Option<&str>) {
        *self.editing.borrow_mut() = id.map(str::to_string);
        for (wid, w) in self.widgets.borrow().iter() {
            if let Some(v) = w.view() {
                v.set_edit(id == Some(wid.as_str()));
            }
        }
    }

    /// Connect a freshly built / re-masked widget's view to the editor.
    fn wire(&self, id: &str) {
        let ws = self.widgets.borrow();
        let Some(w) = ws.get(id) else { return };
        let Some(v) = w.view() else { return };
        let (cb, exit, wid) = (self.on_mask_xf.clone(), self.on_mask_exit.clone(), id.to_string());
        v.set_hooks(Rc::new(move |xf, _| cb(&wid, xf)), exit);
        v.set_mask_look(w.cfg.mask_opacity, w.cfg.mask_xf);
        v.set_edit(self.editing.borrow().as_deref() == Some(id));
    }

    /// The user closed a decorated widget window with its close button: just drop our handle.
    pub fn forget(&self, id: &str) {
        self.widgets.borrow_mut().remove(id);
    }

    /// A hosted widget was dragged: remember where it is so `sync()` sees no difference.
    pub fn note_position(&self, id: &str, x: i32, y: i32) {
        if let Some(w) = self.widgets.borrow_mut().get_mut(id) {
            w.cfg.x = x;
            w.cfg.y = y;
        }
    }

    fn close_widget(&self, id: &str) {
        let Some(w) = self.widgets.borrow_mut().remove(id) else { return };
        // A host that became empty stays alive (its input region is empty, so it is click-through):
        // destroying and re-creating layer-shell surfaces on every preset switch is what made
        // rapid switching unstable. `hosts` is bounded by the number of levels.
        w.close();
    }

    fn host_for(&self, level: Level, layer: Layer, monitor: Option<&str>) -> Rc<LayerHost> {
        let key = (level, monitor.map(str::to_string));
        if let Some(h) = self.hosts.borrow().get(&key) {
            return h.clone();
        }
        let host = LayerHost::new(&self.app, layer, self.on_moved.clone());
        if let Some(m) = monitor.and_then(monitors::find) {
            host.set_monitor(&m);
        }
        self.hosts.borrow_mut().insert(key, host.clone());
        host
    }

    /// Where an X11 window belongs with respect to workspaces.
    fn workspace(&self, cfg: &WidgetConfig) -> Workspace {
        if self.per_workspace.get() {
            match cfg.workspace {
                Some(d) => (false, Some(d.min(crate::x11::desktop_count().unwrap_or(1).saturating_sub(1)))),
                None => (true, None),
            }
        } else {
            (cfg.level != Level::Normal, None)
        }
    }

    pub fn set_per_workspace(&self, on: bool) {
        self.per_workspace.set(on);
    }

    /// Make the widgets match `widgets` (front-most first).
    pub fn sync(self: &Rc<Self>, widgets: &[WidgetConfig]) {
        let order = |list: &[WidgetConfig]| list.iter().filter(|w| w.visible).map(|w| w.id.clone()).collect::<Vec<_>>();
        let order_changed = order(&self.desired.borrow()) != order(widgets);
        *self.desired.borrow_mut() = widgets.to_vec();
        self.rot.borrow_mut().retain(|id, _| widgets.iter().any(|w| &w.id == id));
        let wanted: HashSet<&str> = widgets.iter().filter(|w| w.visible).map(|w| w.id.as_str()).collect();

        let stale: Vec<String> = self.widgets.borrow().keys().filter(|id| !wanted.contains(id.as_str())).cloned().collect();
        for id in stale {
            self.close_widget(&id);
        }

        let mut created = false;
        for cfg in widgets.iter().filter(|w| w.visible) {
            let existing = self.widgets.borrow().get(&cfg.id).map(|w| w.cfg.clone());
            match existing {
                Some(old) if needs_rebuild(&old, cfg) || (self.env.layer_shell && old.monitor != cfg.monitor) => {
                    self.close_widget(&cfg.id);
                    self.spawn_build(cfg.id.clone());
                    created = true;
                }
                Some(_) => {
                    if let Some(w) = self.widgets.borrow_mut().get_mut(&cfg.id) {
                        w.apply_live(&self.env, cfg, self.workspace(cfg));
                    }
                }
                None => {
                    self.spawn_build(cfg.id.clone());
                    created = true;
                }
            }
        }
        // Newly built widgets restack themselves when they appear.
        if !created && order_changed {
            self.restack(None);
        }
    }

    /// The file a widget shows right now: its own path, or the current file of its folder.
    fn current_file(&self, cfg: &WidgetConfig) -> Option<PathBuf> {
        if !cfg.is_folder() {
            return Some(cfg.path.clone());
        }
        let mut rot = self.rot.borrow_mut();
        let key = (cfg.path.clone(), cfg.rotation.clone());
        if let Some(r) = rot.get(&cfg.id).filter(|r| r.key == key) {
            return Some(r.current.clone());
        }
        let files = media::list_folder(&cfg.path);
        let current = match cfg.rotation.mode {
            RotMode::Scheduled => cfg.rotation.scheduled_file(&files, model::now_slot().1).or(files.first()),
            _ => files.first(),
        }?
        .clone();
        rot.insert(cfg.id.clone(), Rot { key, files, current: current.clone(), due: next_due(&cfg.rotation) });
        Some(current)
    }

    /// Decode (async) and open widget `id`, using whatever the *latest* desired
    /// configuration is by the time decoding has finished.
    fn spawn_build(self: &Rc<Self>, id: String) {
        if !self.pending.borrow_mut().insert(id.clone()) {
            return;
        }
        let Some(first) = self.desired.borrow().iter().find(|w| w.id == id).cloned() else {
            self.pending.borrow_mut().remove(&id);
            return;
        };
        let Some(path) = self.current_file(&first) else {
            self.pending.borrow_mut().remove(&id);
            (self.on_error)(&first.title(), "the folder has no supported image files");
            return;
        };
        let this = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = media::load(path.clone()).await;
            let mask = match &first.mask {
                Some(m) => media::load(m.clone()).await.ok(),
                None => None,
            };
            let Some(this) = this.upgrade() else { return };
            this.pending.borrow_mut().remove(&id);
            let cfg = this.desired.borrow().iter().find(|w| w.id == id && w.visible).cloned();
            let Some(cfg) = cfg else { return };
            // The configuration moved on while we were decoding (another preset, a new file or mask):
            // what we decoded is stale, start over with the latest.
            if cfg.mask != first.mask || this.current_file(&cfg).as_ref() != Some(&path) {
                if !this.widgets.borrow().contains_key(&id) {
                    this.spawn_build(id);
                }
                return;
            }
            match result {
                Ok(anim) => {
                    if this.widgets.borrow().contains_key(&id) {
                        return;
                    }
                    let widget = match this.env.role(&cfg) {
                        Role::Layer(layer) => {
                            let host = this.host_for(cfg.level, layer, cfg.monitor.as_deref());
                            WidgetWindow::build_hosted(&host, &cfg, &anim, mask)
                        }
                        Role::Toplevel => WidgetWindow::build_window(&this.app, &this.env, &cfg, &anim, mask, this.workspace(&cfg), this.on_closed.clone()),
                    };
                    this.widgets.borrow_mut().insert(id.clone(), widget);
                    this.wire(&id);
                    this.restack(Some(&id));
                }
                Err(msg) => (this.on_error)(&cfg.title(), &msg),
            }
        });
    }

    /// Called every second: advance folder widgets whose time has come.
    pub fn tick_rotation(self: &Rc<Self>) {
        let now = Instant::now();
        let minute = model::now_slot().1;
        let mut switches: Vec<(String, PathBuf)> = Vec::new();
        {
            let widgets = self.widgets.borrow();
            let mut rot = self.rot.borrow_mut();
            for (id, w) in widgets.iter() {
                let Some(r) = rot.get_mut(id) else { continue };
                let rotation = &w.cfg.rotation;
                let target = match rotation.mode {
                    RotMode::Scheduled => rotation.scheduled_file(&r.files, minute).cloned(),
                    _ if r.due.is_some_and(|d| d <= now) => {
                        r.files = media::list_folder(&w.cfg.path); // pick up files added meanwhile
                        r.due = next_due(rotation);
                        let n = r.files.len();
                        let at = r.files.iter().position(|f| *f == r.current);
                        let next = match (rotation.mode, at) {
                            (RotMode::Random, _) if n > 1 => {
                                // anything but the current file
                                let skip = at.map(|a| a as i32).unwrap_or(-1);
                                let mut i = glib::random_int_range(0, n as i32 - 1);
                                if i >= skip && skip >= 0 {
                                    i += 1;
                                }
                                Some(i as usize)
                            }
                            (_, Some(a)) if n > 0 => Some((a + 1) % n),
                            _ if n > 0 => Some(0),
                            _ => None,
                        };
                        next.map(|i| r.files[i].clone())
                    }
                    _ => None,
                };
                if let Some(t) = target.filter(|t| *t != r.current) {
                    r.current = t.clone();
                    switches.push((id.clone(), t));
                }
            }
        }
        for (id, path) in switches {
            let this = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let Ok(anim) = media::load(path).await else { return };
                let Some(this) = this.upgrade() else { return };
                let mask = match this.widgets.borrow().get(&id).and_then(|w| w.cfg.mask.clone()) {
                    Some(m) => media::load(m).await.ok(),
                    None => None,
                };
                if let Some(w) = this.widgets.borrow().get(&id) {
                    w.set_media(&anim, mask);
                }
                this.wire(&id);
            });
        }
    }

    /// Enforce the user's priority order (index 0 = front) inside each layer.
    ///
    /// * Hosted (Wayland layer-shell): the host stacks its children in exactly this order.
    /// * X11 windows: raise back-to-front; the window manager keeps each `ABOVE` / `BELOW` group intact.
    /// * Plain Wayland toplevels cannot be ordered by clients.
    pub fn restack(&self, changed: Option<&str>) {
        let desired = self.desired.borrow();
        let widgets = self.widgets.borrow();
        let priority = |id: &str| desired.iter().position(|w| w.id == id).unwrap_or(usize::MAX);
        let changed_prio = changed.map(priority);

        // Hosts: order is deterministic, so just apply it.
        for host in self.hosts.borrow().values() {
            let mut ids: Vec<&String> = widgets.iter().filter(|(_, w)| w.host().is_some_and(|h| Rc::ptr_eq(h, host))).map(|(id, _)| id).collect();
            ids.sort_by_key(|id| std::cmp::Reverse(priority(id)));
            host.reorder(&ids.into_iter().cloned().collect::<Vec<_>>());
        }

        if self.env.backend == Backend::X11 {
            for level in [Level::Desktop, Level::Normal, Level::Top] {
                let mut group: Vec<(&String, &WidgetWindow)> = widgets.iter().filter(|(_, w)| w.cfg.level == level).collect();
                group.sort_by_key(|(id, _)| std::cmp::Reverse(priority(id))); // back ... front
                // `Desktop` and `Top` live in their own EWMH layer, so raising them cannot cover
                // the user's applications. Ordinary-layer widgets are only raised when they must be
                // in front of a widget that just appeared (itself on top at that moment), or on a reorder.
                for (id, w) in &group {
                    let eligible = level != Level::Normal || changed_prio.map(|c| priority(id) < c).unwrap_or(true);
                    if eligible {
                        w.raise_x11();
                    }
                }
            }
        }
    }

    /// X11: widgets the user moved with the window manager. Returns (id, monitor, x, y) for
    /// changed ones, with x/y relative to the monitor the widget is on.
    pub fn poll_positions(&self) -> Vec<(String, Option<String>, i32, i32)> {
        if self.env.backend != Backend::X11 {
            return Vec::new();
        }
        let mut moved = Vec::new();
        for (id, w) in self.widgets.borrow_mut().iter_mut() {
            let Some(pos) = w.x11_position() else { continue };
            let at = monitors::to_global(w.cfg.monitor.as_deref(), w.cfg.x, w.cfg.y);
            let global = w.x11_constrain(pos);
            if global != pos {
                if let Backing::Window(win) = &w.backing {
                    if let Some(xid) = crate::x11::xid_of(win) {
                        crate::x11::move_to(xid, global.0, global.1);
                    }
                }
            }
            // Unchanged since we put it there: keep the stored monitor (it may just be unplugged).
            if global != at {
                let (mon, x, y) = monitors::from_global(global.0, global.1);
                w.cfg.monitor = mon.clone();
                w.cfg.x = x;
                w.cfg.y = y;
                moved.push((id.clone(), mon, x, y));
            }
        }
        moved
    }
}
