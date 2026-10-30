//! Monitors by connector name ("HDMI-1", "DP-2"), so a widget can come back to the screen it was on.
//!
//! A widget with a monitor stores its x/y relative to that monitor's top-left corner; widgets from
//! older files have none and keep global coordinates.

use gtk::gdk;
use gtk::prelude::*;

/// (connector name, x, y, width, height) of every monitor, in GDK's logical pixels.
pub type Mon = (String, i32, i32, i32, i32);

pub fn list() -> Vec<Mon> {
    let Some(display) = gdk::Display::default() else { return Vec::new() };
    display
        .monitors()
        .iter::<gdk::Monitor>()
        .flatten()
        .filter_map(|m| {
            let g = m.geometry();
            Some((m.connector()?.to_string(), g.x(), g.y(), g.width(), g.height()))
        })
        .collect()
}

/// The GDK monitor with this connector name.
pub fn find(name: &str) -> Option<gdk::Monitor> {
    gdk::Display::default()?.monitors().iter::<gdk::Monitor>().flatten().find(|m| m.connector().is_some_and(|c| c == name))
}

/// Global position of a widget at (`x`, `y`) on `monitor`. A monitor that is gone (unplugged)
/// falls back to the first one, so the widget does not end up off-screen.
pub fn to_global(monitor: Option<&str>, x: i32, y: i32) -> (i32, i32) {
    let Some(name) = monitor else { return (x, y) };
    let all = list();
    match all.iter().find(|m| m.0 == name).or(all.first()) {
        Some(m) => (m.1 + x, m.2 + y),
        None => (x, y),
    }
}

/// The monitor a global point is on (the nearest one when it is outside all of them).
pub fn locate(x: i32, y: i32) -> Option<Mon> {
    let dist = |m: &Mon| {
        let dx = (m.1 - x).max(0).max(x - (m.1 + m.3 - 1));
        let dy = (m.2 - y).max(0).max(y - (m.2 + m.4 - 1));
        dx as i64 * dx as i64 + dy as i64 * dy as i64
    };
    list().into_iter().min_by_key(dist)
}

/// Global position -> (monitor name, position relative to it).
pub fn from_global(x: i32, y: i32) -> (Option<String>, i32, i32) {
    match locate(x, y) {
        Some(m) => (Some(m.0.clone()), x - m.1, y - m.2),
        None => (None, x, y),
    }
}
