//! GifDeck: animated GIF / PNG desktop widgets, all managed by this one process.

mod anim_view;
mod autostart;
mod ctx;
mod env;
mod layer_host;
mod manager;
mod media;
mod model;
mod ui;
mod widget;
mod x11;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use ctx::{AppCtx, Refresh};

static BACKGROUND: AtomicBool = AtomicBool::new(false);

struct Running {
    ctx: Rc<AppCtx>,
    ui: Rc<ui::Ui>,
    first_activation: std::cell::Cell<bool>,
    _hold: gio::ApplicationHoldGuard,
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(autostart::APP_ID).build();
    app.add_main_option(
        "background",
        glib::Char::from(b'b'),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Start without showing the manager window",
        None,
    );
    app.connect_handle_local_options(|_, opts| {
        if opts.contains("background") {
            BACKGROUND.store(true, Ordering::Relaxed);
        }
        std::ops::ControlFlow::Continue(())
    });

    let running: Rc<RefCell<Option<Running>>> = Rc::new(RefCell::new(None));

    app.connect_startup({
        let running = running.clone();
        move |app| {
            adw::init().ok();
            ui::load_css();
            let display = gtk::gdk::Display::default().expect("no display");
            let env = env::Env::detect(&display);
            eprintln!(
                "gifdeck: {} on {}, layer-shell: {}, sandboxed: {}",
                env.backend_label(),
                env.desktop_label(),
                env.layer_shell,
                env.sandboxed
            );

            let ctx = AppCtx::new(app.clone(), env);
            let ui = ui::Ui::new(&ctx);

            let weak_ui = Rc::downgrade(&ui);
            let refresh_ui = weak_ui.clone();
            ctx.set_ui_hooks(
                Rc::new(move |what: Refresh| {
                    if let Some(ui) = refresh_ui.upgrade() {
                        ui.refresh(what);
                    }
                }),
                Rc::new(move |msg: &str| {
                    if let Some(ui) = weak_ui.upgrade() {
                        ui.toast(msg);
                    }
                }),
            );

            let quit = gio::SimpleAction::new("quit", None);
            quit.connect_activate({
                let app = app.clone();
                move |_, _| app.quit()
            });
            app.add_action(&quit);
            app.set_accels_for_action("app.quit", &["<Primary>q"]);

            // Show the widgets of the active preset (or of the scheduled one).
            ctx.flush();
            ctx.tick_schedule();

            glib::timeout_add_local(Duration::from_secs(15), {
                let ctx = Rc::downgrade(&ctx);
                move || match ctx.upgrade() {
                    Some(ctx) => {
                        ctx.tick_schedule();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                }
            });
            glib::timeout_add_local(Duration::from_secs(3), {
                let ctx = Rc::downgrade(&ctx);
                move || match ctx.upgrade() {
                    Some(ctx) => {
                        ctx.poll_positions();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                }
            });

            glib::timeout_add_local(Duration::from_secs(1), {
                let ctx = Rc::downgrade(&ctx);
                move || match ctx.upgrade() {
                    Some(ctx) => {
                        ctx.poll_apps();
                        ctx.manager.tick_rotation();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                }
            });

            *running.borrow_mut() = Some(Running {
                ctx,
                ui,
                first_activation: std::cell::Cell::new(true),
                _hold: app.hold(), // the process lives until "Quit", even with no window open
            });
        }
    });

    app.connect_activate({
        let running = running.clone();
        move |_| {
            if let Some(r) = running.borrow().as_ref() {
                let first = r.first_activation.replace(false);
                if first && BACKGROUND.load(Ordering::Relaxed) {
                    return; // started at login: widgets only
                }
                let _ = &r.ctx;
                r.ui.window.present();
            }
        }
    });

    app.run()
}
