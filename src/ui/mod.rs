//! The compact manager window.

mod app_picker;
mod masks_page;
mod presets_page;
mod widgets_page;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::autostart;
use crate::ctx::{AppCtx, Refresh};
use crate::model::Storage;
use masks_page::MasksPage;
use presets_page::PresetsPage;
use widgets_page::WidgetsPage;

pub struct Ui {
    pub window: adw::ApplicationWindow,
    ctx: Rc<AppCtx>,
    toasts: adw::ToastOverlay,
    widgets: Rc<WidgetsPage>,
    presets: Rc<PresetsPage>,
    masks: Rc<MasksPage>,
    /// "All widgets on/off" action, also exported over D-Bus (quick settings toggle).
    enabled_action: gio::SimpleAction,
    rebuild_blacklist: Rc<dyn Fn()>,
}

impl Ui {
    pub fn new(ctx: &Rc<AppCtx>) -> Rc<Ui> {
        let widgets = WidgetsPage::new(ctx);
        let presets = PresetsPage::new(ctx);
        let masks = MasksPage::new(ctx);
        let (settings, rebuild_blacklist) = settings_page(ctx);

        let stack = adw::ViewStack::new();
        stack.add_titled_with_icon(&widgets.root, Some("widgets"), "Widgets", "applications-graphics-symbolic");
        stack.add_titled_with_icon(&presets.root, Some("presets"), "Presets", "x-office-calendar-symbolic");
        stack.add_titled_with_icon(&masks.root, Some("masks"), "Masks", "image-x-generic-symbolic");
        stack.add_titled_with_icon(&settings, Some("settings"), "Settings", "emblem-system-symbolic");

        let menu = gio::Menu::new();
        menu.append(Some("Quit GifDeck"), Some("app.quit"));
        let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).primary(true).build();
        let header = adw::HeaderBar::builder().title_widget(&adw::WindowTitle::new("GifDeck", "")).build();
        header.pack_end(&menu_button);

        // One switch for every widget; the same action backs the GNOME quick settings toggle.
        let enabled_action = gio::SimpleAction::new_stateful("widgets-enabled", None, &ctx.state.borrow().widgets_enabled.to_variant());
        {
            let ctx = ctx.clone();
            enabled_action.connect_activate(move |a, _| {
                let on = !a.state().and_then(|v| v.get::<bool>()).unwrap_or(true);
                ctx.set_widgets_enabled(on);
            });
        }
        ctx.app.add_action(&enabled_action);
        let toggle = gtk::ToggleButton::builder().icon_name("view-reveal-symbolic").tooltip_text("Show / hide all widgets").action_name("app.widgets-enabled").build();
        header.pack_start(&toggle);

        // Always at the bottom: three labelled tabs never fit next to the window buttons in a compact window.
        let bottom = adw::ViewSwitcherBar::builder().stack(&stack).reveal(true).build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_bottom_bar(&bottom);
        toolbar.set_content(Some(&stack));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&toolbar));

        // Drop files or folders anywhere: they become widgets (or masks while the Masks tab is open).
        let drop = gtk::DropTarget::new(gio::File::static_type(), gtk::gdk::DragAction::COPY);
        drop.set_types(&[gtk::gdk::FileList::static_type()]);
        {
            let (ctx, stack) = (ctx.clone(), stack.clone());
            drop.connect_drop(move |_, value, _, _| {
                let Ok(list) = value.get::<gtk::gdk::FileList>() else { return false };
                let files = list.files();
                if stack.visible_child_name().as_deref() == Some("masks") {
                    ctx.import_masks(files);
                } else {
                    ctx.import_files(files);
                }
                true
            });
        }
        toasts.add_controller(drop);

        let window = adw::ApplicationWindow::builder()
            .application(&ctx.app)
            .title("GifDeck")
            .default_width(480)
            .default_height(720)
            .content(&toasts)
            .build();
        // Mask editing ends as soon as the user clicks anything but the mask controls.
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.set_button(0);
        {
            let ctx = ctx.clone();
            let win = window.downgrade();
            click.connect_pressed(move |_, _, x, y| {
                if ctx.edit_mask.borrow().is_none() {
                    return;
                }
                let Some(win) = win.upgrade() else { return };
                let mut w = win.pick(x, y, gtk::PickFlags::DEFAULT);
                while let Some(cur) = w {
                    if cur.has_css_class(widgets_page::XF_KEEP) {
                        return;
                    }
                    w = cur.parent();
                }
                ctx.set_mask_edit(None);
            });
        }
        window.add_controller(click);
        // Closing the manager keeps the widgets (and the process) alive.
        window.set_hide_on_close(true);

        Rc::new(Ui { ctx: ctx.clone(), window, toasts, widgets, presets, masks, enabled_action, rebuild_blacklist })
    }

    fn widgets_enabled(&self) -> bool {
        self.ctx.state.borrow().widgets_enabled
    }

    pub fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    pub fn refresh(self: &Rc<Self>, what: Refresh) {
        match what {
            Refresh::Widgets => self.widgets.rebuild(),
            Refresh::Positions => self.widgets.update_positions(),
            Refresh::MaskXf => self.widgets.update_mask_xf(),
            Refresh::MaskEdit => self.widgets.update_mask_edit(),
            Refresh::Names => {
                self.widgets.update_presets();
                self.presets.rebuild_rules();
            }
            Refresh::Schedule => self.presets.rebuild_rules(),
            Refresh::Masks => {
                self.masks.rebuild();
                self.widgets.rebuild();
            }
            Refresh::Enabled => self.enabled_action.set_state(&self.widgets_enabled().to_variant()),
            Refresh::Blacklist => (self.rebuild_blacklist)(),
            Refresh::Status => self.presets.update_status(),
            Refresh::All => {
                self.masks.rebuild();
                self.presets.rebuild_presets();
                self.presets.rebuild_rules();
                self.widgets.update_presets();
                self.widgets.rebuild();
            }
        }
    }
}

/// Horizontal row of controls that wraps onto a second line when the window is narrow.
/// ponytail: gtk::FlowBox instead of adw::WrapBox — WrapBox needs libadwaita 1.7, this machine ships 1.5.
/// Swap back once the distro has libadwaita ≥ 1.7 (and restore the v1_7 feature in Cargo.toml).
pub(super) fn wrap_row(top: i32) -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(false)
        .max_children_per_line(u32::MAX)
        .column_spacing(8)
        .row_spacing(6)
        .valign(gtk::Align::Center)
        .margin_top(top)
        .margin_bottom(top)
        .margin_start(12)
        .margin_end(12)
        .build()
}

/// Time-of-day editor returning minutes since midnight: "16 : 05", or "4 : 05 PM" when `h24` is off.
pub(super) fn time_box(minutes: u16, h24: bool, on_change: impl Fn(u16) + 'static) -> gtk::Box {
    let (lo, hi, shown) = if h24 { (0.0, 23.0, minutes / 60) } else { (1.0, 12.0, if minutes / 60 % 12 == 0 { 12 } else { minutes / 60 % 12 }) };
    let hours = gtk::SpinButton::with_range(lo, hi, 1.0);
    let mins = gtk::SpinButton::with_range(0.0, 59.0, 1.0);
    let ampm = gtk::DropDown::from_strings(&["AM", "PM"]);
    hours.set_value(shown as f64);
    mins.set_value((minutes % 60) as f64);
    ampm.set_selected(u32::from(minutes / 60 >= 12));
    for s in [&hours, &mins] {
        s.set_valign(gtk::Align::Center);
        s.set_wrap(true);
        s.set_width_chars(2);
        s.connect_output(|s| {
            s.set_text(&format!("{:02}", s.value() as i32));
            glib::Propagation::Stop
        });
    }
    ampm.set_valign(gtk::Align::Center);
    let cb = Rc::new(on_change);
    let emit = {
        let (hours, mins, ampm) = (hours.clone(), mins.clone(), ampm.clone());
        move || {
            let h = hours.value() as u16;
            let h = if h24 { h } else { h % 12 + 12 * ampm.selected() as u16 };
            cb(h * 60 + mins.value() as u16);
        }
    };
    let emit = Rc::new(emit);
    for s in [&hours, &mins] {
        let emit = emit.clone();
        s.connect_value_changed(move |_| emit());
    }
    {
        let emit = emit.clone();
        ampm.connect_selected_notify(move |_| emit());
    }
    let b = gtk::Box::builder().spacing(4).valign(gtk::Align::Center).build();
    b.append(&hours);
    b.append(&gtk::Label::new(Some(":")));
    b.append(&mins);
    if !h24 {
        b.append(&ampm);
    }
    b
}

fn info_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle(subtitle).build();
    row.set_subtitle_selectable(true);
    row
}

fn settings_page(ctx: &Rc<AppCtx>) -> (adw::PreferencesPage, Rc<dyn Fn()>) {
    let page = adw::PreferencesPage::new();

    // ---- startup
    let startup = adw::PreferencesGroup::builder().title("Startup").build();
    let autostart_row = adw::SwitchRow::builder()
        .title("Start when I log in")
        .subtitle("Runs in the background and shows your widgets. Open GifDeck again to get this window back.")
        .active(ctx.state.borrow().autostart)
        .build();
    let busy = Rc::new(Cell::new(false));
    {
        let ctx = ctx.clone();
        let busy = busy.clone();
        autostart_row.connect_active_notify(move |row| {
            if busy.get() {
                return;
            }
            let on = row.is_active();
            let (ctx, row, busy) = (ctx.clone(), row.clone(), busy.clone());
            autostart::set_autostart(on, move |result| match result {
                Ok(()) => {
                    ctx.state.borrow_mut().autostart = on;
                    ctx.save();
                }
                Err(e) => {
                    busy.set(true);
                    row.set_active(!on); // put the switch back: it did not work
                    busy.set(false);
                    ctx.toast(&format!("Could not change autostart: {e}"));
                }
            });
        });
    }
    startup.add(&autostart_row);
    page.add(&startup);

    // ---- files
    let files = adw::PreferencesGroup::builder().title("Files").description("Applies to GIFs, PNGs, widget folders and masks you add.").build();
    let storage = adw::ComboRow::builder()
        .title("When adding files")
        .model(&gtk::StringList::new(&Storage::ALL.map(|s| s.label())))
        .selected(ctx.state.borrow().storage.index())
        .build();
    {
        let ctx = ctx.clone();
        storage.connect_selected_notify(move |r| ctx.set_storage(Storage::from_index(r.selected())));
    }
    files.add(&storage);
    let where_to = adw::ActionRow::builder()
        .title("Organized folders")
        .subtitle("GifDeck/GIFs, PNGs, Widgets and Masks inside the chosen location")
        .build();
    files.add(&where_to);
    page.add(&files);

    // ---- blacklist
    let blacklist = adw::PreferencesGroup::builder()
        .title("Application blacklist")
        .description(if ctx.env.backend == crate::env::Backend::X11 {
            "Widgets are hidden while one of these applications has a window showing."
        } else {
            "Needs an X11 session: other applications' windows cannot be seen on Wayland."
        })
        .build();
    let add_app = gtk::Button::builder().child(&adw::ButtonContent::builder().icon_name("list-add-symbolic").label("Add").build()).build();
    blacklist.set_header_suffix(Some(&add_app));
    let shown: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
    let rebuild: Rc<dyn Fn()> = {
        let (ctx, group, shown) = (ctx.clone(), blacklist.clone(), shown.clone());
        Rc::new(move || {
            for r in shown.borrow_mut().drain(..) {
                group.remove(&r);
            }
            let apps = ctx.state.borrow().blacklist.clone();
            for app in apps {
                let row = adw::ActionRow::builder().title(glib::markup_escape_text(&app.name)).build();
                row.add_prefix(&app_picker::icon_of(&app));
                let del = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Remove").valign(gtk::Align::Center).css_classes(["flat"]).build();
                let (ctx, id) = (ctx.clone(), app.id.clone());
                del.connect_clicked(move |_| ctx.remove_blacklist(&id));
                row.add_suffix(&del);
                group.add(&row);
                shown.borrow_mut().push(row);
            }
        })
    };
    rebuild();
    {
        let ctx = ctx.clone();
        add_app.connect_clicked(move |b| {
            let ctx = ctx.clone();
            app_picker::pick_app(b, move |app| ctx.add_blacklist(app));
        });
    }
    page.add(&blacklist);

    // ---- what was detected
    let env = &ctx.env;
    let session = adw::PreferencesGroup::builder().title("This session").description("Detected automatically").build();
    session.add(&info_row("Display server", env.backend_label()));
    session.add(&info_row("Desktop", &env.desktop_label()));
    session.add(&info_row(
        "How layers work",
        match (env.backend, env.layer_shell) {
            (crate::env::Backend::X11, _) => "EWMH window-manager hints (above / below)",
            (crate::env::Backend::Wayland, true) => "wlr-layer-shell: borderless widgets are layer surfaces",
            (crate::env::Backend::Wayland, false) => "Not available: this compositor has no layer-shell",
            _ => "Not available",
        },
    ));
    session.add(&info_row(
        "Transparency",
        if env.composited { "Available (compositing is on)" } else { "Compositing is off: transparent areas will look black or opaque" },
    ));
    if env.sandboxed {
        session.add(&info_row("Packaging", "Flatpak sandbox"));
    }
    page.add(&session);
    (page, rebuild)
}

/// Application-wide CSS: widget windows are fully transparent.
pub fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("../../data/style.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

#[allow(dead_code)]
fn _assert_types(_: &glib::Value) {}
