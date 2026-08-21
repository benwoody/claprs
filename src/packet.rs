//! A small, panic-free APRS packet parser and pretty printer.
//!
//! This is intentionally partial: it decodes the common cases (uncompressed and
//! compressed position reports, objects, messages, status) into a clean line,
//! and gracefully labels everything else (Mic-E, telemetry, weather, ...) rather
//! than trying to fully decode it. Anything it cannot parse falls back to raw.

/// A decoded APRS packet (only the fields we display).
pub struct Packet {
    pub source: String,
    /// Short type label: pos, obj, item, msg, status, mic-e, wx, tlm, ...
    pub kind: &'static str,
    /// Object/item name or message addressee, if any.
    pub name: Option<String>,
    /// Decoded position as (lat, lon) in decimal degrees, if present.
    pub position: Option<(f64, f64)>,
    /// Comment / message text / summary (lossy, trimmed).
    pub info: String,
}

/// Parse one APRS-IS line. Returns `None` if it does not look like a packet.
pub fn parse(line: &str) -> Option<Packet> {
    let (header, payload) = line.split_once(':')?;
    let source = header.split_once('>')?.0.to_string();
    if source.is_empty() || payload.is_empty() {
        return None;
    }
    let b = payload.as_bytes();
    let pkt = match b[0] {
        b'!' | b'=' => position(&source, &b[1..]),
        b'@' | b'/' => {
            if b.len() >= 8 {
                position(&source, &b[8..]) // skip 7-char timestamp
            } else {
                bare(&source, "pos")
            }
        }
        b';' => object(&source, &b[1..]),
        b')' => bare_info(&source, "item", &b[1..]),
        b':' => message(&source, &b[1..]),
        b'>' => text(&source, "status", &b[1..]),
        b'<' => text(&source, "caps", &b[1..]),
        b'T' => bare_info(&source, "tlm", &b[1..]),
        b'`' | b'\'' => bare_info(&source, "mic-e", &b[1..]),
        b'_' => text(&source, "wx", &b[1..]),
        b'$' => bare(&source, "gps"),
        b'?' => text(&source, "query", &b[1..]),
        _ => bare_info(&source, "?", b),
    };
    Some(pkt)
}

/// Render a decoded packet as one aligned line (with distance from `home` if known).
pub fn format_line(p: &Packet, home: Option<(f64, f64)>) -> String {
    let ts = chrono::Local::now().format("%H:%M:%S");
    let (pos_col, dist_col) = match p.position {
        Some((la, lo)) => {
            let d = home
                .map(|h| format!("{:>4.0}mi", haversine_mi(h, (la, lo))))
                .unwrap_or_default();
            (format!("{la:>8.4},{lo:>9.4}"), d)
        }
        None => (String::new(), String::new()),
    };
    let mut info = p.info.clone();
    if let Some(n) = &p.name {
        let n = n.trim();
        info = if info.is_empty() {
            n.to_string()
        } else {
            format!("{n}  {info}")
        };
    }
    let info = truncate(&info, 52);
    format!(
        "{ts}  {:<9}  {:<6}  {:<19}  {:>6}  {}",
        p.source, p.kind, pos_col, dist_col, info
    )
}

/// Column header matching `format_line`.
pub fn header() -> String {
    format!(
        "{:<8}  {:<9}  {:<6}  {:<19}  {:>6}  {}",
        "TIME", "STATION", "TYPE", "POSITION", "DIST", "INFO"
    )
}

fn position(source: &str, rest: &[u8]) -> Packet {
    let mut p = Packet {
        source: source.into(),
        kind: "pos",
        name: None,
        position: None,
        info: String::new(),
    };
    if let Some((pos, comment)) = parse_uncompressed(rest).or_else(|| parse_compressed(rest)) {
        p.position = Some(pos);
        p.info = comment;
    } else {
        p.info = lossy(rest);
    }
    p
}

fn object(source: &str, rest: &[u8]) -> Packet {
    // ;NAME(9) + flag(1) + timestamp(7) + position
    if rest.len() >= 18 {
        let name = lossy(&rest[0..9]);
        let posdata = &rest[17..];
        let (position, info) = match parse_uncompressed(posdata).or_else(|| parse_compressed(posdata))
        {
            Some((pos, comment)) => (Some(pos), comment),
            None => (None, lossy(posdata)),
        };
        Packet {
            source: source.into(),
            kind: "obj",
            name: Some(name),
            position,
            info,
        }
    } else {
        bare_info(source, "obj", rest)
    }
}

fn message(source: &str, rest: &[u8]) -> Packet {
    // ADDRESSEE(9) + ':' + text
    if rest.len() >= 10 && rest[9] == b':' {
        Packet {
            source: source.into(),
            kind: "msg",
            name: Some(lossy(&rest[0..9])),
            position: None,
            info: lossy(&rest[10..]),
        }
    } else {
        bare_info(source, "msg", rest)
    }
}

fn text(source: &str, kind: &'static str, rest: &[u8]) -> Packet {
    Packet {
        source: source.into(),
        kind,
        name: None,
        position: None,
        info: lossy(rest),
    }
}

fn bare(source: &str, kind: &'static str) -> Packet {
    Packet {
        source: source.into(),
        kind,
        name: None,
        position: None,
        info: String::new(),
    }
}

fn bare_info(source: &str, kind: &'static str, rest: &[u8]) -> Packet {
    Packet {
        source: source.into(),
        kind,
        name: None,
        position: None,
        info: lossy(rest),
    }
}

fn parse_uncompressed(b: &[u8]) -> Option<((f64, f64), String)> {
    if b.len() < 19 {
        return None;
    }
    let lat = parse_lat(&b[0..8])?;
    let lon = parse_lon(&b[9..18])?;
    Some(((lat, lon), lossy(&b[19..])))
}

fn parse_lat(b: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(b).ok()?;
    let deg: f64 = s.get(0..2)?.parse().ok()?;
    let min: f64 = s.get(2..7)?.replace(' ', "0").parse().ok()?;
    let hemi = *s.as_bytes().get(7)?;
    if hemi != b'N' && hemi != b'S' {
        return None;
    }
    let v = deg + min / 60.0;
    Some(if hemi == b'S' { -v } else { v })
}

fn parse_lon(b: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(b).ok()?;
    let deg: f64 = s.get(0..3)?.parse().ok()?;
    let min: f64 = s.get(3..8)?.replace(' ', "0").parse().ok()?;
    let hemi = *s.as_bytes().get(8)?;
    if hemi != b'E' && hemi != b'W' {
        return None;
    }
    let v = deg + min / 60.0;
    Some(if hemi == b'W' { -v } else { v })
}

fn parse_compressed(b: &[u8]) -> Option<((f64, f64), String)> {
    if b.len() < 13 {
        return None;
    }
    let y = base91(&b[1..5])?;
    let x = base91(&b[5..9])?;
    let lat = 90.0 - (y as f64) / 380926.0;
    let lon = -180.0 + (x as f64) / 190463.0;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    Some(((lat, lon), lossy(&b[13..])))
}

fn base91(b: &[u8]) -> Option<u32> {
    if b.len() != 4 {
        return None;
    }
    let mut v: u32 = 0;
    for &c in b {
        if !(33..=123).contains(&c) {
            return None;
        }
        v = v * 91 + (c as u32 - 33);
    }
    Some(v)
}

fn haversine_mi(a: (f64, f64), b: (f64, f64)) -> f64 {
    const R_MI: f64 = 3958.7613;
    let (lat1, lon1) = a;
    let (lat2, lon2) = b;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let h = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R_MI * h.sqrt().asin()
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}
