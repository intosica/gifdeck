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

use crate::blend::{self, BlendMode};
use crate::mask_xf::{self, Affine, Handle};
use crate::media::Anim;
use crate::model::{MaskXf, WidgetConfig};

/// Called with the new transform; `true` once the drag is finished.
pub type XfCb = Rc<dyn Fn(MaskXf, bool)>;

fn gsk_of(a: &Affine) -> gsk::Transform {
    gsk::Transform::new().matrix(&graphene::Matrix::from_2d(a.0, a.1, a.2, a.3, a.4, a.5))
}

/// The picture (`cw × ch`) turned by `deg` and centred in the widget's `w × h` allocation.
fn to_view(deg: f64, cw: f64, ch: f64, w: f64, h: f64) -> Affine {
    let mut a = mask_xf::spin(deg, cw, ch);
    a.4 += (w - cw) / 2.0;
    a.5 += (h - ch) / 2.0;
    a
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
        pub mask_alpha: Cell<f64>,
        pub speed: Cell<f64>,
        pub mask_speed: Cell<f64>,
        pub mask_blend: Cell<BlendMode>,
        pub blend: Cell<BlendMode>,
        pub mask_feather: Cell<f64>,
        pub feather: Cell<f64>,
        pub xf: Cell<MaskXf>,
        /// Rotation of the whole picture, in degrees.
        pub angle: Cell<f64>,
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
                    let p = match v.to_content((x, y)) {
                        Some(p) => p,
                        None => return,
                    };
                    match mask_xf::hit(&xf, w as f64, h as f64, p) {
                        Some(handle) => {
                            v.imp().grab.set(Some((handle, xf, p)));
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
                    let d = v.from_view((ox, oy));
                    v.imp().xf.set(mask_xf::drag(handle, &xf0, w as f64, h as f64, p0, d));
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
                    let Some(p) = v.to_content((x, y)) else { return };
                    let name = match mask_xf::hit(&v.imp().xf.get(), w as f64, h as f64, p) {
                        Some(Handle::Body) => "move",
                        Some(Handle::Corner(i)) => if i % 2 == 0 { "nwse-resize" } else { "nesw-resize" },
                        Some(Handle::Edge(i)) => if i % 2 == 0 { "ns-resize" } else { "ew-resize" },
                        Some(Handle::Skew(i)) => if i % 2 == 0 { "col-resize" } else { "row-resize" },
                        Some(Handle::Rotate) => "grabbing",
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
            let (w, h) = mask_xf::bbox(self.angle.get(), self.size.get().0 as f64, self.size.get().1 as f64);
            let v = if orientation == gtk::Orientation::Horizontal { w.round() as i32 } else { h.round() as i32 };
            (v, v, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let anim = self.anim.borrow();
            let Some(anim) = anim.as_ref() else { return };
            let (cw, ch) = self.size.get();
            let (vw, vh) = (self.obj().width() as f64, self.obj().height() as f64);
            let (vw, vh) = if vw < 1.0 || vh < 1.0 { mask_xf::bbox(self.angle.get(), cw as f64, ch as f64) } else { (vw, vh) };
            let view = to_view(self.angle.get(), cw as f64, ch as f64, vw, vh);
            let (texture, _) = &anim.frames[self.frame.get() % anim.frames.len()];
            // Trilinear (mipmapped) filtering keeps shrunken GIFs smooth.
            let filter = if self.downscale.get() { gsk::ScalingFilter::Trilinear } else { gsk::ScalingFilter::Linear };
            let rect = graphene::Rect::new(0.0, 0.0, cw as f32, ch as f32);
            let xf = self.xf.get();
            let mask = self.mask.borrow();
            let mask_tex = mask.as_ref().map(|m| m.frames[self.mask_frame.get() % m.frames.len()].0.clone());
            let mask_xf = gsk_of(&Affine::of(&xf, cw as f64, ch as f64));

            let k = self.mask_alpha.get();
            let (mask_feather, feather) = (self.mask_feather.get(), self.feather.get());
            // The mask image placed over the picture, softened by its feather.
            let put_mask = |s: &gtk::Snapshot, tex: &gtk::gdk::Texture| {
                if mask_feather > 0.0 {
                    s.push_blur(mask_feather);
                }
                s.save();
                s.transform(Some(&mask_xf));
                s.append_scaled_texture(tex, gsk::ScalingFilter::Linear, &rect);
                s.restore();
                if mask_feather > 0.0 {
                    s.pop();
                }
            };
            // The picture, clipped by the mask and with its own outline faded by `feather`.
            let picture = |s: &gtk::Snapshot| {
                if feather > 0.0 {
                    s.push_mask(gsk::MaskMode::Alpha);
                    s.push_blur(feather);
                    s.append_color(&gtk::gdk::RGBA::WHITE, &rect.inset_r(feather as f32, feather as f32));
                    s.pop();
                    s.pop();
                }
                if let Some(tex) = &mask_tex {
                    s.push_mask(gsk::MaskMode::Alpha);
                    // Overall strength k: clip alpha = (1-k) + k*mask, i.e. a cross-fade from "no clip" to the mask.
                    if k < 1.0 {
                        s.push_cross_fade(k);
                        s.append_color(&gtk::gdk::RGBA::BLACK, &rect);
                        s.pop();
                    }
                    put_mask(s, tex);
                    if k < 1.0 {
                        s.pop();
                    }
                    s.pop();
                }
                s.append_scaled_texture(texture, filter, &rect);
                if mask_tex.is_some() {
                    s.pop();
                }
                if feather > 0.0 {
                    s.pop();
                }
            };
            // The mask itself, drawn as the user likes it.
            let opacity = self.mask_opacity.get() * k;
            let overlay = |s: &gtk::Snapshot| {
                let Some(tex) = &mask_tex else { return };
                s.push_opacity(opacity);
                let colour = mask_xf::color(&xf);
                if let Some((m, o)) = &colour {
                    let flat: Vec<f32> = (0..4).flat_map(|c| (0..4).map(move |r| m[r][c])).collect(); // column-major
                    s.push_color_matrix(&graphene::Matrix::from_float(flat.try_into().unwrap()), &graphene::Vec4::new(o[0], o[1], o[2], o[3]));
                }
                put_mask(s, tex);
                if colour.is_some() {
                    s.pop();
                }
                s.pop();
            };

            snapshot.save();
            snapshot.transform(Some(&gsk_of(&view)));
            let renderer = self.obj().native().and_then(|n| n.renderer());
            if mask_tex.is_none() || opacity <= 0.0 {
                picture(snapshot);
            } else if self.blend.get() != BlendMode::Normal {
                blend::draw(snapshot, renderer, self.blend.get(), &rect, &overlay, &picture);
            } else if self.mask_blend.get() != BlendMode::Normal {
                blend::draw(snapshot, renderer, self.mask_blend.get(), &rect, &picture, &overlay);
            } else {
                picture(snapshot);
                overlay(snapshot);
            }
            if self.edit.get() {
                self.draw_handles(snapshot, &xf, cw, ch);
            }
            snapshot.restore();
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
                    Handle::Skew(_) | Handle::Rotate => cr.arc(x, y, 4.5, 0.0, std::f64::consts::TAU),
                    Handle::Corner(_) => cr.rectangle(x - 5.0, y - 5.0, 10.0, 10.0),
                    _ => cr.rectangle(x - 4.0, y - 4.0, 8.0, 8.0),
                }
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.fill_preserve();
                cr.set_line_width(1.5);
                if matches!(handle, Handle::Rotate) {
                    cr.set_source_rgb(0.55, 0.75, 0.15);
                } else if matches!(handle, Handle::Skew(_)) {
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

    /// Live settings: mask drawing (opacity 0 = only clips; `mask_xf` places and colours it,
    /// `mask_alpha` is its overall strength) and playback speeds. Speeds apply from the next frame.
    pub fn set_look(&self, cfg: &WidgetConfig) {
        let imp = self.imp();
        imp.mask_opacity.set(cfg.mask_opacity);
        imp.mask_alpha.set(cfg.mask_alpha);
        imp.speed.set(cfg.speed);
        imp.mask_speed.set(cfg.mask_speed);
        imp.mask_blend.set(cfg.mask_blend);
        imp.blend.set(cfg.blend);
        imp.mask_feather.set(cfg.mask_feather);
        imp.feather.set(cfg.feather);
        if imp.grab.get().is_none() {
            imp.xf.set(cfg.mask_xf);
        }
        self.queue_draw();
    }

    /// Rotation of the whole picture (degrees). The widget asks for the box a
    /// turned picture needs, so nothing is cropped.
    pub fn set_angle(&self, deg: f64) {
        if self.imp().angle.replace(deg) == deg {
            return;
        }
        self.queue_resize();
        self.queue_draw();
    }

    /// Pointer position in picture space (handles live there, so they turn with the picture).
    fn to_content(&self, p: (f64, f64)) -> Option<(f64, f64)> {
        let (cw, ch) = self.imp().size.get();
        let (vw, vh) = (self.width() as f64, self.height() as f64);
        to_view(self.imp().angle.get(), cw as f64, ch as f64, vw, vh).invert(p)
    }

    /// A pointer delta from view space to picture space.
    fn from_view(&self, d: (f64, f64)) -> (f64, f64) {
        let (cw, ch) = self.imp().size.get();
        let (vw, vh) = (self.width() as f64, self.height() as f64);
        to_view(self.imp().angle.get(), cw as f64, ch as f64, vw, vh).unrotate(d)
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
    let speed = if mask { view.imp().mask_speed.get() } else { view.imp().speed.get() }.max(0.01);
    let delay = Duration::from_secs_f64(anim.frames[current].1 as f64 / 1000.0 / speed);
    let due = (started + delay).max(now);
    glib::timeout_add_local_once(due.saturating_duration_since(now), move || {
        let Some(view) = weak.upgrade() else { return };
        if view.imp().generation.get() != generation {
            return;
        }
        view.show_frame((current + 1) % n, mask);
        tick(weak, anim, due, generation, mask);
    });
}
