//! "Start when I log in".
//!
//! * **Flatpak**: the sandbox cannot write `~/.config/autostart`, so we ask the
//!   Background portal (`org.freedesktop.portal.Background.RequestBackground`).
//!   The portal shows its own confirmation where the desktop supports it and
//!   writes the autostart entry on the host. The answer arrives asynchronously
//!   on the Request object, so the UI is told whether it actually worked.
//! * **Native install**: write / remove `~/.config/autostart/<app-id>.desktop`.

use std::fs;
use std::path::PathBuf;

use gtk::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "dev.gifdeck.GifDeck";

pub fn is_sandboxed() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

fn autostart_file() -> PathBuf {
    glib::user_config_dir().join("autostart").join(format!("{APP_ID}.desktop"))
}

/// Ask the system to (not) start GifDeck at login. `done` receives the outcome.
pub fn set_autostart(enabled: bool, done: impl FnOnce(Result<(), String>) + 'static) {
    if is_sandboxed() {
        glib::spawn_future_local(async move { done(portal_request(enabled).await) });
    } else {
        done(write_desktop_entry(enabled));
    }
}

fn write_desktop_entry(enabled: bool) -> Result<(), String> {
    let path = autostart_file();
    if !enabled {
        return match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        };
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=GifDeck\nComment=Animated desktop widgets\nExec={} --background\nIcon={APP_ID}\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&path, entry).map_err(|e| e.to_string())
}

async fn portal_request(enabled: bool) -> Result<(), String> {
    let conn = gio::bus_get_future(gio::BusType::Session).await.map_err(|e| format!("no session bus: {e}"))?;

    // The Response signal is emitted on a Request object whose path we can
    // predict from our unique name and a token we choose. Subscribe *before*
    // calling so the reply cannot be missed.
    let token = format!("gifdeck{}", glib::random_int());
    let sender = conn.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_")).unwrap_or_default();
    let request_path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");

    let (tx, rx) = futures_channel::oneshot::channel::<Result<(), String>>();
    let tx = std::cell::RefCell::new(Some(tx));
    let _subscription = conn.subscribe_to_signal(
        Some("org.freedesktop.portal.Desktop"),
        Some("org.freedesktop.portal.Request"),
        Some("Response"),
        Some(&request_path),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let params = signal.parameters;
            let code = params.child_value(0).get::<u32>().unwrap_or(2);
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(match code {
                    0 => Ok(()),
                    1 => Err("the request was declined".to_string()),
                    _ => Err("the system refused to change autostart".to_string()),
                });
            }
        },
    );

    let options = glib::VariantDict::new(None);
    options.insert("handle_token", token.as_str());
    options.insert("reason", "Show your animated desktop widgets when you log in.");
    options.insert("autostart", enabled);
    options.insert("commandline", vec!["gifdeck", "--background"]);
    options.insert("dbus-activatable", false);
    let params = glib::Variant::tuple_from_iter(["".to_variant(), options.end()]);

    let call = conn
        .call_future(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Background",
            "RequestBackground",
            Some(&params),
            None,
            gio::DBusCallFlags::NONE,
            10_000,
        )
        .await;

    let result = match call {
        Err(e) => Err(format!("portal unavailable: {e}")),
        // Wait (bounded) for the Response signal.
        Ok(_) => match glib::future_with_timeout(std::time::Duration::from_secs(120), rx).await {
            Ok(Ok(r)) => r,
            _ => Err("no answer from the portal".to_string()),
        },
    };
    result
}
