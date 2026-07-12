//! Connect to the running daemon's Unix socket, read the hello handshake,
//! subscribe to device events, and optionally trigger a sync. Mimics what the
//! SwiftUI client will do. Unix-only.
//!
//! Usage:
//!   cargo run --example daemon-probe            # watch device events
//!   cargo run --example daemon-probe -- sync    # also send trigger_sync

#[cfg(not(unix))]
fn main() {
    eprintln!("daemon-probe is a Unix-only example.");
}

#[cfg(unix)]
fn main() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let path = classick::daemon::ipc_server::default_pipe_name();
    eprintln!("connecting to {path}");
    let stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("connect failed: {e} (is the daemon running?)");
            std::process::exit(1);
        }
    };
    let mut writer = stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(stream);

    // The daemon sends `hello` first.
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        eprintln!("no hello from daemon");
        std::process::exit(2);
    }
    println!("<= {}", line.trim());

    writeln!(writer, "{}", r#"{"type":"subscribe_device_events"}"#).unwrap();
    writer.flush().unwrap();
    eprintln!("=> subscribe_device_events (plug/unplug the iPod to see events)");

    if std::env::args().nth(1).as_deref() == Some("sync") {
        writeln!(writer, "{}", r#"{"type":"trigger_sync","source":"manual"}"#).unwrap();
        writer.flush().unwrap();
        eprintln!("=> trigger_sync");
    }

    for l in reader.lines() {
        match l {
            Ok(l) => println!("<= {l}"),
            Err(_) => break,
        }
    }
}
