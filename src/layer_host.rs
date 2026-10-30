//! `LayerHost`: one wlr-layer-shell surface per layer, drawing *all* widgets of that layer.
//!
//! Why not one surface per widget? The layer-shell protocol leaves the stacking
//! order of surfaces **within a layer** up to the compositor (sway and others
//! differ), so a user-defined priority order cannot be enforced by mapping order.
//! Inside one surface, GTK stacks children in sibling order, which we control
//! exactly. It is still one process, and cheaper than N surfaces.
//!
//! The host surface is anchored to all four edges (it covers the output) but its
//! *input region* is just the union of the widgets' rectangles, so everything
//! between widgets stays clickable on the desktop below. A widget marked
//! "click-through" is left out of the region.
//!
//! Because the surface never moves or resizes, dragging a widget is just moving
//! a child, in a coordinate frame that stays stable for the whole grab.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{cairo, gdk};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::anim_view::AnimView;
use crate::media::Anim;
use crate::model::WidgetConfig;
use crate::widget::{placed_size, scaled_size, MovedCb};

struct Child {
    id: String,
    view: AnimView,
    pos: (i32, i32),
    size: (i32, i32),
    click_through: bool,
    snap_edges: bool,
    keep_inside: bool,
}

/// Pull toward a screen edge when this close (pixels).
const SNAP: i32 = 16;

/// Where a dragged widget of `size` ends up, given the surface size.
/// Unconfined widgets may leave the screen; `keep_inside` clamps, `snap_edges` sticks to near edges.
pub fn place(pos: (i32, i32), size: (i32, i32), area: (i32, i32), keep_inside: bool, snap_edges: bool) -> (i32, i32) {
    let axis = |p: i32, len: i32, max: i32| {
        if max <= 0 {
            return p; // surface not sized yet: do not shove the widget anywhere
        }
        let mut p = p;
        if keep_inside {
            p = p.clamp(0, (max - len).max(0));
        }
        if snap_edges {
            if p.abs() < SNAP {
                p = 0;
            } else if (max - len - p).abs() < SNAP {
                p = max - len;
            }
        }
        p
    };
    (axis(pos.0, size.0, area.0), axis(pos.1, size.1, area.1))
}

pub struct LayerHost {
    window: gtk::Window,
    fixed: gtk::Fixed,
    /// Back to front.
    children: RefCell<Vec<Child>>,
    on_moved: MovedCb,
}

impl LayerHost {
    pub fn new(app: &adw::Application, layer: Layer, on_moved: MovedCb) -> Rc<LayerHost> {
        let fixed = gtk::Fixed::new();
        let window = gtk::Window::builder()
            .application(app)
            .title("GifDeck widgets")
            .decorated(false)
            .resizable(true) // the compositor sizes layer surfaces
            .build();
        window.add_css_class("gifdeck-widget");
        window.init_layer_shell(); // before the window is realised
        window.set_layer(layer);
        window.set_namespace(Some("gifdeck"));
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_exclusive_zone(-1); // coordinates are from the screen corner, ignoring panels
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        window.set_child(Some(&fixed));

        let host = Rc::new(LayerHost { window, fixed, children: RefCell::new(Vec::new()), on_moved });

        let weak = Rc::downgrade(&host);
        host.window.connect_map(move |_| {
            if let Some(h) = weak.upgrade() {
                h.update_input_region();
            }
        });
        host.install_drag();
        host.window.present();
        host
    }

    /// Add (or replace) a widget. New widgets go to the front until `reorder` is called.
    pub fn upsert(&self, cfg: &WidgetConfig, anim: &Rc<Anim>, mask: Option<Rc<Anim>>) {
        self.remove(&cfg.id);
        let (cw, ch) = scaled_size(anim, cfg.scale);
        let (w, h) = placed_size(anim, cfg.scale, cfg.angle);
        let view = AnimView::new(anim.clone(), cw, ch, mask);
        view.set_angle(cfg.angle);
        view.set_opacity(cfg.opacity);
        view.play();
        if !cfg.click_through {
            view.set_cursor_from_name(Some("grab"));
        }
        self.fixed.put(&view, cfg.x as f64, cfg.y as f64);
        self.children.borrow_mut().push(Child { id: cfg.id.clone(), view, pos: (cfg.x, cfg.y), size: (w, h), click_through: cfg.click_through, snap_edges: cfg.snap_edges, keep_inside: cfg.keep_inside });
        self.update_input_region();
    }

    /// Show another picture in an existing widget (folder rotation).
    pub fn replace_anim(&self, cfg: &WidgetConfig, anim: &Rc<Anim>, mask: Option<Rc<Anim>>) {
        if let Some(c) = self.children.borrow_mut().iter_mut().find(|c| c.id == cfg.id) {
            let (cw, ch) = scaled_size(anim, cfg.scale);
            c.size = placed_size(anim, cfg.scale, cfg.angle);
            c.view.set_anim(anim.clone(), cw, ch, mask);
            c.view.set_angle(cfg.angle);
            c.view.play();
        }
        self.update_input_region();
    }

    pub fn remove(&self, id: &str) {
        let removed = {
            let mut children = self.children.borrow_mut();
            children.iter().position(|c| c.id == id).map(|i| children.remove(i))
        };
        if let Some(child) = removed {
            self.fixed.remove(&child.view);
            self.update_input_region();
        }
    }

    pub fn view(&self, id: &str) -> Option<AnimView> {
        self.children.borrow().iter().find(|c| c.id == id).map(|c| c.view.clone())
    }

    pub fn set_position(&self, id: &str, x: i32, y: i32) {
        if let Some(c) = self.children.borrow_mut().iter_mut().find(|c| c.id == id) {
            c.pos = (x, y);
            self.fixed.move_(&c.view, x as f64, y as f64);
        }
        self.update_input_region();
    }

    pub fn set_opacity(&self, id: &str, opacity: f64) {
        if let Some(c) = self.children.borrow().iter().find(|c| c.id == id) {
            c.view.set_opacity(opacity);
        }
    }

    /// `back_to_front`: widget ids in the order they must be stacked (last = in front).
    /// Ids that are not hosted here are ignored.
    pub fn reorder(&self, back_to_front: &[String]) {
        let mut children = self.children.borrow_mut();
        let rank = |id: &str| back_to_front.iter().position(|b| b == id).unwrap_or(usize::MAX);
        children.sort_by_key(|c| rank(&c.id)); // stable, unknown ids keep their relative order (at the front)
        // GTK stacks siblings in insertion order: re-insert back to front.
        for c in children.iter() {
            self.fixed.remove(&c.view);
            self.fixed.put(&c.view, c.pos.0 as f64, c.pos.1 as f64);
        }
    }

    /// Only the widgets themselves take mouse input; the rest of the surface is click-through.
    pub fn set_monitor(&self, monitor: &gdk::Monitor) {
        self.window.set_monitor(Some(monitor));
    }

    fn update_input_region(&self) {
        let Some(surface) = self.window.surface() else { return };
        let region = cairo::Region::create();
        for c in self.children.borrow().iter().filter(|c| !c.click_through) {
            let _ = region.union_rectangle(&cairo::RectangleInt::new(c.pos.0, c.pos.1, c.size.0, c.size.1));
        }
        surface.set_input_region(&region);
    }

    fn install_drag(self: &Rc<Self>) {
        #[derive(Default)]
        struct Drag {
            id: Option<String>,
            start: (i32, i32),
            size: (i32, i32),
            keep_inside: bool,
            snap_edges: bool,
        }
        let drag = Rc::new(RefCell::new(Drag::default()));
        let gesture = gtk::GestureDrag::new();
        gesture.set_button(gdk::BUTTON_PRIMARY);

        gesture.connect_drag_begin({
            let (host, drag): (Weak<LayerHost>, _) = (Rc::downgrade(self), drag.clone());
            move |g, x, y| {
                let Some(host) = host.upgrade() else { return };
                // Front-most widget under the pointer.
                let hit = host
                    .children
                    .borrow()
                    .iter()
                    .rev()
                    .find(|c| !c.click_through && x >= c.pos.0 as f64 && x < (c.pos.0 + c.size.0) as f64 && y >= c.pos.1 as f64 && y < (c.pos.1 + c.size.1) as f64)
                    .map(|c| (c.id.clone(), c.pos, c.size, c.keep_inside, c.snap_edges));
                match hit {
                    Some((id, start, size, keep_inside, snap_edges)) => *drag.borrow_mut() = Drag { id: Some(id), start, size, keep_inside, snap_edges },
                    None => {
                        *drag.borrow_mut() = Drag::default();
                        g.set_state(gtk::EventSequenceState::Denied);
                    }
                }
            }
        });

        gesture.connect_drag_update({
            let (host, drag) = (Rc::downgrade(self), drag.clone());
            move |_, ox, oy| {
                let (Some(host), d) = (host.upgrade(), drag.borrow()) else { return };
                let Some(id) = &d.id else { return };
                let want = (d.start.0 + ox.round() as i32, d.start.1 + oy.round() as i32);
                let (nx, ny) = place(want, d.size, (host.window.width(), host.window.height()), d.keep_inside, d.snap_edges);
                host.set_position(id, nx, ny);
            }
        });

        gesture.connect_drag_end({
            let (host, drag) = (Rc::downgrade(self), drag.clone());
            move |_, _, _| {
                let Some(host) = host.upgrade() else { return };
                let id = drag.borrow_mut().id.take();
                if let Some(id) = id {
                    let pos = host.children.borrow().iter().find(|c| c.id == id).map(|c| c.pos);
                    if let Some((x, y)) = pos {
                        (host.on_moved)(&id, x, y);
                    }
                }
            }
        });

        self.fixed.add_controller(gesture);
    }
}

#[cfg(test)]
mod tests {
    use super::place;

    #[test]
    fn drag_placement() {
        let area = (1000, 800);
        let size = (100, 50);
        // free: may leave the screen, and an unsized surface never moves anything
        assert_eq!(place((-30, 900), size, area, false, false), (-30, 900));
        assert_eq!(place((-30, 900), size, (0, 0), true, true), (-30, 900));
        // contained
        assert_eq!(place((-30, 900), size, area, true, false), (0, 750));
        // snapped near edges only
        assert_eq!(place((10, 40), size, area, false, true), (0, 40));
        assert_eq!(place((895, 300), size, area, false, true), (900, 300));
    }
}
