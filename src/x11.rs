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

use crate::model::{Level, Win};

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
    client_list: u32,
    hidden: u32,
    gtk_app_id: u32,
    wm_desktop: u32,
    current_desktop: u32,
    number_of_desktops: u32,
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
        let client_list = atom("_NET_CLIENT_LIST")?;
        let hidden = atom("_NET_WM_STATE_HIDDEN")?;
        let gtk_app_id = atom("_GTK_APPLICATION_ID")?;
        let wm_desktop = atom("_NET_WM_DESKTOP")?;
        let current_desktop = atom("_NET_CURRENT_DESKTOP")?;
        let number_of_desktops = atom("_NET_NUMBER_OF_DESKTOPS")?;
        Some(Conn { conn, root, net_wm_state, above, below, sticky, skip_taskbar, skip_pager, frame_extents, client_list, hidden, gtk_app_id, wm_desktop, current_desktop, number_of_desktops })
    }

    /// Atoms that should be set for `level`; `sticky` = visible on every workspace.
    fn wanted(&self, level: Level, sticky: bool) -> Vec<u32> {
        let mut v = vec![self.skip_taskbar, self.skip_pager];
        match level {
            Level::Top => v.push(self.above),
            Level::Desktop => v.push(self.below),
            Level::Normal => {}
        }
        if sticky {
            v.push(self.sticky);
        }
        v
    }

    /// A root-window cardinal such as `_NET_CURRENT_DESKTOP`.
    fn root_cardinal(&self, atom: u32) -> Option<u32> {
        let r = self.conn.get_property(false, self.root, atom, AtomEnum::CARDINAL, 0, 1).ok()?.reply().ok()?;
        let v = r.value32()?.next();
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
///
/// `sticky`: on every workspace. `desktop`: the one workspace (0-based) it belongs to otherwise.
pub fn set_initial_state(xid: u32, level: Level, sticky: bool, desktop: Option<u32>) {
    with_conn(|c| {
        let atoms = c.wanted(level, sticky);
        let _ = c.conn.change_property32(PropMode::REPLACE, xid, c.net_wm_state, AtomEnum::ATOM, &atoms);
        if let (false, Some(d)) = (sticky, desktop) {
            let _ = c.conn.change_property32(PropMode::REPLACE, xid, c.wm_desktop, AtomEnum::CARDINAL, &[d]);
        }
        let _ = c.conn.flush();
    });
}

/// (Re)apply the state of a *mapped* window: add what `level` wants, remove the layer atoms it does not.
pub fn apply_state(xid: u32, level: Level, sticky: bool, desktop: Option<u32>) {
    with_conn(|c| {
        let wanted = c.wanted(level, sticky);
        for atom in [c.above, c.below, c.sticky] {
            if !wanted.contains(&atom) {
                c.send_state(xid, 0, atom);
            }
        }
        for atom in wanted {
            c.send_state(xid, 1, atom);
        }
        if let (false, Some(d)) = (sticky, desktop) {
            let ev = ClientMessageEvent::new(32, xid, c.wm_desktop, [d, 1, 0, 0, 0]);
            let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
            let _ = c.conn.send_event(false, c.root, mask, ev);
        }
        let _ = c.conn.flush();
    });
}

/// The workspace (0-based) the user is looking at.
pub fn current_desktop() -> Option<u32> {
    with_conn(|c| c.root_cardinal(c.current_desktop)).flatten()
}

/// How many workspaces the window manager has right now.
pub fn desktop_count() -> Option<u32> {
    with_conn(|c| c.root_cardinal(c.number_of_desktops)).flatten()
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

/// Every top-level window of the session (except our own), for application rules.
/// Empty when there is no X11 connection (Wayland offers no standard way to list windows).
pub fn windows() -> Vec<Win> {
    with_conn(|c| {
        let prop = |win: u32, property: u32, ty: AtomEnum| {
            c.conn.get_property(false, win, property, ty, 0, 1024).ok().and_then(|k| k.reply().ok()).map(|r| r.value).unwrap_or_default()
        };
        let list: Vec<u32> = c
            .conn
            .get_property(false, c.root, c.client_list, AtomEnum::WINDOW, 0, 4096)
            .ok()
            .and_then(|k| k.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default();
        list.into_iter()
            .filter_map(|win| {
                let text = |bytes: Vec<u8>| bytes.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).to_lowercase()).collect::<Vec<_>>();
                let mut keys = text(prop(win, AtomEnum::WM_CLASS.into(), AtomEnum::STRING));
                keys.extend(text(prop(win, c.gtk_app_id, AtomEnum::ANY)));
                if keys.iter().any(|k| k == "gifdeck" || k == "dev.gifdeck.gifdeck") {
                    return None;
                }
                let minimized = prop(win, c.net_wm_state, AtomEnum::ATOM)
                    .chunks_exact(4)
                    .any(|a| u32::from_ne_bytes([a[0], a[1], a[2], a[3]]) == c.hidden);
                Some(Win { keys, minimized })
            })
            .collect()
    })
    .unwrap_or_default()
}
