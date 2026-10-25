//! The compact manager window.

mod presets_page;
mod widgets_page;

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::autostart;
use crate::ctx::{AppCtx, Refresh};
use presets_page::PresetsPage;
use widgets_page::WidgetsPage;

pub struct Ui {
    pub window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    widgets: Rc<WidgetsPage>,
    presets: Rc<PresetsPage>,
}

impl Ui {
    pub fn new(ctx: &Rc<AppCtx>) -> Rc<Ui> {
        let widgets = WidgetsPage::new(ctx);
        let presets = PresetsPage::new(ctx);
        let settings = settings_page(ctx);

        let stack = adw::ViewStack::new();
        stack.add_titled_with_icon(&widgets.root, Some("widgets"), "Widgets", "applications-graphics-symbolic");
        stack.add_titled_with_icon(&presets.root, Some("presets"), "Presets", "x-office-calendar-symbolic");
        stack.add_titled_with_icon(&settings, Some("settings"), "Settings", "emblem-system-symbolic");

        let menu = gio::Menu::new();
        menu.append(Some("Quit GifDeck"), Some("app.quit"));
        let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).primary(true).build();
        let header = adw::HeaderBar::builder().title_widget(&adw::WindowTitle::new("GifDeck", "")).build();
        header.pack_end(&menu_button);

        // Always at the bottom: three labelled tabs never fit next to the window buttons in a compact window.
        let bottom = adw::ViewSwitcherBar::builder().stack(&stack).reveal(true).build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_bottom_bar(&bottom);
        toolbar.set_content(Some(&stack));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&toolbar));

        let window = adw::ApplicationWindow::builder()
            .application(&ctx.app)
            .title("GifDeck")
            .default_width(480)
            .default_height(720)
            .content(&toasts)
            .build();
        // Closing the manager keeps the widgets (and the process) alive.
        window.set_hide_on_close(true);

        Rc::new(Ui { window, toasts, widgets, presets })
    }

    pub fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    pub fn refresh(self: &Rc<Self>, what: Refresh) {
        match what {
            Refresh::Widgets => self.widgets.rebuild(),
            Refresh::Positions => self.widgets.update_positions(),
            Refresh::Names => {
                self.widgets.update_presets();
                self.presets.rebuild_rules();
            }
            Refresh::Schedule => self.presets.rebuild_rules(),
            Refresh::Status => self.presets.update_status(),
            Refresh::All => {
                self.presets.rebuild_presets();
                self.presets.rebuild_rules();
                self.widgets.update_presets();
                self.widgets.rebuild();
            }
        }
    }
}

fn info_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle(subtitle).build();
    row.set_subtitle_selectable(true);
    row
}

fn settings_page(ctx: &Rc<AppCtx>) -> adw::PreferencesPage {
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
    page
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
