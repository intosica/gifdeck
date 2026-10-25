//! Runtime detection of the display stack, and what it means for widget windows.
//!
//! Three situations matter:
//!
//! | Session                     | How layers work                                  | Positioning       |
//! |-----------------------------|--------------------------------------------------|-------------------|
//! | X11 / Xorg                  | EWMH hints (`_NET_WM_STATE_ABOVE` / `_BELOW`)    | yes (`x11rb`)     |
//! | Wayland + wlr-layer-shell   | borderless widgets become layer surfaces         | yes (margins)     |
//! |   (COSMIC, sway, Hyprland…) | (Top / Bottom layer) in a shared surface per layer | only layer ones   |
//! | Wayland without layer-shell | plain windows, no layer control (e.g. GNOME)     | no                |

use std::path::Path;

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::Layer;

use crate::model::{Level, WidgetConfig};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    X11,
    Wayland,
    Other,
}

/// What kind of window a widget is realised as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Ordinary toplevel window (xdg-toplevel on Wayland, normal window on X11).
    Toplevel,
    /// wlr-layer-shell surface.
    Layer(Layer),
}

#[derive(Clone, Debug)]
pub struct Env {
    pub backend: Backend,
    pub layer_shell: bool,
    pub desktop: String,
    pub composited: bool,
    pub sandboxed: bool,
}

impl Env {
    pub fn detect(display: &gdk::Display) -> Env {
        let backend = match display.type_().name() {
            "GdkX11Display" => Backend::X11,
            "GdkWaylandDisplay" => Backend::Wayland,
            _ => Backend::Other,
        };
        Env {
            backend,
            layer_shell: backend == Backend::Wayland && gtk4_layer_shell::is_supported(),
            desktop: std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            composited: display.is_composited(),
            sandboxed: Path::new("/.flatpak-info").exists(),
        }
    }

    pub fn is_cosmic(&self) -> bool {
        self.desktop.split(':').any(|d| d.eq_ignore_ascii_case("cosmic"))
    }

    pub fn backend_label(&self) -> &'static str {
        match self.backend {
            Backend::X11 => "X11 / Xorg",
            Backend::Wayland => "Wayland",
            Backend::Other => "Unknown",
        }
    }

    pub fn desktop_label(&self) -> String {
        if self.is_cosmic() {
            "COSMIC".into()
        } else if self.desktop.is_empty() {
            "unknown desktop".into()
        } else {
            self.desktop.clone()
        }
    }

    /// How a widget with this configuration is realised on this session.
    pub fn role(&self, cfg: &WidgetConfig) -> Role {
        if self.layer_shell && !cfg.decorated {
            match cfg.level {
                Level::Desktop => return Role::Layer(Layer::Bottom),
                Level::Top => return Role::Layer(Layer::Top),
                Level::Normal => {}
            }
        }
        Role::Toplevel
    }

    /// Can the "layer" setting actually be honoured for this widget?
    pub fn layering_works(&self, cfg: &WidgetConfig) -> bool {
        match self.backend {
            Backend::X11 => true,
            Backend::Wayland => cfg.level == Level::Normal || (self.layer_shell && !cfg.decorated),
            Backend::Other => cfg.level == Level::Normal,
        }
    }

    /// Can the app place this widget at x/y?
    pub fn positioning_works(&self, cfg: &WidgetConfig) -> bool {
        match self.backend {
            Backend::X11 => true,
            Backend::Wayland => matches!(self.role(cfg), Role::Layer(_)),
            Backend::Other => false,
        }
    }

    /// One-sentence explanation shown in the UI when something is limited.
    pub fn limitation(&self, cfg: &WidgetConfig) -> Option<&'static str> {
        if !self.layering_works(cfg) {
            return Some(if self.backend == Backend::Wayland && !self.layer_shell {
                "This compositor has no layer-shell, so only the normal layer is available."
            } else if cfg.decorated {
                "On Wayland, windows with decorations cannot be pinned above or below others. Turn decorations off for layers."
            } else {
                "Layers are not available on this display server."
            });
        }
        if !self.positioning_works(cfg) {
            return Some("Wayland places ordinary windows itself. Drag the title bar (or the widget) to move it; the position is not remembered.");
        }
        None
    }
}
