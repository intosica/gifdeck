//! Global preset hotkeys on X11: passive key grabs on the root window, read from the same
//! process through the main loop (no extra thread).
//!
//! Wayland has no way for a client to grab keys globally. There the hotkey works while the
//! manager window is focused (app accelerators), and any desktop shortcut can run
//! `gifdeck --preset "Name"`, which is forwarded to the running instance.

use std::cell::RefCell;
use std::os::fd::AsRawFd;
use std::rc::Rc;

use gtk::{gdk, glib};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, GrabMode, ModMask};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

/// CapsLock and NumLock must not stop a hotkey from working: grab every combination of them.
const LOCK: u16 = 1 << 1;
const MOD2: u16 = 1 << 4;

pub struct Hotkeys {
    conn: RustConnection,
    root: u32,
    min_keycode: u8,
    per_keycode: usize,
    keysyms: Vec<u32>,
    /// (keycode, modifiers, preset id) of every active grab.
    binds: RefCell<Vec<(u8, u16, String)>>,
}

/// GDK modifiers as the X11 modifier mask.
fn x_mods(m: gdk::ModifierType) -> u16 {
    let mut x = 0;
    for (g, bit) in [(gdk::ModifierType::SHIFT_MASK, 1), (gdk::ModifierType::CONTROL_MASK, 4), (gdk::ModifierType::ALT_MASK, 8), (gdk::ModifierType::SUPER_MASK, 64)] {
        if m.contains(g) {
            x |= bit;
        }
    }
    x
}

impl Hotkeys {
    /// `on_press` gets the preset id of the hotkey that was pressed.
    pub fn new(on_press: impl Fn(&str) + 'static) -> Option<Rc<Hotkeys>> {
        let (conn, screen) = RustConnection::connect(None).ok()?;
        let setup = conn.setup();
        let root = setup.roots.get(screen)?.root;
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let map = conn.get_keyboard_mapping(min, max - min + 1).ok()?.reply().ok()?;
        let fd = conn.stream().as_raw_fd();
        let hk = Rc::new(Hotkeys { conn, root, min_keycode: min, per_keycode: map.keysyms_per_keycode as usize, keysyms: map.keysyms, binds: RefCell::new(Vec::new()) });
        let weak = Rc::downgrade(&hk);
        glib::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| {
            let Some(hk) = weak.upgrade() else { return glib::ControlFlow::Break };
            while let Ok(Some(ev)) = hk.conn.poll_for_event() {
                if let Event::KeyPress(k) = ev {
                    let mods = u16::from(k.state) & !(LOCK | MOD2);
                    let hit = hk.binds.borrow().iter().find(|(c, m, _)| *c == k.detail && *m == mods).map(|b| b.2.clone());
                    if let Some(id) = hit {
                        on_press(&id);
                    }
                }
            }
            glib::ControlFlow::Continue
        });
        Some(hk)
    }

    fn keycode_of(&self, keysym: u32) -> Option<u8> {
        let i = self.keysyms.chunks(self.per_keycode.max(1)).position(|syms| syms.contains(&keysym))?;
        u8::try_from(i + self.min_keycode as usize).ok()
    }

    /// Replace all grabs by `(preset id, accelerator)`. Returns the ids that could not be grabbed
    /// (unparsable, or another program already owns the key).
    pub fn set(&self, wanted: &[(String, String)]) -> Vec<String> {
        for (code, mods, _) in self.binds.borrow_mut().drain(..) {
            for lock in [0, LOCK, MOD2, LOCK | MOD2] {
                let _ = self.conn.ungrab_key(code, self.root, ModMask::from(mods | lock));
            }
        }
        let mut failed = Vec::new();
        for (id, accel) in wanted {
            let grab = gtk::accelerator_parse(accel).and_then(|(key, mods)| {
                let (code, mods) = (self.keycode_of(glib::translate::IntoGlib::into_glib(key))?, x_mods(mods));
                for lock in [0, LOCK, MOD2, LOCK | MOD2] {
                    let cookie = self.conn.grab_key(true, self.root, ModMask::from(mods | lock), code, GrabMode::ASYNC, GrabMode::ASYNC).ok()?;
                    cookie.check().ok()?;
                }
                Some((code, mods))
            });
            match grab {
                Some((code, mods)) => self.binds.borrow_mut().push((code, mods, id.clone())),
                None => failed.push(id.clone()),
            }
        }
        let _ = self.conn.flush();
        failed
    }
}
