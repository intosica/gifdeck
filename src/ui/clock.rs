//! Two ways to pick a schedule time without number boxes:
//! a digital clock you drag up and down, and an analog clock you sweep with the mouse.

use std::cell::{Cell, RefCell};
use std::f64::consts::{PI, TAU};
use std::rc::Rc;

use gtk::prelude::*;

use crate::ctx::AppCtx;
use crate::model::{fmt_time, Rule};

pub type Edit = Rc<dyn Fn(Box<dyn FnOnce(&mut Rule)>)>;

const BLUE: (f64, f64, f64) = (0.21, 0.52, 0.89);
const RED: (f64, f64, f64) = (0.88, 0.11, 0.14);
const GREY: (f64, f64, f64) = (0.6, 0.6, 0.6);

// ------------------------------------------------------------------ digital clock

#[derive(Clone, Copy, PartialEq, Debug)]
enum Seg {
    Hours,
    Minutes,
    AmPm,
}

/// The value after dragging `dy` pixels (negative = up = later) on `seg`, starting from `start` minutes.
fn dragged(seg: Seg, start: u16, dy: f64) -> u16 {
    let (per_px, step) = match seg {
        Seg::Hours => (12.0, 60),
        Seg::Minutes => (4.0, 1),
        Seg::AmPm => (24.0, 720),
    };
    let n = (-dy / per_px).round() as i32;
    match seg {
        // minutes roll over inside their hour
        Seg::Minutes => (start / 60 * 60) + (i32::from(start % 60) + n).rem_euclid(60) as u16,
        _ => (i32::from(start) + n * step).rem_euclid(1440) as u16,
    }
}

/// "4 : 05 PM" as blue numbers; click on hours, minutes or AM/PM and drag up or down.
pub fn digital(minutes: u16, h24: bool, on_change: impl Fn(u16) + 'static) -> gtk::Box {
    let value = Rc::new(Cell::new(minutes));
    let label = |text: &str| gtk::Label::builder().label(text).css_classes(["gifdeck-digital"]).build();
    let (hours, colon, mins, ampm) = (label(""), label(":"), label(""), label(""));
    let show = {
        let (hours, mins, ampm) = (hours.clone(), mins.clone(), ampm.clone());
        move |m: u16| {
            let h = m / 60;
            hours.set_text(&if h24 { format!("{h:02}") } else { format!("{}", if h % 12 == 0 { 12 } else { h % 12 }) });
            mins.set_text(&format!("{:02}", m % 60));
            ampm.set_text(if h < 12 { "AM" } else { "PM" });
        }
    };
    show(minutes);
    let b = gtk::Box::builder().valign(gtk::Align::Center).tooltip_text("Drag up or down on the hours, minutes or AM / PM").build();
    b.set_cursor_from_name(Some("ns-resize"));
    for l in [&hours, &colon, &mins] {
        b.append(l);
    }
    if !h24 {
        b.append(&ampm);
    }

    let drag = gtk::GestureDrag::new();
    let state: Rc<Cell<(Seg, u16)>> = Rc::new(Cell::new((Seg::Hours, minutes)));
    {
        let (value, state, b2) = (value.clone(), state.clone(), b.clone());
        let (mw, aw) = (mins.clone().upcast::<gtk::Widget>(), ampm.clone().upcast::<gtk::Widget>());
        drag.connect_drag_begin(move |g, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            let hit = std::iter::successors(b2.pick(x, y, gtk::PickFlags::DEFAULT), |w| w.parent()).find(|w| *w == mw || *w == aw);
            let seg = match hit {
                Some(w) if w == mw => Seg::Minutes,
                Some(_) => Seg::AmPm,
                None => Seg::Hours,
            };
            state.set((seg, value.get()));
        });
    }
    {
        let (value, state) = (value.clone(), state.clone());
        drag.connect_drag_update(move |_, _, dy| {
            let (seg, start) = state.get();
            let m = dragged(seg, start, dy);
            if m != value.get() {
                value.set(m);
                show(m);
                on_change(m);
            }
        });
    }
    b.add_controller(drag);
    b
}

// ------------------------------------------------------------------ analog clock

/// Minute (0..720, fractional) on the 12-hour dial at offset (dx, dy) from its centre.
fn minute_at(dx: f64, dy: f64) -> f64 {
    dx.atan2(-dy).rem_euclid(TAU) / TAU * 720.0
}

/// A sweep of `span` minutes (negative = counter-clockwise) from `start_abs` becomes (start, end) of a rule.
fn range_of(start_abs: i32, span: i32) -> (u16, u16) {
    let (lo, hi) = if span >= 0 { (start_abs, start_abs + span) } else { (start_abs + span, start_abs) };
    (lo.rem_euclid(1440) as u16, hi.rem_euclid(1440) as u16)
}

/// The part(s) of a rule's window that fall into the half-day `[half, half + 720)`, as (from, to) minutes.
fn visible(r: &Rule, half: i32) -> Vec<(f64, f64)> {
    let (s, e) = if r.all_day || r.start == r.end {
        (0, 1440)
    } else if r.start < r.end {
        (i32::from(r.start), i32::from(r.end))
    } else {
        (i32::from(r.start), i32::from(r.end) + 1440)
    };
    [0, -1440]
        .iter()
        .filter_map(|k| {
            let (a, b) = ((s + k).max(half), (e + k).min(half + 720));
            (a < b).then_some((f64::from(a), f64::from(b)))
        })
        .collect()
}

#[derive(Default)]
struct Sweep {
    start_abs: i32,
    prev: f64,
    total: f64,
}

/// Dial that shows one half of the day. Press where the window starts and hold while sweeping
/// clockwise or counter-clockwise to where it ends. Returns the whole widget and its dial (redraw it
/// when other rules change).
pub fn analog(ctx: &Rc<AppCtx>, rule: &Rule, h24: bool, edit: Edit) -> (gtk::Box, gtk::DrawingArea) {
    const SIZE: i32 = 124;
    let id = rule.id.clone();
    let half = Rc::new(Cell::new(if rule.all_day { 0 } else { i32::from(rule.start) / 720 * 720 }));
    let area = gtk::DrawingArea::builder().content_width(SIZE).content_height(SIZE).halign(gtk::Align::Center).build();
    area.set_cursor_from_name(Some("crosshair"));

    {
        let (ctx, id, half) = (ctx.clone(), id.clone(), half.clone());
        area.set_draw_func(move |a, cr, w, h| {
            let st = ctx.state.borrow();
            let Some(me) = st.rules.iter().find(|r| r.id == id) else { return };
            let fg = a.color();
            let (fg, half) = ((f64::from(fg.red()), f64::from(fg.green()), f64::from(fg.blue())), half.get());
            let (cx, cy, r) = (f64::from(w) / 2.0, f64::from(h) / 2.0, 44.0);
            let pt = |m: f64| (m - f64::from(half)) / 720.0 * TAU - PI / 2.0;
            let arc = |from: f64, to: f64, rad: f64| {
                if to - from >= 720.0 {
                    cr.new_sub_path();
                    cr.arc(cx, cy, rad, 0.0, TAU);
                } else {
                    cr.arc(cx, cy, rad, pt(from), pt(to));
                }
            };
            let rgba = |c: (f64, f64, f64), a: f64| cr.set_source_rgba(c.0, c.1, c.2, a);

            // face
            rgba(fg, 0.07);
            cr.arc(cx, cy, r, 0.0, TAU);
            let _ = cr.fill();
            rgba(fg, 0.35);
            cr.set_line_width(1.0);
            cr.arc(cx, cy, r, 0.0, TAU);
            let _ = cr.stroke();
            for i in 0..12 {
                let t = f64::from(i) / 12.0 * TAU;
                let len = if i % 3 == 0 { 7.0 } else { 3.5 };
                cr.move_to(cx + t.sin() * (r - len), cy - t.cos() * (r - len));
                cr.line_to(cx + t.sin() * r, cy - t.cos() * r);
                let _ = cr.stroke();
            }
            cr.set_font_size(10.0);
            for k in 0..4 {
                let hr = half / 60 + 3 * k;
                let text = if h24 { hr.to_string() } else { (if hr % 12 == 0 { 12 } else { hr % 12 }).to_string() };
                let ext = cr.text_extents(&text).ok();
                let (tw, th) = ext.map_or((0.0, 0.0), |e| (e.width(), e.height()));
                let t = f64::from(k) / 4.0 * TAU;
                cr.move_to(cx + t.sin() * (r - 16.0) - tw / 2.0, cy - t.cos() * (r - 16.0) + th / 2.0);
                let _ = cr.show_text(&text);
            }

            // other rules that run at the same time: grey, red when they clash
            let mut clash = false;
            for o in st.rules.iter().filter(|o| o.id != me.id && me.overlaps(o)) {
                let bad = me.conflicts(o);
                clash |= bad;
                rgba(if bad { RED } else { GREY }, if bad { 0.9 } else { 0.8 });
                cr.set_line_width(5.0);
                for (a, b) in visible(o, half) {
                    arc(a, b, r + 7.0);
                    let _ = cr.stroke();
                }
            }

            // this rule
            let own = if clash { RED } else { BLUE };
            for (a, b) in visible(me, half) {
                rgba(own, 0.28);
                cr.move_to(cx, cy);
                arc(a, b, r);
                cr.close_path();
                let _ = cr.fill();
                rgba(own, 1.0);
                cr.set_line_width(3.0);
                arc(a, b, r - 1.5);
                let _ = cr.stroke();
            }
        });
    }

    let summary = gtk::Label::builder().css_classes(["gifdeck-digital"]).halign(gtk::Align::Center).build();
    let show = {
        let summary = summary.clone();
        move |s: u16, e: u16| summary.set_text(&format!("{} – {}", fmt_time(s, h24), fmt_time(e, h24)))
    };
    show(rule.start, rule.end);

    // AM / PM (or the two halves of a 24-hour day)
    let (first, second) = if h24 { ("00–12", "12–24") } else { ("AM", "PM") };
    let am = gtk::ToggleButton::builder().label(first).active(half.get() == 0).build();
    let pm = gtk::ToggleButton::builder().label(second).active(half.get() != 0).group(&am).build();
    for (btn, h) in [(&am, 0), (&pm, 720)] {
        let (half, area) = (half.clone(), area.clone());
        btn.connect_toggled(move |b| {
            if b.is_active() {
                half.set(h);
                area.queue_draw();
            }
        });
    }
    let halves = gtk::Box::builder().css_classes(["linked"]).halign(gtk::Align::Center).build();
    halves.append(&am);
    halves.append(&pm);

    // press + hold + sweep
    let sweep = Rc::new(RefCell::new(Sweep::default()));
    let origin = Rc::new(Cell::new((0.0, 0.0)));
    let drag = gtk::GestureDrag::new();
    {
        let (sweep, origin, half, area) = (sweep.clone(), origin.clone(), half.clone(), area.clone());
        drag.connect_drag_begin(move |g, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            origin.set((x, y));
            let m = (minute_at(x - f64::from(area.width()) / 2.0, y - f64::from(area.height()) / 2.0) / 5.0).round() * 5.0 % 720.0;
            *sweep.borrow_mut() = Sweep { start_abs: half.get() + m as i32, prev: m, total: 0.0 };
        });
    }
    {
        let (half, area, last) = (half.clone(), area.clone(), Cell::new((0u16, 0u16)));
        let (am, pm) = (am.clone(), pm.clone());
        drag.connect_drag_update(move |_, ox, oy| {
            let (x, y) = (origin.get().0 + ox - f64::from(area.width()) / 2.0, origin.get().1 + oy - f64::from(area.height()) / 2.0);
            if x.hypot(y) < 44.0 * 0.15 {
                return; // the angle is meaningless this close to the centre
            }
            let mut s = sweep.borrow_mut();
            let cur = minute_at(x, y);
            let mut d = cur - s.prev;
            if d > 360.0 {
                d -= 720.0;
            } else if d < -360.0 {
                d += 720.0;
            }
            s.prev = cur;
            s.total = (s.total + d).clamp(-1435.0, 1435.0);
            let span = (s.total / 5.0).round() as i32 * 5;
            if span == 0 {
                return;
            }
            let (start, end) = range_of(s.start_abs, span);
            // follow the pointer into the other half of the day
            let now_half = (s.start_abs + span).rem_euclid(1440) / 720 * 720;
            if now_half != half.get() {
                (if now_half == 0 { &am } else { &pm }).set_active(true);
            }
            if last.get() != (start, end) {
                last.set((start, end));
                show(start, end);
                edit(Box::new(move |r| {
                    r.start = start;
                    r.end = end;
                }));
                area.queue_draw();
            }
        });
    }
    area.add_controller(drag);

    let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).valign(gtk::Align::Center).tooltip_text("Press where the window starts, hold and sweep clockwise or counter-clockwise to where it ends").build();
    b.append(&area);
    b.append(&halves);
    b.append(&summary);
    (b, area)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digital_drag() {
        assert_eq!(dragged(Seg::Hours, 9 * 60 + 30, -24.0), 11 * 60 + 30); // up 24px: +2 h
        assert_eq!(dragged(Seg::Hours, 30, 12.0), 23 * 60 + 30); // down below midnight wraps
        assert_eq!(dragged(Seg::Minutes, 9 * 60 + 58, -8.0), 9 * 60); // rolls over inside the hour
        assert_eq!(dragged(Seg::AmPm, 9 * 60, -24.0), 21 * 60);
    }

    #[test]
    fn dial_angles() {
        assert_eq!(minute_at(0.0, -1.0), 0.0); // 12 o'clock
        assert!((minute_at(1.0, 0.0) - 180.0).abs() < 1e-9); // 3 o'clock = 3 h
        assert!((minute_at(0.0, 1.0) - 360.0).abs() < 1e-9);
        assert!((minute_at(-1.0, 0.0) - 540.0).abs() < 1e-9);
    }

    #[test]
    fn sweep_becomes_a_range() {
        assert_eq!(range_of(540, 480), (540, 1020)); // clockwise 9:00 → 17:00
        assert_eq!(range_of(540, -60), (480, 540)); // counter-clockwise
        assert_eq!(range_of(0, -30), (1410, 0)); // across midnight
    }

    #[test]
    fn window_is_clipped_to_the_half_day() {
        let r = Rule { start: 22 * 60, end: 60, all_day: false, ..Default::default() };
        assert_eq!(visible(&r, 0), vec![(0.0, 60.0)]);
        assert_eq!(visible(&r, 720), vec![(1320.0, 1440.0)]);
    }
}
