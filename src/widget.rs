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

/// The box a widget occupies: its scaled picture, grown to fit when rotated.
pub fn placed_size(anim: &Anim, scale: f64, angle: f64) -> (i32, i32) {
    let (w, h) = scaled_size(anim, scale);
    let (w, h) = crate::mask_xf::bbox(angle, w as f64, h as f64);
    ((w.round() as i32).max(1), (h.round() as i32).max(1))
}

pub fn scaled_size(anim: &Anim, scale: f64) -> (i32, i32) {
    (
        ((anim.width as f64 * scale).round() as i32).max(1),
        ((anim.height as f64 * scale).round() as i32).max(1),
    )
}

impl WidgetWindow {
    /// Show the widget inside a shared layer surface.
    pub fn build_hosted(host: &Rc<LayerHost>, cfg: &WidgetConfig, anim: &Rc<Anim>, mask: Option<Rc<Anim>>) -> WidgetWindow {
        host.upsert(cfg, anim, mask);
        WidgetWindow { cfg: cfg.clone(), backing: Backing::Hosted(host.clone()), view: None }
    }

    /// Show the widget in its own window.
    pub fn build_window(app: &adw::Application, env: &Env, cfg: &WidgetConfig, anim: &Rc<Anim>, mask: Option<Rc<Anim>>, on_closed: ClosedCb) -> WidgetWindow {
        let (w, h) = placed_size(anim, cfg.scale, cfg.angle);
        let (cw, ch) = scaled_size(anim, cfg.scale);
        let view = AnimView::new(anim.clone(), cw, ch, mask);
        view.set_angle(cfg.angle);
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

    /// Show another picture (folder rotation) without rebuilding the window.
    pub fn set_media(&self, anim: &Rc<Anim>, mask: Option<Rc<Anim>>) {
        match &self.backing {
            Backing::Hosted(host) => host.replace_anim(&self.cfg, anim, mask),
            Backing::Window(_) => {
                if let Some(v) = &self.view {
                    let (w, h) = scaled_size(anim, self.cfg.scale);
                    v.set_anim(anim.clone(), w, h, mask);
                    v.set_angle(self.cfg.angle);
                    v.play();
                }
            }
        }
    }

    pub fn view(&self) -> Option<AnimView> {
        match &self.backing {
            Backing::Hosted(host) => host.view(&self.cfg.id),
            Backing::Window(_) => self.view.clone(),
        }
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
                if let Some(v) = host.view(&cfg.id) {
                    v.set_mask_look(cfg.mask_opacity, cfg.mask_xf);
                }
                if moved {
                    host.set_position(&cfg.id, cfg.x, cfg.y);
                }
            }
            Backing::Window(window) => {
                if let Some(v) = &self.view {
                    v.set_opacity(cfg.opacity);
                    v.set_mask_look(cfg.mask_opacity, cfg.mask_xf);
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

    /// X11 only: apply snap / keep-inside to the window's current spot; returns the final position.
    pub fn x11_constrain(&self, pos: (i32, i32)) -> (i32, i32) {
        let Backing::Window(w) = &self.backing else { return pos };
        if !(self.cfg.snap_edges || self.cfg.keep_inside) {
            return pos;
        }
        let Some(mon) = gtk::gdk::Display::default().and_then(|d| d.monitors().item(0)).and_downcast::<gtk::gdk::Monitor>() else { return pos };
        let g = mon.geometry();
        let rel = (pos.0 - g.x(), pos.1 - g.y());
        let r = crate::layer_host::place(rel, (w.width(), w.height()), (g.width(), g.height()), self.cfg.keep_inside, self.cfg.snap_edges);
        (r.0 + g.x(), r.1 + g.y())
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
