//! The wallet's reading of `coreP2PAddrs` entries, for the glibc
//! differential in `testdata/oracle/glibc` (MG-04b D78). Reads one
//! hex-encoded entry per line on stdin and writes, per line, the service
//! `parse_core_service` reads and `ServiceAddr::from_socket_addr` keeps
//! (`network:address hex:port`), or `-` for none.

use std::io::{BufRead, Write};

use dw_protx::shared::netinfo::{ServiceAddr, parse_core_service};

fn main() {
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for line in std::io::stdin().lock().lines() {
        let line = line.expect("stdin");
        let bytes = hex::decode(line.trim()).expect("one hex entry per line");
        let entry = String::from_utf8(bytes).expect("an ASCII entry");
        match parse_core_service(&entry).and_then(ServiceAddr::from_socket_addr) {
            Some(s) => writeln!(out, "{}:{}:{}", s.network, hex::encode(&s.address), s.port),
            None => writeln!(out, "-"),
        }
        .expect("stdout");
    }
}
