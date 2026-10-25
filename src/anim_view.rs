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

use crate::media::Anim;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct AnimView {
        pub anim: RefCell<Option<Rc<Anim>>>,
        pub frame: Cell<usize>,
        pub size: Cell<(i32, i32)>,
        pub downscale: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AnimView {
        const NAME: &'static str = "GifDeckAnimView";
        type Type = super::AnimView;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for AnimView {}

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
            snapshot.append_scaled_texture(texture, filter, &graphene::Rect::new(0.0, 0.0, w as f32, h as f32));
        }
    }
}

glib::wrapper! {
    pub struct AnimView(ObjectSubclass<imp::AnimView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl AnimView {
    pub fn new(anim: Rc<Anim>, width: i32, height: i32) -> Self {
        let view: Self = glib::Object::new();
        let imp = view.imp();
        imp.downscale.set(width < anim.width);
        imp.size.set((width, height));
        *imp.anim.borrow_mut() = Some(anim);
        view.set_halign(gtk::Align::Start);
        view.set_valign(gtk::Align::Start);
        view
    }

    fn show_frame(&self, index: usize) {
        self.imp().frame.set(index);
        self.queue_draw();
    }

    /// Start looping. The timer chain stops by itself once the widget is gone.
    pub fn play(&self) {
        let anim = match self.imp().anim.borrow().clone() {
            Some(a) if a.is_animated() => a,
            _ => return,
        };
        tick(self.downgrade(), anim, Instant::now());
    }
}

/// `started` is when the currently shown frame appeared.
fn tick(weak: glib::WeakRef<AnimView>, anim: Rc<Anim>, started: Instant) {
    let Some(view) = weak.upgrade() else { return };
    let n = anim.frames.len();
    let current = view.imp().frame.get() % n;
    let now = Instant::now();
    // Anchoring to `started` avoids drift; if we are far behind (suspend, heavy load)
    // do not fast-forward through a backlog of frames.
    let due = (started + Duration::from_millis(anim.frames[current].1 as u64)).max(now);
    glib::timeout_add_local_once(due.saturating_duration_since(now), move || {
        let Some(view) = weak.upgrade() else { return };
        view.show_frame((current + 1) % n);
        tick(weak, anim, due);
    });
}
