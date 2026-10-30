//! "Widgets" page: active preset, and the ordered list of widgets with their settings.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::ctx::AppCtx;
use crate::media;
use crate::model::{AppMode, Level, RotMode, SchedEntry, WidgetConfig};
use crate::monitors;
use super::app_picker;

/// Keeps clicks on the mask controls from leaving mask-edit mode (see `Ui::new`).
pub const XF_KEEP: &str = "gifdeck-xf-keep";

struct XfRows {
    id: String,
    edit: adw::SwitchRow,
    spins: Vec<(adw::SpinRow, fn(&WidgetConfig) -> f64)>,
}

type XfSpec = (&'static str, f64, f64, f64, fn(&WidgetConfig) -> f64, fn(&mut WidgetConfig, f64));

/// Mask rows in display units (percent / degrees).
const XF_SPECS: [XfSpec; 10] = [
    ("Mask opacity (%) — 0 only clips", 0.0, 100.0, 5.0, |c| c.mask_opacity * 100.0, |c, v| c.mask_opacity = v / 100.0),
    ("Mask size (%)", 2.0, 2000.0, 5.0, |c| c.mask_xf.size * 100.0, |c, v| c.mask_xf.size = v / 100.0),
    ("Mask stretch width (%)", 2.0, 2000.0, 5.0, |c| c.mask_xf.sx * 100.0, |c, v| c.mask_xf.sx = v / 100.0),
    ("Mask stretch height (%)", 2.0, 2000.0, 5.0, |c| c.mask_xf.sy * 100.0, |c, v| c.mask_xf.sy = v / 100.0),
    ("Mask skew X (°)", -75.0, 75.0, 1.0, |c| c.mask_xf.skew_x, |c, v| c.mask_xf.skew_x = v),
    ("Mask skew Y (°)", -75.0, 75.0, 1.0, |c| c.mask_xf.skew_y, |c, v| c.mask_xf.skew_y = v),
    ("Mask rotation (°)", -180.0, 180.0, 5.0, |c| c.mask_xf.rot, |c, v| c.mask_xf.rot = v),
    ("Mask hue (°)", -180.0, 180.0, 5.0, |c| c.mask_xf.hue, |c, v| c.mask_xf.hue = v),
    ("Mask contrast (%)", 0.0, 300.0, 5.0, |c| c.mask_xf.contrast * 100.0, |c, v| c.mask_xf.contrast = v / 100.0),
    ("Mask brightness (%)", -100.0, 100.0, 5.0, |c| c.mask_xf.brightness * 100.0, |c, v| c.mask_xf.brightness = v / 100.0),
];

pub struct WidgetsPage {
    ctx: Rc<AppCtx>,
    pub root: gtk::ScrolledWindow,
    preset_row: adw::ComboRow,
    list: gtk::ListBox,
    expanded: RefCell<HashSet<String>>,
    /// (widget id, x row, y row) so drags can update the numbers in place.
    pos_rows: RefCell<Vec<(String, adw::SpinRow, adw::SpinRow)>>,
    /// Mask transform controls of each widget, so dragging on screen can update them in place.
    xf_rows: RefCell<Vec<XfRows>>,
    /// Set while the UI itself changes controls, so handlers ignore those changes.
    updating: Rc<Cell<bool>>,
}

fn summary(cfg: &WidgetConfig) -> String {
    let mut s = format!("{} · {}%", cfg.level.label(), (cfg.scale * 100.0).round() as i32);
    if cfg.is_folder() {
        s.push_str(&format!(" · folder, {} files", media::list_folder(&cfg.path).len()));
    }
    if !cfg.visible {
        s.push_str(" · hidden");
    }
    s
}

type Edit = Rc<dyn Fn(Box<dyn FnOnce(&mut WidgetConfig)>)>;

impl WidgetsPage {
    pub fn new(ctx: &Rc<AppCtx>) -> Rc<WidgetsPage> {
        let preset_row = adw::ComboRow::builder().title("Preset").subtitle("Edits below apply to this preset").build();
        let preset_group = adw::PreferencesGroup::builder().title("Active preset").build();
        preset_group.add(&preset_row);

        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        let hint = gtk::Label::builder()
            .label("No widgets yet.\nPress Add to pick a GIF, PNG or WebP, Folder to use a whole\nfolder as one rotating widget, or drop files here.")
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .margin_top(24)
            .margin_bottom(24)
            .build();
        list.set_placeholder(Some(&hint));

        let add = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label("Add").build())
            .css_classes(["suggested-action"])
            .build();

        let folder_btn = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().icon_name("folder-symbolic").label("Folder").build())
            .build();
        let btns = gtk::Box::builder().spacing(6).build();
        btns.append(&folder_btn);
        btns.append(&add);

        let group = adw::PreferencesGroup::builder()
            .title("Widgets")
            .description("Top of the list is in front.")
            .header_suffix(&btns)
            .build();
        group.add(&list);

        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).margin_top(12).margin_bottom(18).margin_start(12).margin_end(12).build();
        content.append(&preset_group);
        content.append(&group);
        let clamp = adw::Clamp::builder().maximum_size(640).child(&content).build();
        let root = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).build();

        let page = Rc::new(WidgetsPage {
            ctx: ctx.clone(),
            root,
            preset_row,
            list,
            expanded: RefCell::new(HashSet::new()),
            pos_rows: RefCell::new(Vec::new()),
            xf_rows: RefCell::new(Vec::new()),
            updating: Rc::new(Cell::new(false)),
        });

        {
            let ctx = ctx.clone();
            let updating = page.updating.clone();
            page.preset_row.connect_selected_notify(move |row| {
                if updating.get() {
                    return;
                }
                let id = ctx.state.borrow().presets.get(row.selected() as usize).map(|p| p.id.clone());
                if let Some(id) = id {
                    ctx.activate_preset(&id);
                }
            });
        }
        {
            let ctx = ctx.clone();
            add.connect_clicked(move |btn| {
                let filter = gtk::FileFilter::new();
                filter.set_name(Some("GIF, PNG and WebP images"));
                filter.add_mime_type("image/gif");
                filter.add_mime_type("image/png");
                filter.add_mime_type("image/webp");
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                let dialog = gtk::FileDialog::builder().title("Add GIF, PNG or WebP").modal(true).filters(&filters).default_filter(&filter).build();
                let parent = btn.root().and_downcast::<gtk::Window>();
                let ctx = ctx.clone();
                dialog.open_multiple(parent.as_ref(), gio::Cancellable::NONE, move |res| {
                    if let Ok(files) = res {
                        let files: Vec<gio::File> = files.iter::<gio::File>().flatten().collect();
                        ctx.import_files(files);
                    }
                });
            });
        }

        {
            let ctx = ctx.clone();
            folder_btn.connect_clicked(move |btn| {
                let dialog = gtk::FileDialog::builder().title("Choose a folder of GIFs / PNGs / WebPs").modal(true).build();
                let parent = btn.root().and_downcast::<gtk::Window>();
                let ctx = ctx.clone();
                dialog.select_folder(parent.as_ref(), gio::Cancellable::NONE, move |res| {
                    if let Ok(f) = res {
                        ctx.import_files(vec![f]);
                    }
                });
            });
        }

        page.update_presets();
        page.rebuild();
        page
    }

    /// Preset names / selection (also after a rename).
    pub fn update_presets(&self) {
        let st = self.ctx.state.borrow();
        let names: Vec<&str> = st.presets.iter().map(|p| p.name.as_str()).collect();
        let selected = st.presets.iter().position(|p| p.id == st.active).unwrap_or(0) as u32;
        self.updating.set(true);
        self.preset_row.set_model(Some(&gtk::StringList::new(&names)));
        self.preset_row.set_selected(selected);
        self.updating.set(false);
    }

    /// Rebuild the list (add / remove / reorder / preset switch).
    pub fn rebuild(self: &Rc<Self>) {
        self.updating.set(true);
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.pos_rows.borrow_mut().clear();
        self.xf_rows.borrow_mut().clear();
        let widgets = self.ctx.state.borrow().active_preset().widgets.clone();
        let total = widgets.len();
        for (i, cfg) in widgets.iter().enumerate() {
            self.list.append(&self.build_row(cfg, i, total));
        }
        self.apply_workspace_filter();
        self.updating.set(false);
    }

    /// X11 "per workspace" mode: list only the widgets of the workspace being looked at.
    /// Hidden entirely (not greyed out), so the page stays short.
    pub fn apply_workspace_filter(&self) {
        let (on, now, widgets) = {
            let st = self.ctx.state.borrow();
            (st.per_workspace && self.ctx.env.backend == crate::env::Backend::X11, crate::x11::current_desktop(), st.active_preset().widgets.clone())
        };
        let mut child = self.list.first_child();
        for cfg in &widgets {
            let Some(row) = child else { break };
            child = row.next_sibling();
            row.set_visible(!on || cfg.workspace.is_none() || cfg.workspace == now);
        }
    }

    /// x/y changed from outside (drag, window manager).
    pub fn update_positions(&self) {
        self.updating.set(true);
        let st = self.ctx.state.borrow();
        for (id, xr, yr) in self.pos_rows.borrow().iter() {
            if let Some(w) = st.active_preset().widgets.iter().find(|w| &w.id == id) {
                xr.set_value(w.x as f64);
                yr.set_value(w.y as f64);
            }
        }
        self.updating.set(false);
    }

    /// Mask transform changed on screen (or reset): update the numbers.
    pub fn update_mask_xf(&self) {
        self.updating.set(true);
        let st = self.ctx.state.borrow();
        for r in self.xf_rows.borrow().iter() {
            if let Some(w) = st.active_preset().widgets.iter().find(|w| w.id == r.id) {
                for (row, get) in &r.spins {
                    row.set_value(get(w).round());
                }
            }
        }
        self.updating.set(false);
    }

    /// Mask-edit switches follow `ctx.edit_mask`.
    pub fn update_mask_edit(&self) {
        self.updating.set(true);
        let on = self.ctx.edit_mask.borrow().clone();
        for r in self.xf_rows.borrow().iter() {
            r.edit.set_active(on.as_deref() == Some(r.id.as_str()));
        }
        self.updating.set(false);
    }

    fn build_row(self: &Rc<Self>, cfg: &WidgetConfig, idx: usize, total: usize) -> adw::ExpanderRow {
        let ctx = self.ctx.clone();
        let id = cfg.id.clone();
        let updating = self.updating.clone();

        let row = adw::ExpanderRow::builder()
            .title(glib::markup_escape_text(&cfg.title()))
            .subtitle(glib::markup_escape_text(&summary(cfg)))
            .expanded(self.expanded.borrow().contains(&id))
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("image-x-generic-symbolic"));
        {
            let this = Rc::downgrade(self);
            let id = id.clone();
            row.connect_expanded_notify(move |r| {
                if let Some(this) = this.upgrade() {
                    if r.is_expanded() {
                        this.expanded.borrow_mut().insert(id.clone());
                    } else {
                        this.expanded.borrow_mut().remove(&id);
                    }
                }
            });
        }

        // ---- header buttons
        let visible = gtk::Switch::builder().active(cfg.visible).valign(gtk::Align::Center).tooltip_text("Show this widget").build();
        let icon_button = |icon: &str, tip: &str| gtk::Button::builder().icon_name(icon).tooltip_text(tip).valign(gtk::Align::Center).css_classes(["flat"]).build();
        let up = icon_button("go-up-symbolic", "Move toward the front");
        let down = icon_button("go-down-symbolic", "Move toward the back");
        let remove = icon_button("user-trash-symbolic", "Remove widget");
        up.set_sensitive(idx > 0);
        down.set_sensitive(idx + 1 < total);
        for w in [&remove.clone().upcast::<gtk::Widget>(), &down.clone().upcast(), &up.clone().upcast(), &visible.clone().upcast()] {
            row.add_suffix(w);
        }
        {
            let (ctx, id) = (ctx.clone(), id.clone());
            up.connect_clicked(move |_| ctx.move_widget(&id, -1));
        }
        {
            let (ctx, id) = (ctx.clone(), id.clone());
            down.connect_clicked(move |_| ctx.move_widget(&id, 1));
        }
        {
            let (ctx, id) = (ctx.clone(), id.clone());
            remove.connect_clicked(move |_| ctx.remove_widget(&id));
        }

        // ---- settings
        let decorated = adw::SwitchRow::builder().title("Window decorations").subtitle("Show the title bar and border").active(cfg.decorated).build();
        let labels: Vec<&str> = Level::ALL.iter().map(|l| l.label()).collect();
        let layer = adw::ComboRow::builder().title("Layer").model(&gtk::StringList::new(&labels)).selected(cfg.level.index()).build();
        let note = adw::ActionRow::builder().title("Note").css_classes(["warning"]).build();
        note.set_subtitle_lines(0);
        let angle = adw::SpinRow::with_range(-180.0, 180.0, 5.0);
        angle.set_title("Rotation (°)");
        angle.set_value(cfg.angle.round());
        let scale = adw::SpinRow::with_range(10.0, 400.0, 5.0);
        scale.set_title("Size (%)");
        scale.set_value((cfg.scale * 100.0).round());
        let opacity = adw::SpinRow::with_range(10.0, 100.0, 5.0);
        opacity.set_title("Opacity (%)");
        opacity.set_value((cfg.opacity * 100.0).round());
        let click = adw::SwitchRow::builder().title("Click-through").subtitle("Mouse clicks pass through to what is behind. Turn off to move the widget.").active(cfg.click_through).build();
        let x = adw::SpinRow::with_range(-4000.0, 20000.0, 1.0);
        x.set_title("Position X");
        x.set_value(cfg.x as f64);
        let y = adw::SpinRow::with_range(-4000.0, 20000.0, 1.0);
        y.set_title("Position Y");
        y.set_value(cfg.y as f64);
        // Monitor: "Automatic" or one of the connected outputs (a missing one stays selectable, so it is not forgotten).
        let monitor_list = monitors::list();
        let mut monitor_names: Vec<String> = monitor_list.iter().map(|m| m.0.clone()).collect();
        if let Some(m) = cfg.monitor.as_ref().filter(|m| !monitor_names.contains(m)) {
            monitor_names.push(m.clone());
        }
        let monitor_labels: Vec<String> = std::iter::once("Automatic".to_string())
            .chain(monitor_names.iter().map(|n| if monitor_list.iter().any(|m| &m.0 == n) { n.clone() } else { format!("{n} (not connected)") }))
            .collect();
        let monitor_labels: Vec<&str> = monitor_labels.iter().map(String::as_str).collect();
        let monitor_sel = cfg.monitor.as_ref().and_then(|m| monitor_names.iter().position(|n| n == m)).map(|i| i as u32 + 1).unwrap_or(0);
        let monitor = adw::ComboRow::builder()
            .title("Monitor")
            .subtitle("The screen the widget stays on; position is measured from its top-left corner")
            .model(&gtk::StringList::new(&monitor_labels))
            .selected(monitor_sel)
            .build();

        // Bound to workspace (X11): applies to every layer.
        let x11_ws = ctx.env.backend == crate::env::Backend::X11;
        let ws_count = crate::x11::desktop_count().unwrap_or(1).max(cfg.workspace.map_or(1, |w| w + 1));
        let ws_labels: Vec<String> = std::iter::once("All workspaces".to_string()).chain((1..=ws_count).map(|i| format!("Workspace {i}"))).collect();
        let ws_labels: Vec<&str> = ws_labels.iter().map(String::as_str).collect();
        let workspace = adw::ComboRow::builder()
            .title("Bound to workspace")
            .subtitle("Show this widget only on one workspace, whatever its layer")
            .model(&gtk::StringList::new(&ws_labels))
            .selected(cfg.workspace.map_or(0, |w| w + 1))
            .visible(x11_ws)
            .build();

        let snap = adw::SwitchRow::builder().title("Snap to screen edges").subtitle("While dragging, stick to an edge when close").active(cfg.snap_edges).build();
        let keep = adw::SwitchRow::builder().title("Keep inside the screen").subtitle("While dragging, never push the widget past the edges").active(cfg.keep_inside).build();

        // Alpha mask: pick any saved mask (or none) from the dropdown.
        let masks = ctx.state.borrow().masks.clone();
        let mut mask_names = vec!["None".to_string()];
        mask_names.extend(masks.iter().map(|m| m.name.clone()));
        let mask_names: Vec<&str> = mask_names.iter().map(String::as_str).collect();
        let mask_sel = cfg.mask.as_ref().and_then(|p| masks.iter().position(|m| &m.path == p)).map(|i| i as u32 + 1).unwrap_or(0);
        let mask = adw::ComboRow::builder().title("Alpha mask").subtitle("Clip the widget to a saved shape (Masks tab)").model(&gtk::StringList::new(&mask_names)).selected(mask_sel).build();

        // Mask look + on-screen transform; only useful with a mask chosen.
        let edit_sw = adw::SwitchRow::builder().title("Transform mask on screen").subtitle("Drag handles on the widget; turns off when you click anything else").active(self.ctx.edit_mask.borrow().as_deref() == Some(id.as_str())).build();
        let xf_reset = gtk::Button::builder().label("Reset").valign(gtk::Align::Center).tooltip_text("Back to the default mask placement and colours").build();
        edit_sw.add_suffix(&xf_reset);
        let xf_spins: Vec<(adw::SpinRow, XfSpec)> = XF_SPECS
            .iter()
            .map(|sp| {
                let r = adw::SpinRow::with_range(sp.1, sp.2, sp.3);
                r.set_title(sp.0);
                r.set_value(sp.4(cfg).round());
                (r, *sp)
            })
            .collect();
        let mask_rows: Vec<gtk::Widget> = std::iter::once(edit_sw.clone().upcast()).chain(xf_spins.iter().map(|(r, _)| r.clone().upcast())).collect();
        for r in &mask_rows {
            r.add_css_class(XF_KEEP);
            r.set_sensitive(cfg.mask.is_some());
        }
        self.xf_rows.borrow_mut().push(XfRows { id: id.clone(), edit: edit_sw.clone(), spins: xf_spins.iter().map(|(r, sp)| (r.clone(), sp.4)).collect() });

        // Application rule.
        let app_mode = adw::ComboRow::builder()
            .title("Application rule")
            .subtitle(if ctx.env.backend == crate::env::Backend::X11 { "React to another application's window" } else { "Needs an X11 session: other windows cannot be seen on Wayland" })
            .model(&gtk::StringList::new(&AppMode::ALL.map(|m| m.label())))
            .selected(cfg.app_mode.index())
            .build();
        let app_row = adw::ActionRow::builder().title("Application").subtitle(cfg.app.as_ref().map(|a| a.name.as_str()).unwrap_or("None chosen")).build();
        if let Some(a) = &cfg.app {
            app_row.add_prefix(&app_picker::icon_of(a));
        }
        let choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
        app_row.add_suffix(&choose);
        app_row.set_sensitive(cfg.app_mode != AppMode::Always);

        let mut rows: Vec<gtk::Widget> = vec![
            decorated.clone().upcast(),
            layer.clone().upcast(),
            note.clone().upcast(),
            scale.clone().upcast(),
            angle.clone().upcast(),
            opacity.clone().upcast(),
            click.clone().upcast(),
            snap.clone().upcast(),
            keep.clone().upcast(),
            mask.clone().upcast(),
        ];
        rows.extend(mask_rows.iter().cloned());
        rows.extend([
            app_mode.clone().upcast(),
            app_row.clone().upcast(),
        ]);
        // Folder widgets: rotation.
        let rot = &cfg.rotation;
        let rot_mode = adw::ComboRow::builder().title("Rotation").model(&gtk::StringList::new(&RotMode::ALL.map(|m| m.label()))).selected(rot.mode.index()).build();
        let spin = |title: &str, v: u32| {
            let r = adw::SpinRow::with_range(1.0, 86400.0, 1.0);
            r.set_title(title);
            r.set_value(v as f64);
            r
        };
        let interval = spin("Change every (seconds)", rot.interval);
        let rmin = spin("At least (seconds)", rot.min);
        let rmax = spin("At most (seconds)", rot.max);
        let sched = SchedEditor::new(&ctx, &id, cfg);
        if cfg.is_folder() {
            let sync_visible = {
                let (interval, rmin, rmax, sched) = (interval.clone(), rmin.clone(), rmax.clone(), sched.exp.clone());
                move |m: RotMode| {
                    interval.set_visible(m == RotMode::Sequential);
                    rmin.set_visible(m == RotMode::Random);
                    rmax.set_visible(m == RotMode::Random);
                    sched.set_visible(m == RotMode::Scheduled);
                }
            };
            sync_visible(rot.mode);
            rot_mode.connect_selected_notify(move |r| sync_visible(RotMode::from_index(r.selected())));
            rows.extend([rot_mode.clone().upcast(), interval.clone().upcast(), rmin.clone().upcast(), rmax.clone().upcast(), sched.exp.clone().upcast()]);
        }
        rows.extend([monitor.clone().upcast(), workspace.clone().upcast(), x.clone().upcast(), y.clone().upcast()]);
        for w in &rows {
            row.add_row(w);
        }
        sched.fill();
        self.pos_rows.borrow_mut().push((id.clone(), x.clone(), y.clone()));

        // Re-derive subtitle, the capability note and which fields make sense.
        let refresh_row: Rc<dyn Fn()> = {
            let (row, note, x, y) = (row.downgrade(), note.downgrade(), x.downgrade(), y.downgrade());
            let (ctx, id) = (ctx.clone(), id.clone());
            Rc::new(move || {
                let (Some(row), Some(note), Some(x), Some(y)) = (row.upgrade(), note.upgrade(), x.upgrade(), y.upgrade()) else { return };
                let Some(cfg) = ctx.widget_cfg(&id) else { return };
                row.set_subtitle(&glib::markup_escape_text(&summary(&cfg)));
                match ctx.env.limitation(&cfg) {
                    Some(text) => {
                        note.set_subtitle(text);
                        note.set_visible(true);
                    }
                    None => note.set_visible(false),
                }
                let can_place = ctx.env.positioning_works(&cfg);
                x.set_sensitive(can_place);
                y.set_sensitive(can_place);
            })
        };
        refresh_row();

        let edit: Edit = {
            let (ctx, id, refresh_row) = (ctx.clone(), id.clone(), refresh_row.clone());
            Rc::new(move |f| {
                ctx.edit_widget(&id, f);
                refresh_row();
            })
        };

        macro_rules! on_change {
            ($signal:ident, $widget:expr, |$w:ident, $cfg:ident| $body:expr) => {{
                let (edit, updating) = (edit.clone(), updating.clone());
                $widget.$signal(move |$w| {
                    if updating.get() {
                        return;
                    }
                    let v = $body;
                    edit(Box::new(move |$cfg: &mut WidgetConfig| v($cfg)));
                });
            }};
        }
        // Each body returns a closure that is applied to the stored config.
        on_change!(connect_active_notify, visible, |s, c| { let on = s.is_active(); move |c: &mut WidgetConfig| c.visible = on });
        on_change!(connect_active_notify, decorated, |s, c| { let on = s.is_active(); move |c: &mut WidgetConfig| c.decorated = on });
        on_change!(connect_selected_notify, layer, |s, c| { let l = Level::from_index(s.selected()); move |c: &mut WidgetConfig| c.level = l });
        on_change!(connect_value_notify, scale, |s, c| { let v = s.value() / 100.0; move |c: &mut WidgetConfig| c.scale = v });
        on_change!(connect_value_notify, angle, |s, c| { let v = s.value(); move |c: &mut WidgetConfig| c.angle = v });
        on_change!(connect_value_notify, opacity, |s, c| { let v = s.value() / 100.0; move |c: &mut WidgetConfig| c.opacity = v });
        on_change!(connect_active_notify, click, |s, c| { let on = s.is_active(); move |c: &mut WidgetConfig| c.click_through = on });
        on_change!(connect_active_notify, snap, |s, c| { let on = s.is_active(); move |c: &mut WidgetConfig| c.snap_edges = on });
        on_change!(connect_active_notify, keep, |s, c| { let on = s.is_active(); move |c: &mut WidgetConfig| c.keep_inside = on });
        {
            let paths: Vec<std::path::PathBuf> = masks.iter().map(|m| m.path.clone()).collect();
            let ctx = ctx.clone();
            on_change!(connect_selected_notify, mask, |s, c| {
                let p = (s.selected() as usize).checked_sub(1).and_then(|i| paths.get(i).cloned());
                let rows = mask_rows.clone();
                for r in &rows {
                    r.set_sensitive(p.is_some());
                }
                if p.is_none() {
                    ctx.set_mask_edit(None);
                }
                move |c: &mut WidgetConfig| c.mask = p
            });
        }
        for (r, sp) in &xf_spins {
            let set = sp.5;
            on_change!(connect_value_notify, r, |s, c| { let v = s.value(); move |c: &mut WidgetConfig| set(c, v) });
        }
        {
            let (ctx, id, updating) = (ctx.clone(), id.clone(), updating.clone());
            edit_sw.connect_active_notify(move |s| {
                if !updating.get() {
                    ctx.set_mask_edit(s.is_active().then(|| id.clone()));
                }
            });
        }
        {
            let (ctx, id, this) = (ctx.clone(), id.clone(), Rc::downgrade(self));
            xf_reset.connect_clicked(move |_| {
                ctx.edit_widget(&id, |c| {
                    c.mask_xf = Default::default();
                });
                if let Some(this) = this.upgrade() {
                    this.update_mask_xf();
                }
            });
        }
        {
            let app_row = app_row.clone();
            on_change!(connect_selected_notify, app_mode, |s, c| {
                let m = AppMode::from_index(s.selected());
                app_row.set_sensitive(m != AppMode::Always);
                move |c: &mut WidgetConfig| c.app_mode = m
            });
        }
        {
            let (edit, app_row) = (edit.clone(), app_row.clone());
            choose.connect_clicked(move |b| {
                let (edit, app_row) = (edit.clone(), app_row.clone());
                app_picker::pick_app(b, move |app| {
                    app_row.set_subtitle(&glib::markup_escape_text(&app.name));
                    edit(Box::new(move |c| c.app = Some(app)));
                });
            });
        }
        on_change!(connect_selected_notify, rot_mode, |s, c| { let m = RotMode::from_index(s.selected()); move |c: &mut WidgetConfig| c.rotation.mode = m });
        on_change!(connect_value_notify, interval, |s, c| { let v = s.value() as u32; move |c: &mut WidgetConfig| c.rotation.interval = v });
        on_change!(connect_value_notify, rmin, |s, c| { let v = s.value() as u32; move |c: &mut WidgetConfig| { c.rotation.min = v; c.rotation.max = c.rotation.max.max(v) } });
        on_change!(connect_value_notify, rmax, |s, c| { let v = s.value() as u32; move |c: &mut WidgetConfig| { c.rotation.max = v; c.rotation.min = c.rotation.min.min(v) } });
        on_change!(connect_selected_notify, workspace, |s, c| { let w = s.selected().checked_sub(1); move |c: &mut WidgetConfig| c.workspace = w });
        {
            let names = monitor_names.clone();
            let x11 = ctx.env.backend == crate::env::Backend::X11;
            let this = Rc::downgrade(self);
            on_change!(connect_selected_notify, monitor, |s, c| {
                let m = (s.selected() as usize).checked_sub(1).and_then(|i| names.get(i).cloned());
                let this = this.clone();
                move |c: &mut WidgetConfig| {
                    // Back to "Automatic" on X11: keep the widget where it is, now in screen coordinates.
                    if m.is_none() && x11 {
                        (c.x, c.y) = monitors::to_global(c.monitor.as_deref(), c.x, c.y);
                    }
                    c.monitor = m;
                    if let Some(this) = this.upgrade() {
                        // After the edit is stored: the number fields follow.
                        glib::idle_add_local_once(move || this.update_positions());
                    }
                }
            });
        }
        on_change!(connect_value_notify, x, |s, c| { let v = s.value() as i32; move |c: &mut WidgetConfig| c.x = v });
        on_change!(connect_value_notify, y, |s, c| { let v = s.value() as i32; move |c: &mut WidgetConfig| c.y = v });

        row
    }
}

/// "From this time on, show this file" list of a folder widget in scheduled mode.
struct SchedEditor {
    ctx: Rc<AppCtx>,
    id: String,
    /// File names of the folder (what the entries can pick).
    files: Vec<String>,
    exp: adw::ExpanderRow,
    rows: RefCell<Vec<gtk::Widget>>,
}

impl SchedEditor {
    fn new(ctx: &Rc<AppCtx>, id: &str, cfg: &WidgetConfig) -> Rc<SchedEditor> {
        let files = media::list_folder(&cfg.path).iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
        let exp = adw::ExpanderRow::builder().title("Schedule").subtitle("From each time on, show the chosen file").expanded(true).build();
        let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("Add a time").valign(gtk::Align::Center).css_classes(["flat"]).build();
        exp.add_suffix(&add);
        let ed = Rc::new(SchedEditor { ctx: ctx.clone(), id: id.to_string(), files, exp, rows: RefCell::new(Vec::new()) });
        {
            let ed2 = ed.clone();
            add.connect_clicked(move |_| {
                let file = ed2.files.first().cloned().unwrap_or_default();
                ed2.ctx.edit_widget(&ed2.id, |c| c.rotation.schedule.push(SchedEntry { start: 8 * 60, file }));
                ed2.fill();
            });
        }
        ed
    }

    /// (Re)build the entry rows from the stored configuration.
    fn fill(self: &Rc<Self>) {
        for r in self.rows.borrow_mut().drain(..) {
            self.exp.remove(&r);
        }
        let h24 = self.ctx.state.borrow().time_24h;
        let entries = self.ctx.widget_cfg(&self.id).map(|c| c.rotation.schedule).unwrap_or_default();
        for (i, e) in entries.iter().enumerate() {
            let line = super::wrap_row(6);
            let (ctx, id) = (self.ctx.clone(), self.id.clone());
            line.append(&super::time_box(e.start, h24, move |m| ctx.edit_widget(&id, move |c| {
                if let Some(e) = c.rotation.schedule.get_mut(i) {
                    e.start = m;
                }
            })));
            let names: Vec<&str> = self.files.iter().map(String::as_str).collect();
            let pick = gtk::DropDown::from_strings(&names);
            pick.set_valign(gtk::Align::Center);
            pick.set_selected(self.files.iter().position(|f| *f == e.file).unwrap_or(0) as u32);
            let (ctx, id, files) = (self.ctx.clone(), self.id.clone(), self.files.clone());
            pick.connect_selected_notify(move |d| {
                let Some(f) = files.get(d.selected() as usize).cloned() else { return };
                ctx.edit_widget(&id, move |c| {
                    if let Some(e) = c.rotation.schedule.get_mut(i) {
                        e.file = f;
                    }
                });
            });
            line.append(&pick);
            let del = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Remove").valign(gtk::Align::Center).css_classes(["flat"]).build();
            let ed = self.clone();
            del.connect_clicked(move |_| {
                ed.ctx.edit_widget(&ed.id, |c| {
                    if i < c.rotation.schedule.len() {
                        c.rotation.schedule.remove(i);
                    }
                });
                ed.fill();
            });
            line.append(&del);
            let row = gtk::ListBoxRow::builder().activatable(false).child(&line).build();
            self.exp.add_row(&row);
            self.rows.borrow_mut().push(row.upcast());
        }
    }
}
