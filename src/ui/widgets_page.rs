//! "Widgets" page: active preset, and the ordered list of widgets with their settings.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::ctx::AppCtx;
use crate::model::{Level, WidgetConfig};

pub struct WidgetsPage {
    ctx: Rc<AppCtx>,
    pub root: gtk::ScrolledWindow,
    preset_row: adw::ComboRow,
    list: gtk::ListBox,
    expanded: RefCell<HashSet<String>>,
    /// (widget id, x row, y row) so drags can update the numbers in place.
    pos_rows: RefCell<Vec<(String, adw::SpinRow, adw::SpinRow)>>,
    /// Set while the UI itself changes controls, so handlers ignore those changes.
    updating: Rc<Cell<bool>>,
}

fn summary(cfg: &WidgetConfig) -> String {
    let mut s = format!("{} · {}%", cfg.level.label(), (cfg.scale * 100.0).round() as i32);
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
            .label("No widgets yet.\nPress Add and pick a GIF or PNG.")
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

        let group = adw::PreferencesGroup::builder()
            .title("Widgets")
            .description("Top of the list is in front.")
            .header_suffix(&add)
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
                filter.set_name(Some("GIF and PNG images"));
                filter.add_mime_type("image/gif");
                filter.add_mime_type("image/png");
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                let dialog = gtk::FileDialog::builder().title("Add GIF or PNG").modal(true).filters(&filters).default_filter(&filter).build();
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
        let widgets = self.ctx.state.borrow().active_preset().widgets.clone();
        let total = widgets.len();
        for (i, cfg) in widgets.iter().enumerate() {
            self.list.append(&self.build_row(cfg, i, total));
        }
        self.updating.set(false);
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
        for w in [
            decorated.clone().upcast::<gtk::Widget>(),
            layer.clone().upcast(),
            note.clone().upcast(),
            scale.clone().upcast(),
            opacity.clone().upcast(),
            click.clone().upcast(),
            x.clone().upcast(),
            y.clone().upcast(),
        ] {
            row.add_row(&w);
        }
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
        on_change!(connect_value_notify, opacity, |s, c| { let v = s.value() / 100.0; move |c: &mut WidgetConfig| c.opacity = v });
        on_change!(connect_active_notify, click, |s, c| { let on = s.is_active(); move |c: &mut WidgetConfig| c.click_through = on });
        on_change!(connect_value_notify, x, |s, c| { let v = s.value() as i32; move |c: &mut WidgetConfig| c.x = v });
        on_change!(connect_value_notify, y, |s, c| { let v = s.value() as i32; move |c: &mut WidgetConfig| c.y = v });

        row
    }
}
