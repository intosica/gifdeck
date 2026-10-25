//! X11 / Xorg window layering through EWMH hints.
//!
//! GTK4 removed `set_keep_above()` & friends, so on X11 we talk to the window
//! manager directly with a *second connection from the same process* (no extra
//! process is involved). Spec: <https://specifications.freedesktop.org/wm-spec/latest/>
//!
//! * Always on top  -> `_NET_WM_STATE_ABOVE`
//! * Desktop layer  -> `_NET_WM_STATE_BELOW` (+ sticky, so it shows on every workspace)
//! * all widgets    -> `_NET_WM_STATE_SKIP_TASKBAR` / `SKIP_PAGER`

use std::rc::Rc;

use gtk::prelude::*;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask, PropMode, StackMode,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::model::Level;

struct Conn {
    conn: RustConnection,
    root: u32,
    net_wm_state: u32,
    above: u32,
    below: u32,
    sticky: u32,
    skip_taskbar: u32,
    skip_pager: u32,
    frame_extents: u32,
}

impl Conn {
    fn open() -> Option<Conn> {
        let (conn, screen) = RustConnection::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
        let net_wm_state = atom("_NET_WM_STATE")?;
        let above = atom("_NET_WM_STATE_ABOVE")?;
        let below = atom("_NET_WM_STATE_BELOW")?;
        let sticky = atom("_NET_WM_STATE_STICKY")?;
        let skip_taskbar = atom("_NET_WM_STATE_SKIP_TASKBAR")?;
        let skip_pager = atom("_NET_WM_STATE_SKIP_PAGER")?;
        let frame_extents = atom("_NET_FRAME_EXTENTS")?;
        Some(Conn { conn, root, net_wm_state, above, below, sticky, skip_taskbar, skip_pager, frame_extents })
    }

    /// Atoms that should be set for `level`.
    fn wanted(&self, level: Level) -> Vec<u32> {
        let mut v = vec![self.skip_taskbar, self.skip_pager];
        match level {
            Level::Top => v.extend([self.above, self.sticky]),
            Level::Desktop => v.extend([self.below, self.sticky]),
            Level::Normal => {}
        }
        v
    }

    /// `_NET_WM_STATE` client message (action: 0 = remove, 1 = add).
    fn send_state(&self, xid: u32, action: u32, atom: u32) {
        let ev = ClientMessageEvent::new(32, xid, self.net_wm_state, [action, atom, 0, 1, 0]);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let _ = self.conn.send_event(false, self.root, mask, ev);
    }
}

thread_local! {
    static CONN: Option<Rc<Conn>> = Conn::open().map(Rc::new);
}

fn with_conn<R>(f: impl FnOnce(&Conn) -> R) -> Option<R> {
    CONN.with(|c| c.as_ref().map(|c| f(c)))
}

/// X11 window id of a realised GTK window (None on Wayland).
pub fn xid_of(window: &gtk::Window) -> Option<u32> {
    let surface = window.surface()?;
    let x = surface.downcast_ref::<gdk4_x11::X11Surface>()?;
    Some(x.xid() as u32)
}

/// Set the initial `_NET_WM_STATE` *before* the window is mapped, so the
/// window manager puts it into the right layer from the first frame.
pub fn set_initial_state(xid: u32, level: Level) {
    with_conn(|c| {
        let atoms = c.wanted(level);
        let _ = c.conn.change_property32(PropMode::REPLACE, xid, c.net_wm_state, AtomEnum::ATOM, &atoms);
        let _ = c.conn.flush();
    });
}

/// (Re)apply the state of a *mapped* window: add what `level` wants, remove the layer atoms it does not.
pub fn apply_state(xid: u32, level: Level) {
    with_conn(|c| {
        let wanted = c.wanted(level);
        for atom in [c.above, c.below, c.sticky] {
            if !wanted.contains(&atom) {
                c.send_state(xid, 0, atom);
            }
        }
        for atom in wanted {
            c.send_state(xid, 1, atom);
        }
        let _ = c.conn.flush();
    });
}

pub fn move_to(xid: u32, x: i32, y: i32) {
    with_conn(|c| {
        let _ = c.conn.configure_window(xid, &ConfigureWindowAux::new().x(x).y(y));
        let _ = c.conn.flush();
    });
}

/// Raise within its layer. Calling this for widgets from back to front yields the user's priority order.
pub fn raise(xid: u32) {
    with_conn(|c| {
        let _ = c.conn.configure_window(xid, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
        let _ = c.conn.flush();
    });
}

/// Position of the window *including* the window manager's frame (title bar, border).
///
/// `move_to` hands the same coordinates to the window manager, which treats
/// them as the frame's top-left corner. Using the client-area position here
/// instead would make decorated widgets creep by the title-bar height on every restart.
pub fn position(xid: u32) -> Option<(i32, i32)> {
    with_conn(|c| {
        let r = c.conn.translate_coordinates(xid, c.root, 0, 0).ok()?.reply().ok()?;
        // _NET_FRAME_EXTENTS = left, right, top, bottom (absent for undecorated windows).
        let (left, top) = c
            .conn
            .get_property(false, xid, c.frame_extents, AtomEnum::CARDINAL, 0, 4)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().map(|v| v.collect::<Vec<u32>>()))
            .filter(|v| v.len() == 4)
            .map(|v| (v[0] as i32, v[2] as i32))
            .unwrap_or((0, 0));
        Some((r.dst_x as i32 - left, r.dst_y as i32 - top))
    })
    .flatten()
}
