//! One widget as the manager sees it.
//!
//! A widget is realised in one of two ways (see `env.rs` for when):
//!
//! * **Window**: its own `gtk::Window`. X11 (with EWMH layer hints from `x11.rs`),
//!   and ordinary windows on Wayland (decorated, or "normal layer").
//! * **Hosted**: a child inside a [`LayerHost`], the shared layer-shell surface of
//!   its layer (Wayland "always on top" / "desktop" widgets).

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, glib};

use crate::anim_view::AnimView;
use crate::env::{Backend, Env};
use crate::layer_host::LayerHost;
use crate::media::Anim;
use crate::model::WidgetConfig;
use crate::x11;

pub type MovedCb = Rc<dyn Fn(&str, i32, i32)>;
pub type ClosedCb = Rc<dyn Fn(&str)>;

pub enum Backing {
    Window(gtk::Window),
    Hosted(Rc<LayerHost>),
}

pub struct WidgetWindow {
    /// The configuration this widget currently reflects.
    pub cfg: WidgetConfig,
    pub backing: Backing,
    view: Option<AnimView>,
}

pub fn scaled_size(anim: &Anim, scale: f64) -> (i32, i32) {
    (
        ((anim.width as f64 * scale).round() as i32).max(1),
        ((anim.height as f64 * scale).round() as i32).max(1),
    )
}

impl WidgetWindow {
    /// Show the widget inside a shared layer surface.
    pub fn build_hosted(host: &Rc<LayerHost>, cfg: &WidgetConfig, anim: &Rc<Anim>) -> WidgetWindow {
        host.upsert(cfg, anim);
        WidgetWindow { cfg: cfg.clone(), backing: Backing::Hosted(host.clone()), view: None }
    }

    /// Show the widget in its own window.
    pub fn build_window(app: &adw::Application, env: &Env, cfg: &WidgetConfig, anim: &Rc<Anim>, on_closed: ClosedCb) -> WidgetWindow {
        let (w, h) = scaled_size(anim, cfg.scale);
        let view = AnimView::new(anim.clone(), w, h);
        view.set_opacity(cfg.opacity);
        view.play();

        let window = gtk::Window::builder()
            .application(app)
            .title(cfg.title())
            .decorated(cfg.decorated)
            .resizable(false) // fits its content; scale is the size control
            .default_width(w)
            .default_height(h)
            .build();
        window.add_css_class("gifdeck-widget");

        if cfg.decorated {
            window.set_child(Some(&view));
        } else {
            // Lets the compositor / window manager drag the borderless window from anywhere.
            let handle = gtk::WindowHandle::new();
            handle.set_child(Some(&view));
            window.set_child(Some(&handle));
        }

        if env.backend == Backend::X11 {
            let level = cfg.level;
            let (x, y) = (cfg.x, cfg.y);
            window.connect_realize(move |win| {
                if let Some(xid) = x11::xid_of(win) {
                    x11::set_initial_state(xid, level);
                }
            });
            window.connect_map(move |win| {
                if let Some(xid) = x11::xid_of(win) {
                    x11::apply_state(xid, level);
                    x11::move_to(xid, x, y);
                }
            });
        }

        if cfg.click_through {
            window.connect_map(|win| {
                if let Some(surface) = win.surface() {
                    surface.set_input_region(&cairo::Region::create());
                }
            });
        }

        let id = cfg.id.clone();
        window.connect_close_request(move |_| {
            on_closed(&id);
            glib::Propagation::Proceed
        });

        window.present();
        WidgetWindow { cfg: cfg.clone(), backing: Backing::Window(window), view: Some(view) }
    }

    pub fn host(&self) -> Option<&Rc<LayerHost>> {
        match &self.backing {
            Backing::Hosted(h) => Some(h),
            Backing::Window(_) => None,
        }
    }

    /// Apply changes that do not need a rebuild.
    pub fn apply_live(&mut self, env: &Env, cfg: &WidgetConfig) {
        let moved = (cfg.x, cfg.y) != (self.cfg.x, self.cfg.y);
        match &self.backing {
            Backing::Hosted(host) => {
                host.set_opacity(&cfg.id, cfg.opacity);
                if moved {
                    host.set_position(&cfg.id, cfg.x, cfg.y);
                }
            }
            Backing::Window(window) => {
                if let Some(v) = &self.view {
                    v.set_opacity(cfg.opacity);
                }
                if moved && env.backend == Backend::X11 {
                    if let Some(xid) = x11::xid_of(window) {
                        x11::move_to(xid, cfg.x, cfg.y);
                    }
                }
            }
        }
        self.cfg = cfg.clone();
    }

    /// X11 only: where the window manager has put the window.
    pub fn x11_position(&self) -> Option<(i32, i32)> {
        match &self.backing {
            Backing::Window(w) => x11::xid_of(w).and_then(x11::position),
            Backing::Hosted(_) => None,
        }
    }

    pub fn raise_x11(&self) {
        if let Backing::Window(w) = &self.backing {
            if let Some(xid) = x11::xid_of(w) {
                x11::raise(xid);
            }
        }
    }

    /// Remove the widget. (An emptied host is cleaned up by the manager.)
    pub fn close(&self) {
        match &self.backing {
            Backing::Window(w) => w.destroy(),
            Backing::Hosted(h) => h.remove(&self.cfg.id),
        }
    }
}
