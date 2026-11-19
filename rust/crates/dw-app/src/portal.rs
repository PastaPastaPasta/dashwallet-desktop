//! GTK's application startup builds a D-Bus proxy for
//! `org.freedesktop.portal.Desktop` (`Inhibit`) with auto-start, and Tao's
//! window creation reads `org.freedesktop.portal.Settings` with auto-start.
//! When nothing owns that name, the bus tries to activate
//! `xdg-desktop-portal` and those calls sit until activation or client
//! timeouts fail (~25 s in GTK + 5 s in Tao), before the window can paint.
//!
//! This does not claim any bus name. Before the window, a probe asks the bus
//! whether the portal is owned and, if not, asks it to start the portal, for
//! at most [`PROBE_CAP`]. A portal that is up by then is used as usual, so
//! its colour scheme reaches the window. Otherwise, for the rest of the
//! process, its calls to `Inhibit` and `Settings` set no-auto-start and fail
//! at once instead of waiting on an activation. Other interfaces (such as
//! `FileChooser`) and other processes are untouched.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const STARTUP_INTERFACES: &[&str] = &[
    "org.freedesktop.portal.Inhibit",
    "org.freedesktop.portal.Settings",
];
/// How long startup waits for the portal to be owned or activated.
const PROBE_CAP: Duration = Duration::from_millis(500);
/// `G_DBUS_PROXY_FLAGS_DO_NOT_AUTO_START`.
const DO_NOT_AUTO_START: u32 = 1 << 2;

static SKIP_AUTOSTART: AtomicBool = AtomicBool::new(false);

pub(crate) fn should_skip_autostart(
    destination: Option<&str>,
    interface: Option<&str>,
    skip_autostart: bool,
) -> bool {
    skip_autostart
        && destination == Some(PORTAL)
        && interface.is_some_and(|iface| STARTUP_INTERFACES.contains(&iface))
}

pub(crate) fn proxy_flags(
    name: Option<&str>,
    interface: Option<&str>,
    flags: u32,
    skip_autostart: bool,
) -> u32 {
    if should_skip_autostart(name, interface, skip_autostart) {
        flags | DO_NOT_AUTO_START
    } else {
        flags
    }
}

pub(crate) fn skip_portal_autostart_if_unavailable() {
    skip_portal_autostart_unless_up_within(PROBE_CAP);
}

/// An activation still running at `cap` carries on inside the bus; this
/// process stops waiting for it.
fn skip_portal_autostart_unless_up_within(cap: Duration) {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("dw-portal-check".into())
        .spawn(move || {
            let _ = tx.send(portal_is_up());
        });
    let up = spawned.is_ok() && rx.recv_timeout(cap).unwrap_or(false);
    SKIP_AUTOSTART.store(!up, Ordering::SeqCst);
    if !up {
        eprintln!("dw-app: desktop portal did not come up; not asking the bus to start it again");
    }
}

fn portal_is_up() -> bool {
    let Ok(conn) = zbus::blocking::Connection::session() else {
        return false;
    };
    portal_is_owned_on(&conn) || portal_starts_on(&conn)
}

fn portal_starts_on(conn: &zbus::blocking::Connection) -> bool {
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "StartServiceByName",
        &(PORTAL, 0u32),
    )
    .is_ok()
}

fn portal_is_owned_on(conn: &zbus::blocking::Connection) -> bool {
    let Ok(reply) = conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &(PORTAL,),
    ) else {
        return false;
    };
    reply.body().deserialize().unwrap_or(false)
}

type NewSync = unsafe extern "C" fn(
    *mut c_void,
    u32,
    *mut c_void,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut c_void,
    *mut *mut c_void,
) -> *mut c_void;

type NewForBusSync = unsafe extern "C" fn(
    c_int,
    u32,
    *mut c_void,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut c_void,
    *mut *mut c_void,
) -> *mut c_void;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

unsafe extern "C" {
    fn __real_dbus_connection_send_with_reply_and_block(
        connection: *mut c_void,
        message: *mut c_void,
        timeout_milliseconds: c_int,
        error: *mut c_void,
    ) -> *mut c_void;
    fn dbus_message_get_destination(message: *mut c_void) -> *const c_char;
    fn dbus_message_get_interface(message: *mut c_void) -> *const c_char;
    fn dbus_message_set_auto_start(message: *mut c_void, auto_start: u32);
}

/// The next definition of `symbol` after this executable's. Aborts with a
/// message if there is none: GTK links libgio, so that cannot happen.
fn next<T>(symbol: &CStr) -> T {
    let ptr = unsafe { dlsym(-1isize as *mut c_void, symbol.as_ptr()) };
    if ptr.is_null() {
        eprintln!(
            "dw-app: {} not found after the executable; aborting",
            symbol.to_string_lossy()
        );
        std::process::abort();
    }
    unsafe { std::mem::transmute_copy(&ptr) }
}

unsafe fn str_of<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(ptr) }.to_str().ok()
    }
}

unsafe fn flags_for(name: *const c_char, interface: *const c_char, flags: u32) -> u32 {
    let skip = SKIP_AUTOSTART.load(Ordering::SeqCst);
    if !skip {
        return flags;
    }
    proxy_flags(
        unsafe { str_of(name) },
        unsafe { str_of(interface) },
        flags,
        true,
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn g_dbus_proxy_new_sync(
    connection: *mut c_void,
    flags: u32,
    info: *mut c_void,
    name: *const c_char,
    object_path: *const c_char,
    interface: *const c_char,
    cancellable: *mut c_void,
    error: *mut *mut c_void,
) -> *mut c_void {
    static REAL: OnceLock<NewSync> = OnceLock::new();
    let real = *REAL.get_or_init(|| next(c"g_dbus_proxy_new_sync"));
    unsafe {
        real(
            connection,
            flags_for(name, interface, flags),
            info,
            name,
            object_path,
            interface,
            cancellable,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn g_dbus_proxy_new_for_bus_sync(
    bus_type: c_int,
    flags: u32,
    info: *mut c_void,
    name: *const c_char,
    object_path: *const c_char,
    interface: *const c_char,
    cancellable: *mut c_void,
    error: *mut *mut c_void,
) -> *mut c_void {
    static REAL: OnceLock<NewForBusSync> = OnceLock::new();
    let real = *REAL.get_or_init(|| next(c"g_dbus_proxy_new_for_bus_sync"));
    unsafe {
        real(
            bus_type,
            flags_for(name, interface, flags),
            info,
            name,
            object_path,
            interface,
            cancellable,
            error,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __wrap_dbus_connection_send_with_reply_and_block(
    connection: *mut c_void,
    message: *mut c_void,
    timeout_milliseconds: c_int,
    error: *mut c_void,
) -> *mut c_void {
    if !message.is_null() && SKIP_AUTOSTART.load(Ordering::SeqCst) {
        let dest = unsafe { str_of(dbus_message_get_destination(message)) };
        let iface = unsafe { str_of(dbus_message_get_interface(message)) };
        if should_skip_autostart(dest, iface, true) {
            unsafe { dbus_message_set_auto_start(message, 0) };
        }
    }
    unsafe {
        __real_dbus_connection_send_with_reply_and_block(
            connection,
            message,
            timeout_milliseconds,
            error,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;

    use zbus::interface;
    use zbus::zvariant::OwnedValue;

    use super::*;

    const CHILD_ENV: &str = "DW_PORTAL_PRIVATE_BUS_CASE";

    #[link(name = "gobject-2.0")]
    unsafe extern "C" {
        fn g_object_unref(object: *mut c_void);
    }

    #[link(name = "glib-2.0")]
    unsafe extern "C" {
        fn g_error_free(error: *mut c_void);
    }

    unsafe extern "C" {
        fn dbus_bus_get_private(bus_type: c_int, error: *mut c_void) -> *mut c_void;
        fn dbus_connection_close(connection: *mut c_void);
        fn dbus_connection_unref(connection: *mut c_void);
        fn dbus_message_new_method_call(
            destination: *const c_char,
            path: *const c_char,
            iface: *const c_char,
            method: *const c_char,
        ) -> *mut c_void;
        fn dbus_message_unref(message: *mut c_void);
        fn dbus_connection_send_with_reply_and_block(
            connection: *mut c_void,
            message: *mut c_void,
            timeout_milliseconds: c_int,
            error: *mut c_void,
        ) -> *mut c_void;
        fn dbus_error_init(error: *mut c_void);
        fn dbus_error_free(error: *mut c_void);
    }

    struct PrivateBus {
        address: String,
        daemon: Child,
        _dir: tempfile::TempDir,
    }

    /// The portal service file a private bus offers for activation.
    enum Activation {
        None,
        /// Runs and never takes the name, like a portal that cannot start.
        NeverUp,
        /// This test binary in `serve` mode: takes the name and answers.
        Serves,
    }

    impl PrivateBus {
        fn start(activation: Activation) -> Self {
            let dir = dw_testutil::private_tempdir();
            let services = dir.path().join("services");
            std::fs::create_dir(&services).unwrap();
            let exec = match activation {
                Activation::None => None,
                Activation::NeverUp => Some("/bin/sleep 30".to_string()),
                Activation::Serves => Some(format!(
                    "/usr/bin/env {CHILD_ENV}=serve {} --exact portal::tests::private_bus_portal_cases",
                    std::env::current_exe().unwrap().display()
                )),
            };
            if let Some(exec) = exec {
                std::fs::write(
                    services.join("org.freedesktop.portal.Desktop.service"),
                    format!("[D-BUS Service]\nName={PORTAL}\nExec={exec}\n"),
                )
                .unwrap();
            }
            let socket = dir.path().join("bus");
            let address = format!("unix:path={}", socket.display());
            let conf = dir.path().join("bus.conf");
            std::fs::write(
                &conf,
                format!(
                    "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\"\n \
                     \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n\
                     <busconfig>\n  \
                       <type>session</type>\n  \
                       <listen>{address}</listen>\n  \
                       <servicedir>{}</servicedir>\n  \
                       <policy context=\"default\">\n    \
                         <allow send_destination=\"*\" eavesdrop=\"true\"/>\n    \
                         <allow eavesdrop=\"true\"/>\n    \
                         <allow own=\"*\"/>\n  \
                       </policy>\n\
                     </busconfig>\n",
                    services.display()
                ),
            )
            .unwrap();
            let daemon = Command::new("dbus-daemon")
                .arg(format!("--config-file={}", conf.display()))
                .arg("--nofork")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("dbus-daemon");
            let deadline = Instant::now() + Duration::from_secs(60);
            while !socket.exists() {
                assert!(Instant::now() < deadline, "dbus-daemon did not listen");
                std::thread::sleep(Duration::from_millis(20));
            }
            Self {
                address,
                daemon,
                _dir: dir,
            }
        }
    }

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    struct ControlledSettings;

    #[interface(name = "org.freedesktop.portal.Settings")]
    impl ControlledSettings {
        fn read(&self, _namespace: &str, _key: &str) -> OwnedValue {
            OwnedValue::from(1u32)
        }
    }

    fn read_color_scheme(conn: &zbus::blocking::Connection) -> zbus::Result<u32> {
        let reply = conn.call_method(
            Some(PORTAL),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Settings"),
            "Read",
            &("org.freedesktop.appearance", "color-scheme"),
        )?;
        let value: OwnedValue = reply.body().deserialize()?;
        Ok(u32::try_from(value)?)
    }

    fn run_child(address: &str, case_name: &str) {
        let out = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("portal::tests::private_bus_portal_cases")
            .arg("--nocapture")
            .env("DBUS_SESSION_BUS_ADDRESS", address)
            .env(CHILD_ENV, case_name)
            .output()
            .expect("portal child");
        assert!(
            out.status.success(),
            "case {case_name} failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn an_unowned_portal_skips_startup_autostart_only() {
        assert_eq!(
            proxy_flags(
                Some(PORTAL),
                Some("org.freedesktop.portal.Inhibit"),
                0,
                true
            ),
            DO_NOT_AUTO_START
        );
        assert_eq!(
            proxy_flags(
                Some(PORTAL),
                Some("org.freedesktop.portal.Settings"),
                0,
                true
            ),
            DO_NOT_AUTO_START
        );
        assert_eq!(
            proxy_flags(
                Some(PORTAL),
                Some("org.freedesktop.portal.FileChooser"),
                0,
                true
            ),
            0
        );
        assert_eq!(
            proxy_flags(
                Some(PORTAL),
                Some("org.freedesktop.portal.Inhibit"),
                0,
                false
            ),
            0
        );
        assert_eq!(
            proxy_flags(
                Some("org.gnome.SessionManager"),
                Some("org.freedesktop.portal.Inhibit"),
                0,
                true
            ),
            0
        );
        assert_eq!(proxy_flags(None, None, 0, true), 0);
    }

    #[test]
    fn private_bus_portal_cases() {
        match std::env::var(CHILD_ENV).ok().as_deref() {
            Some("unowned") => {
                skip_portal_autostart_if_unavailable();
                assert!(SKIP_AUTOSTART.load(Ordering::SeqCst));
                let resolved = unsafe {
                    dlsym(
                        std::ptr::null_mut(),
                        c"g_dbus_proxy_new_for_bus_sync".as_ptr(),
                    )
                };
                assert_eq!(
                    resolved, g_dbus_proxy_new_for_bus_sync as *const () as *mut c_void,
                    "executable must interpose g_dbus_proxy_new_for_bus_sync"
                );
                let new_for_bus: NewForBusSync = unsafe { std::mem::transmute(resolved) };

                let started = Instant::now();
                let mut gerr: *mut c_void = std::ptr::null_mut();
                let proxy = unsafe {
                    new_for_bus(
                        2,
                        0,
                        std::ptr::null_mut(),
                        c"org.freedesktop.portal.Desktop".as_ptr(),
                        c"/org/freedesktop/portal/desktop".as_ptr(),
                        c"org.freedesktop.portal.Inhibit".as_ptr(),
                        std::ptr::null_mut(),
                        &mut gerr,
                    )
                };
                let gio_elapsed = started.elapsed();
                if !proxy.is_null() {
                    unsafe { g_object_unref(proxy) };
                }
                if !gerr.is_null() {
                    unsafe { g_error_free(gerr) };
                }
                assert!(
                    gio_elapsed < Duration::from_secs(2),
                    "GIO Inhibit proxy waited on portal activation: {gio_elapsed:?}"
                );

                let started = Instant::now();
                let mut derr = [0usize; 4];
                unsafe { dbus_error_init(derr.as_mut_ptr().cast()) };
                let conn = unsafe { dbus_bus_get_private(0, derr.as_mut_ptr().cast()) };
                assert!(!conn.is_null(), "private libdbus session connection");
                let msg = unsafe {
                    dbus_message_new_method_call(
                        c"org.freedesktop.portal.Desktop".as_ptr(),
                        c"/org/freedesktop/portal/desktop".as_ptr(),
                        c"org.freedesktop.portal.Settings".as_ptr(),
                        c"Read".as_ptr(),
                    )
                };
                assert!(!msg.is_null());
                let reply = unsafe {
                    dbus_connection_send_with_reply_and_block(
                        conn,
                        msg,
                        10_000,
                        derr.as_mut_ptr().cast(),
                    )
                };
                let dbus_elapsed = started.elapsed();
                assert!(reply.is_null(), "unowned portal returned a reply");
                unsafe {
                    dbus_message_unref(msg);
                    dbus_error_free(derr.as_mut_ptr().cast());
                    dbus_connection_close(conn);
                    dbus_connection_unref(conn);
                }
                assert!(
                    dbus_elapsed < Duration::from_secs(2),
                    "libdbus Settings.Read waited on portal activation: {dbus_elapsed:?}"
                );
                let peer = zbus::blocking::Connection::session().unwrap();
                assert!(
                    !portal_is_owned_on(&peer),
                    "process claimed org.freedesktop.portal.Desktop on the session bus"
                );
                let reply = peer
                    .request_name_with_flags(PORTAL, zbus::fdo::RequestNameFlags::DoNotQueue.into())
                    .unwrap();
                assert_eq!(reply, zbus::fdo::RequestNameReply::PrimaryOwner);
            }
            Some(case @ ("owned" | "activatable")) => {
                skip_portal_autostart_unless_up_within(Duration::from_secs(120));
                assert!(
                    !SKIP_AUTOSTART.load(Ordering::SeqCst),
                    "{case} portal was skipped"
                );
                let conn = zbus::blocking::Connection::session().unwrap();
                assert_eq!(read_color_scheme(&conn).expect("the portal answered"), 1);
                let dbus = zbus::blocking::fdo::DBusProxy::new(&conn).unwrap();
                let owner = dbus
                    .get_name_owner(PORTAL.try_into().unwrap())
                    .expect("the portal is owned");
                let owner_pid = dbus
                    .get_connection_unix_process_id(owner.into_inner().into())
                    .unwrap();
                assert_ne!(
                    owner_pid,
                    std::process::id(),
                    "this process owns the portal"
                );
            }
            Some("serve") => {
                let address = std::env::var("DBUS_STARTER_ADDRESS").expect("started by the bus");
                let conn = zbus::blocking::connection::Builder::address(address.as_str())
                    .unwrap()
                    .serve_at("/org/freedesktop/portal/desktop", ControlledSettings)
                    .unwrap()
                    .name(PORTAL)
                    .unwrap()
                    .build()
                    .unwrap();
                while conn
                    .call_method(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        Some("org.freedesktop.DBus.Peer"),
                        "Ping",
                        &(),
                    )
                    .is_ok()
                {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
            Some(other) => panic!("unknown case {other}"),
            None => {
                let never_up = PrivateBus::start(Activation::NeverUp);
                run_child(&never_up.address, "unowned");
                drop(never_up);

                let serves = PrivateBus::start(Activation::Serves);
                run_child(&serves.address, "activatable");
                drop(serves);

                let owned = PrivateBus::start(Activation::None);
                let service = zbus::blocking::connection::Builder::address(owned.address.as_str())
                    .unwrap()
                    .name(PORTAL)
                    .unwrap()
                    .serve_at("/org/freedesktop/portal/desktop", ControlledSettings)
                    .unwrap()
                    .build()
                    .unwrap();
                assert!(portal_is_owned_on(&service));
                run_child(&owned.address, "owned");
            }
        }
    }
}
