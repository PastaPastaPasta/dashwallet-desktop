//! GTK registers the application on the session bus during `Builder::build`,
//! before the window exists, and asks `org.freedesktop.portal.Desktop` for
//! settings. When that name is not owned, the bus tries to activate
//! xdg-desktop-portal and the client waits out the failure: about 25 seconds,
//! which is the blank window. Owning the name ourselves and answering
//! `Settings.Read` at once skips the activation. A portal that is already
//! running keeps the name; this does nothing then.

use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;
use zbus::{DBusError, interface};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";

#[derive(Debug, DBusError)]
#[zbus(prefix = "org.freedesktop.portal.Error")]
enum PortalError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotFound(String),
}

struct Settings;

#[interface(name = "org.freedesktop.portal.Settings")]
impl Settings {
    fn read(&self, _namespace: &str, _key: &str) -> Result<OwnedValue, PortalError> {
        Err(PortalError::NotFound("no setting".into()))
    }

    #[zbus(signal)]
    async fn setting_changed(
        _emitter: &SignalEmitter<'_>,
        _namespace: &str,
        _key: &str,
        _value: OwnedValue,
    ) -> zbus::Result<()>;
}

/// If the desktop portal is not on the session bus, own its name and answer
/// the startup calls. Returns once the name is owned or the attempt is done,
/// so `Builder::build` does not race the first `Read`.
pub(crate) fn answer_if_portal_is_down() {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("dw-portal-stub".into())
        .spawn(move || {
            let held = serve();
            let _ = tx.send(held.is_some());
            if held.is_some() {
                // The connection has to stay up for the process: dropping it
                // releases the name, and a later GTK portal call would wait
                // on activation again.
                std::thread::park();
            }
        });
    if spawned.is_err() {
        return;
    }
    let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
}

fn serve() -> Option<zbus::blocking::Connection> {
    let conn = zbus::blocking::Connection::session().ok()?;
    // Register the object before the name, so a call cannot arrive first.
    conn.object_server().at(PATH, Settings).ok()?;
    let reply = conn
        .request_name_with_flags(PORTAL, zbus::fdo::RequestNameFlags::DoNotQueue.into())
        .ok()?;
    if reply != zbus::fdo::RequestNameReply::PrimaryOwner {
        return None;
    }
    eprintln!("dw-app: desktop portal is not running; answering its startup calls");
    Some(conn)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// A missing portal must not turn `Settings.Read` into a 25 second wait.
    /// A portal that is already up answers itself; either way the call returns.
    #[test]
    fn a_settings_read_returns_without_the_activation_timeout() {
        let Ok(conn) = zbus::blocking::Connection::session() else {
            return;
        };
        answer_if_portal_is_down();
        let proxy =
            zbus::blocking::Proxy::new(&conn, PORTAL, PATH, "org.freedesktop.portal.Settings")
                .unwrap();
        let started = Instant::now();
        let read: zbus::Result<OwnedValue> =
            proxy.call("Read", &("org.freedesktop.appearance", "color-scheme"));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "Settings.Read took {:?}: {read:?}",
            started.elapsed()
        );
    }
}
