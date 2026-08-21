//! Minimal read-only APRS-IS client.
//!
//! Opens a single TCP connection, logs in receive-only (passcode `-1`) with a
//! server-side filter, and calls `on_line` for every packet line received.
//! Server comment lines (starting with `#`) are swallowed, except the login
//! response which is echoed to stderr so you can see the connection was accepted.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

/// Connect to `server` (host:port), log in as `callsign` (read-only) with
/// `filter`, and stream packets to `on_line` until the connection closes.
pub fn stream(
    server: &str,
    callsign: &str,
    filter: &str,
    mut on_line: impl FnMut(&str),
) -> Result<()> {
    let stream =
        TcpStream::connect(server).with_context(|| format!("connecting to APRS-IS at {server}"))?;
    let mut writer = stream.try_clone().context("cloning socket")?;

    // Receive-only login: passcode -1 means "cannot transmit to APRS-IS".
    let login = format!(
        "user {callsign} pass -1 vers claprs {} filter {}\r\n",
        env!("CARGO_PKG_VERSION"),
        filter
    );
    writer
        .write_all(login.as_bytes())
        .context("sending APRS-IS login")?;
    writer.flush().ok();

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line.context("reading from APRS-IS")?;
        if let Some(rest) = line.strip_prefix('#') {
            if rest.contains("logresp") {
                eprintln!("[aprs-is]{rest}");
            }
            continue;
        }
        on_line(&line);
    }
    Ok(())
}
