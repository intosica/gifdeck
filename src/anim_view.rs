//! `AnimView`: a tiny custom widget that draws the current frame of an [`Anim`]
//! scaled to a fixed size.
//!
//! Why not `gtk::Picture`? Swapping paintables re-runs layout, and a Picture's
//! natural size is the image's own size, which fights with a user-chosen scale.
//! Here a frame change is a single `queue_draw()`: the GPU already holds every
//! frame as a texture, so animating costs one cheap re-snapshot per GIF frame.
//! The next frame is scheduled with a one-shot timer, so a 10 fps GIF wakes the
//! process 10 times a second, not 60.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

use crate::mask_xf::{self, Affine, Handle};
use crate::media::Anim;
use crate::model::MaskXf;

/// Called with the new transform; `true` once the drag is finished.
pub type XfCb = Rc<dyn Fn(MaskXf, bool)>;

fn transform(xf: &MaskXf, w: i32, h: i32) -> gsk::Transform {
    let a = Affine::of(xf, w as f64, h as f64);
    gsk::Transform::new().matrix(&graphene::Matrix::from_2d(a.0, a.1, a.2, a.3, a.4, a.5))
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AnimView {
        pub anim: RefCell<Option<Rc<Anim>>>,
        pub frame: Cell<usize>,
        pub size: Cell<(i32, i32)>,
        pub downscale: Cell<bool>,
        pub mask: RefCell<Option<Rc<Anim>>>,
        pub mask_frame: Cell<usize>,
        pub mask_opacity: Cell<f64>,
        pub xf: Cell<MaskXf>,
        /// Mask transform handles are shown and draggable.
        pub edit: Cell<bool>,
        pub grab: Cell<Option<(Handle, MaskXf, (f64, f64))>>,
        pub idle_cursor: RefCell<Option<String>>,
        pub on_xf: RefCell<Option<XfCb>>,
        pub on_exit: RefCell<Option<Rc<dyn Fn()>>>,
        /// Bumped whenever the animation is replaced, so old timer chains stop.
        pub generation: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AnimView {
        const NAME: &'static str = "GifDeckAnimView";
        type Type = super::AnimView;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for AnimView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            let drag = gtk::GestureDrag::new();
            drag.set_button(gtk::gdk::BUTTON_PRIMARY);
            drag.connect_drag_begin({
                let v = view.downgrade();
                move |g, x, y| {
                    let Some(v) = v.upgrade() else { return };
                    if !v.imp().edit.get() {
                        g.set_state(gtk::EventSequenceState::Denied);
                        return;
                    }
                    let (w, h) = v.imp().size.get();
                    let xf = v.imp().xf.get();
                    match mask_xf::hit(&xf, w as f64, h as f64, (x, y)) {
                        Some(handle) => {
                            v.imp().grab.set(Some((handle, xf, (x, y))));
                            g.set_state(gtk::EventSequenceState::Claimed);
                        }
                        None => {
                            // Touched the widget outside the mask: leave edit mode, let the widget be dragged as usual.
                            g.set_state(gtk::EventSequenceState::Denied);
                            let cb = v.imp().on_exit.borrow().clone();
                            if let Some(cb) = cb {
                                cb();
                            }
                        }
                    }
                }
            });
            drag.connect_drag_update({
                let v = view.downgrade();
                move |_, ox, oy| {
                    let Some(v) = v.upgrade() else { return };
                    let Some((handle, xf0, p0)) = v.imp().grab.get() else { return };
                    let (w, h) = v.imp().size.get();
                    v.imp().xf.set(mask_xf::drag(handle, &xf0, w as f64, h as f64, p0, (ox, oy)));
                    v.queue_draw();
                }
            });
            drag.connect_drag_end({
                let v = view.downgrade();
                move |_, _, _| {
                    let Some(v) = v.upgrade() else { return };
                    if v.imp().grab.take().is_some() {
                        let cb = v.imp().on_xf.borrow().clone();
                        if let Some(cb) = cb {
                            cb(v.imp().xf.get(), true);
                        }
                    }
                }
            });
            view.add_controller(drag);

            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion({
                let v = view.downgrade();
                move |_, x, y| {
                    let Some(v) = v.upgrade() else { return };
                    if !v.imp().edit.get() || v.imp().grab.get().is_some() {
                        return;
                    }
                    let (w, h) = v.imp().size.get();
                    let name = match mask_xf::hit(&v.imp().xf.get(), w as f64, h as f64, (x, y)) {
                        Some(Handle::Body) => "move",
                        Some(Handle::Corner(i)) => if i % 2 == 0 { "nwse-resize" } else { "nesw-resize" },
                        Some(Handle::Edge(i)) => if i % 2 == 0 { "ns-resize" } else { "ew-resize" },
                        Some(Handle::Skew(i)) => if i % 2 == 0 { "col-resize" } else { "row-resize" },
                        None => "default",
                    };
                    v.set_cursor_from_name(Some(name));
                }
            });
            view.add_controller(motion);
        }
    }

    impl WidgetImpl for AnimView {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let (w, h) = self.size.get();
            let v = if orientation == gtk::Orientation::Horizontal { w } else { h };
            (v, v, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let anim = self.anim.borrow();
            let Some(anim) = anim.as_ref() else { return };
            let (w, h) = self.size.get();
            let (texture, _) = &anim.frames[self.frame.get() % anim.frames.len()];
            // Trilinear (mipmapped) filtering keeps shrunken GIFs smooth.
            let filter = if self.downscale.get() { gsk::ScalingFilter::Trilinear } else { gsk::ScalingFilter::Linear };
            let rect = graphene::Rect::new(0.0, 0.0, w as f32, h as f32);
            let xf = self.xf.get();
            let mask = self.mask.borrow();
            let mask_tex = mask.as_ref().map(|m| m.frames[self.mask_frame.get() % m.frames.len()].0.clone());
            if let Some(tex) = &mask_tex {
                snapshot.push_mask(gsk::MaskMode::Alpha);
                snapshot.save();
                snapshot.transform(Some(&transform(&xf, w, h)));
                snapshot.append_scaled_texture(tex, gsk::ScalingFilter::Linear, &rect);
                snapshot.restore();
                snapshot.pop();
            }
            snapshot.append_scaled_texture(texture, filter, &rect);
            if let Some(tex) = &mask_tex {
                snapshot.pop();
                // The mask itself, drawn over the widget as the user likes it.
                let opacity = self.mask_opacity.get();
                if opacity > 0.0 {
                    snapshot.push_opacity(opacity);
                    let colour = mask_xf::color(&xf);
                    if let Some((m, o)) = &colour {
                        let flat: Vec<f32> = (0..4).flat_map(|c| (0..4).map(move |r| m[r][c])).collect(); // column-major
                        snapshot.push_color_matrix(&graphene::Matrix::from_float(flat.try_into().unwrap()), &graphene::Vec4::new(o[0], o[1], o[2], o[3]));
                    }
                    snapshot.save();
                    snapshot.transform(Some(&transform(&xf, w, h)));
                    snapshot.append_scaled_texture(tex, gsk::ScalingFilter::Linear, &rect);
                    snapshot.restore();
                    if colour.is_some() {
                        snapshot.pop();
                    }
                    snapshot.pop();
                }
            }
            if self.edit.get() {
                self.draw_handles(snapshot, &xf, w, h);
            }
        }
    }

    impl AnimView {
        fn draw_handles(&self, snapshot: &gtk::Snapshot, xf: &MaskXf, w: i32, h: i32) {
            let cr = snapshot.append_cairo(&graphene::Rect::new(0.0, 0.0, w as f32, h as f32));
            let (wf, hf) = (w as f64, h as f64);
            let t = Affine::of(xf, wf, hf);
            let quad = [(0.0, 0.0), (wf, 0.0), (wf, hf), (0.0, hf)].map(|p| t.apply(p));
            cr.move_to(quad[0].0, quad[0].1);
            for q in &quad[1..] {
                cr.line_to(q.0, q.1);
            }
            cr.close_path();
            cr.set_line_width(3.0);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
            let _ = cr.stroke_preserve();
            cr.set_line_width(1.0);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
            let _ = cr.stroke();
            for (handle, (x, y)) in mask_xf::handles(xf, wf, hf) {
                match handle {
                    Handle::Skew(_) => cr.arc(x, y, 4.5, 0.0, std::f64::consts::TAU),
                    Handle::Corner(_) => cr.rectangle(x - 5.0, y - 5.0, 10.0, 10.0),
                    _ => cr.rectangle(x - 4.0, y - 4.0, 8.0, 8.0),
                }
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.fill_preserve();
                cr.set_line_width(1.5);
                if matches!(handle, Handle::Skew(_)) {
                    cr.set_source_rgb(0.9, 0.45, 0.1);
                } else {
                    cr.set_source_rgb(0.21, 0.52, 0.89);
                }
                let _ = cr.stroke();
            }
        }
    }
}

glib::wrapper! {
    pub struct AnimView(ObjectSubclass<imp::AnimView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl AnimView {
    pub fn new(anim: Rc<Anim>, width: i32, height: i32, mask: Option<Rc<Anim>>) -> Self {
        let view: Self = glib::Object::new();
        view.set_anim(anim, width, height, mask);
        view.set_halign(gtk::Align::Start);
        view.set_valign(gtk::Align::Start);
        view
    }

    /// Replace what is shown (the caller calls `play()` afterwards).
    pub fn set_anim(&self, anim: Rc<Anim>, width: i32, height: i32, mask: Option<Rc<Anim>>) {
        let imp = self.imp();
        imp.downscale.set(width < anim.width);
        imp.size.set((width, height));
        imp.frame.set(0);
        imp.generation.set(imp.generation.get().wrapping_add(1));
        imp.mask_frame.set(0);
        *imp.mask.borrow_mut() = mask;
        *imp.anim.borrow_mut() = Some(anim);
        self.queue_resize();
        self.queue_draw();
    }

    fn show_frame(&self, index: usize, mask: bool) {
        let imp = self.imp();
        if mask { imp.mask_frame.set(index) } else { imp.frame.set(index) }
        self.queue_draw();
    }

    /// How the mask is drawn: `opacity` 0 = only clips; `xf` places and colours it.
    pub fn set_mask_look(&self, opacity: f64, xf: MaskXf) {
        let imp = self.imp();
        imp.mask_opacity.set(opacity);
        if imp.grab.get().is_none() {
            imp.xf.set(xf);
        }
        self.queue_draw();
    }

    pub fn set_hooks(&self, on_xf: XfCb, on_exit: Rc<dyn Fn()>) {
        *self.imp().on_xf.borrow_mut() = Some(on_xf);
        *self.imp().on_exit.borrow_mut() = Some(on_exit);
    }

    /// Show / hide the transform handles (only meaningful with a mask).
    pub fn set_edit(&self, on: bool) {
        let imp = self.imp();
        let on = on && imp.mask.borrow().is_some();
        if imp.edit.replace(on) == on {
            return;
        }
        if on {
            *imp.idle_cursor.borrow_mut() = self.cursor().and_then(|c| c.name().map(|n| n.to_string()));
        } else {
            imp.grab.set(None);
            self.set_cursor_from_name(imp.idle_cursor.borrow().as_deref());
        }
        self.queue_draw();
    }

    /// Start looping. The timer chain stops by itself once the widget is gone.
    pub fn play(&self) {
        let imp = self.imp();
        let now = Instant::now();
        if let Some(a) = imp.anim.borrow().clone().filter(|a| a.is_animated()) {
            tick(self.downgrade(), a, now, imp.generation.get(), false);
        }
        if let Some(a) = imp.mask.borrow().clone().filter(|a| a.is_animated()) {
            tick(self.downgrade(), a, now, imp.generation.get(), true);
        }
    }
}

/// `started` is when the currently shown frame appeared.
fn tick(weak: glib::WeakRef<AnimView>, anim: Rc<Anim>, started: Instant, generation: u32, mask: bool) {
    let Some(view) = weak.upgrade() else { return };
    if view.imp().generation.get() != generation {
        return;
    }
    let n = anim.frames.len();
    let current = if mask { view.imp().mask_frame.get() } else { view.imp().frame.get() } % n;
    let now = Instant::now();
    // Anchoring to `started` avoids drift; if we are far behind (suspend, heavy load)
    // do not fast-forward through a backlog of frames.
    let due = (started + Duration::from_millis(anim.frames[current].1 as u64)).max(now);
    glib::timeout_add_local_once(due.saturating_duration_since(now), move || {
        let Some(view) = weak.upgrade() else { return };
        if view.imp().generation.get() != generation {
            return;
        }
        view.show_frame((current + 1) % n, mask);
        tick(weak, anim, due, generation, mask);
    });
}
