//! On-demand last-known-position lookups via the aprs.fi API.
//!
//! Unlike the live APRS-IS stream, this returns the *last stored* position for
//! a station immediately. It requires a free API key (https://aprs.fi/page/api)
//! and should be used sparingly — it is a rate-limited web API, not a firehose.

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// Fetch and print the last-known location(s) for `callsign`.
pub fn last(key: &str, callsign: &str) -> Result<()> {
    let url = format!(
        "https://api.aprs.fi/api/get?name={callsign}&what=loc&apikey={key}&format=json"
    );

    let body: Value = ureq::get(&url)
        .set(
            "User-Agent",
            concat!("claprs/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .context("calling the aprs.fi API")?
        .into_json()
        .context("parsing the aprs.fi response")?;

    let result = body.get("result").and_then(Value::as_str).unwrap_or("");
    if result != "ok" {
        let desc = body
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        bail!("aprs.fi error: {desc}");
    }

    let entries = body.get("entries").and_then(Value::as_array);
    match entries {
        Some(list) if !list.is_empty() => {
            for e in list {
                print_entry(e);
            }
        }
        _ => println!("no stored position found for {callsign}"),
    }
    Ok(())
}

fn print_entry(e: &Value) {
    let s = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or("");
    let name = if s("name").is_empty() { "?" } else { s("name") };
    let when = e
        .get("lasttime")
        .and_then(Value::as_str)
        .and_then(|t| t.parse::<i64>().ok())
        .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %Z")
                .to_string()
        })
        .unwrap_or_else(|| "?".to_string());

    println!("{name}");
    println!("  position : {}, {}", s("lat"), s("lng"));
    println!("  heard    : {when}");
    let comment = s("comment");
    if !comment.is_empty() {
        println!("  comment  : {comment}");
    }
}
