//! GTK's application startup builds a D-Bus proxy for
//! `org.freedesktop.portal.Desktop` (`Inhibit`) with auto-start, and Tao's
//! window creation reads `org.freedesktop.portal.Settings` with auto-start.
//! When nothing owns that name, the bus tries to activate
//! `xdg-desktop-portal` and those calls sit until activation or client
//! timeouts fail (~25 s in GTK + 5 s in Tao), before the window can paint.
//!
//! This does not claim any bus name. If a short check finds the portal
//! already owned, D-Bus calls are left unchanged. Otherwise startup calls to
//! `Inhibit` and `Settings` in this process set no-auto-start, so they fail
//! immediately without asking the bus to launch the portal. Other interfaces
//! (such as `FileChooser`) and other processes are untouched.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const STARTUP_INTERFACES: &[&str] = &[
    "org.freedesktop.portal.Inhibit",
    "org.freedesktop.portal.Settings",
];
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

pub(crate) fn skip_portal_autostart_if_unowned() {
    // Referenced so the linker keeps the interposers in the executable.
    let _ = g_dbus_proxy_new_sync as *const ();
    let _ = g_dbus_proxy_new_for_bus_sync as *const ();
    let _ = __wrap_dbus_connection_send_with_reply_and_block as *const ();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("dw-portal-check".into())
        .spawn(move || {
            let _ = tx.send(!portal_is_owned());
        });
    let skip = if spawned.is_err() {
        true
    } else {
        rx.recv_timeout(Duration::from_millis(400)).unwrap_or(true)
    };
    SKIP_AUTOSTART.store(skip, Ordering::SeqCst);
    if skip {
        eprintln!("dw-app: desktop portal is not owned; not asking the bus to start it");
    }
}

fn portal_is_owned() -> bool {
    let Ok(conn) = zbus::blocking::Connection::session() else {
        return false;
    };
    portal_is_owned_on(&conn)
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

fn next<T>(symbol: &CStr) -> Option<T> {
    let ptr = unsafe { dlsym(-1isize as *mut c_void, symbol.as_ptr()) };
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute_copy(&ptr) })
    }
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
    let real = *REAL.get_or_init(|| next(c"g_dbus_proxy_new_sync").expect("g_dbus_proxy_new_sync"));
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
    let real = *REAL.get_or_init(|| {
        next(c"g_dbus_proxy_new_for_bus_sync").expect("g_dbus_proxy_new_for_bus_sync")
    });
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
    use std::io::{BufRead, BufReader};
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

    impl PrivateBus {
        fn start(with_slow_portal_service: bool) -> Self {
            let dir = dw_testutil::private_tempdir();
            let services = dir.path().join("services");
            std::fs::create_dir(&services).unwrap();
            if with_slow_portal_service {
                std::fs::write(
                    services.join("org.freedesktop.portal.Desktop.service"),
                    "[D-BUS Service]\nName=org.freedesktop.portal.Desktop\nExec=/bin/sleep 30\n",
                )
                .unwrap();
            }
            let conf = dir.path().join("bus.conf");
            std::fs::write(
                &conf,
                format!(
                    "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\"\n \
                     \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n\
                     <busconfig>\n  \
                       <type>session</type>\n  \
                       <listen>unix:tmpdir=/tmp</listen>\n  \
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
            let mut daemon = Command::new("dbus-daemon")
                .arg(format!("--config-file={}", conf.display()))
                .arg("--nofork")
                .arg("--print-address=1")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("dbus-daemon");
            let stdout = daemon.stdout.take().expect("dbus-daemon stdout");
            let mut address = String::new();
            BufReader::new(stdout).read_line(&mut address).unwrap();
            let address = address.trim().to_string();
            assert!(!address.is_empty(), "dbus-daemon printed no address");
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
                skip_portal_autostart_if_unowned();
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
                assert!(
                    !portal_is_owned(),
                    "process claimed org.freedesktop.portal.Desktop on the session bus"
                );
                let peer = zbus::blocking::Connection::session().unwrap();
                let reply = peer
                    .request_name_with_flags(PORTAL, zbus::fdo::RequestNameFlags::DoNotQueue.into())
                    .unwrap();
                assert_eq!(reply, zbus::fdo::RequestNameReply::PrimaryOwner);
            }
            Some("owned") => {
                skip_portal_autostart_if_unowned();
                assert!(!SKIP_AUTOSTART.load(Ordering::SeqCst));
                let conn = zbus::blocking::Connection::session().unwrap();
                let reply = conn
                    .call_method(
                        Some(PORTAL),
                        "/org/freedesktop/portal/desktop",
                        Some("org.freedesktop.portal.Settings"),
                        "Read",
                        &("org.freedesktop.appearance", "color-scheme"),
                    )
                    .expect("controlled portal answered");
                let val: OwnedValue = reply.body().deserialize().unwrap();
                assert_eq!(u32::try_from(val).unwrap(), 1);
            }
            Some(other) => panic!("unknown case {other}"),
            None => {
                let unowned = PrivateBus::start(true);
                run_child(&unowned.address, "unowned");
                drop(unowned);

                let owned = PrivateBus::start(false);
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
