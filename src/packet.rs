//! A small, panic-free APRS packet parser and pretty printer.
//!
//! Decodes the common cases (uncompressed, compressed, and Mic-E position
//! reports, plus objects, messages, status) into a clean line, and gracefully
//! labels everything else. Anything it cannot parse falls back to raw.

/// A decoded APRS packet (only the fields we display).
pub struct Packet {
    pub source: String,
    /// Short type label: pos, obj, item, msg, status, mic-e, wx, tlm, ...
    pub kind: &'static str,
    /// Object/item name or message addressee, if any.
    pub name: Option<String>,
    /// Decoded position as (lat, lon) in decimal degrees, if present.
    pub position: Option<(f64, f64)>,
    /// APRS symbol as (table id, symbol code) bytes, if present.
    pub symbol: Option<(u8, u8)>,
    /// Comment / message text / summary (lossy, trimmed).
    pub info: String,
}

impl Packet {
    /// A two-cell-wide emoji for this station's APRS symbol, or two spaces if
    /// unknown. Every returned string is width 2 so table columns stay aligned.
    pub fn icon(&self) -> &'static str {
        match self.symbol {
            Some((table, code)) => symbol_icon(table, code),
            None => "  ",
        }
    }
}

/// Parse one APRS-IS line. Returns `None` if it does not look like a packet.
pub fn parse(line: &str) -> Option<Packet> {
    let (header, payload) = line.split_once(':')?;
    let (source, after) = header.split_once('>')?;
    let dest = after.split(',').next().unwrap_or("");
    if source.is_empty() || payload.is_empty() {
        return None;
    }
    let b = payload.as_bytes();
    let pkt = match b[0] {
        b'!' | b'=' => position(source, &b[1..]),
        b'@' | b'/' => {
            if b.len() >= 8 {
                position(source, &b[8..]) // skip 7-char timestamp
            } else {
                bare(source, "pos")
            }
        }
        b';' => object(source, &b[1..]),
        b')' => bare_info(source, "item", &b[1..]),
        b':' => message(source, &b[1..]),
        b'>' => text(source, "status", &b[1..]),
        b'<' => text(source, "caps", &b[1..]),
        b'T' => bare_info(source, "tlm", &b[1..]),
        b'`' | b'\'' => mic_e(source, dest, &b[1..]),
        b'_' => weather_report(source, &b[1..]),
        b'$' => bare(source, "gps"),
        b'?' => text(source, "query", &b[1..]),
        _ => bare_info(source, "?", b),
    };
    Some(pkt)
}

/// Render a decoded packet as one aligned line (with distance from `home` if known).
pub fn format_line(p: &Packet, home: Option<(f64, f64)>) -> String {
    let ts = chrono::Local::now().format("%H:%M:%S");
    let (pos_col, dist_col) = match p.position {
        Some((la, lo)) => {
            let d = home
                .map(|h| format!("{:>4.0}mi", distance_mi(h, (la, lo))))
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
        "{ts}  {}  {:<9}  {:<6}  {:<19}  {:>6}  {}",
        p.icon(), p.source, p.kind, pos_col, dist_col, info
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
    let mut p = new(source, "pos");
    if let Some((pos, sym, comment)) = parse_uncompressed(rest).or_else(|| parse_compressed(rest)) {
        p.position = Some(pos);
        p.symbol = Some(sym);
        if sym.1 == b'_' {
            // weather station symbol: the comment is a weather report
            p.kind = "wx";
            p.info = format_weather(&comment);
        } else {
            p.info = comment;
        }
    } else {
        p.info = lossy(rest);
    }
    p
}

fn object(source: &str, rest: &[u8]) -> Packet {
    // ;NAME(9) + flag(1) + timestamp(7) + position
    if rest.len() >= 18 {
        let mut p = new(source, "obj");
        p.name = Some(lossy(&rest[0..9]));
        let posdata = &rest[17..];
        match parse_uncompressed(posdata).or_else(|| parse_compressed(posdata)) {
            Some((pos, sym, comment)) => {
                p.position = Some(pos);
                p.symbol = Some(sym);
                p.info = comment;
            }
            None => p.info = lossy(posdata),
        }
        p
    } else {
        bare_info(source, "obj", rest)
    }
}

fn message(source: &str, rest: &[u8]) -> Packet {
    // ADDRESSEE(9) + ':' + text
    if rest.len() >= 10 && rest[9] == b':' {
        let mut p = new(source, "msg");
        p.name = Some(lossy(&rest[0..9]));
        p.info = lossy(&rest[10..]);
        p
    } else {
        bare_info(source, "msg", rest)
    }
}

fn mic_e(source: &str, dest: &str, info: &[u8]) -> Packet {
    let d = dest.as_bytes();
    if d.len() >= 6 {
        if let Some((lat, north, offset, west)) = decode_mic_e_dest(&d[0..6]) {
            if let Some((lon, speed, course, alt, comment)) = decode_mic_e_info(info, offset, west) {
                let lat = if north { lat } else { -lat };
                let mut parts: Vec<String> = Vec::new();
                if speed > 0 {
                    parts.push(format!("{speed}kt {course:03}°"));
                }
                if let Some(a) = alt {
                    if (-1000..=60000).contains(&a) {
                        parts.push(format!("{a}m"));
                    }
                }
                if !comment.is_empty() {
                    parts.push(comment);
                }
                let mut p = new(source, "mic-e");
                p.position = Some((lat, lon));
                if info.len() >= 8 {
                    p.symbol = Some((info[7], info[6])); // table id, symbol code
                }
                p.info = parts.join("  ");
                return p;
            }
        }
    }
    bare_info(source, "mic-e", info)
}

fn text(source: &str, kind: &'static str, rest: &[u8]) -> Packet {
    let mut p = new(source, kind);
    p.info = lossy(rest);
    p
}

fn bare(source: &str, kind: &'static str) -> Packet {
    new(source, kind)
}

fn bare_info(source: &str, kind: &'static str, rest: &[u8]) -> Packet {
    let mut p = new(source, kind);
    p.info = lossy(rest);
    p
}

fn new(source: &str, kind: &'static str) -> Packet {
    Packet {
        source: source.into(),
        kind,
        name: None,
        position: None,
        symbol: None,
        info: String::new(),
    }
}

/// Map an APRS symbol (table id, code) to a two-cell emoji. Uses only default
/// emoji-presentation characters (no variation selectors) so every result is
/// reliably width 2. Keyed mostly on the symbol code; good enough for the
/// common cases across both tables.
fn symbol_icon(_table: u8, code: u8) -> &'static str {
    match code {
        b'>' => "🚗",
        b'j' | b'R' => "🚙",
        b'v' => "🚐",
        b'k' | b'u' => "🚛",
        b'U' => "🚌",
        b'<' => "🛵",
        b'b' => "🚲",
        b'[' => "🚶",
        b'_' | b'W' => "⛅",
        b'#' => "📡",
        b'-' | b'y' => "🏠",
        b'\'' | b'^' => "🛫",
        b'X' => "🚁",
        b'O' => "🎈",
        b's' | b'Y' | b'C' => "⛵",
        b'a' => "🚑",
        b'f' => "🚒",
        b':' => "🔥",
        b'P' | b'!' => "👮",
        b'&' | b'I' => "🌐",
        b'r' => "📻",
        b'h' => "🏥",
        _ => "📍",
    }
}

// --- weather ----------------------------------------------------------------

/// Positionless weather report (`_` DTI): `_MMDDHHMM` then weather fields.
fn weather_report(source: &str, rest: &[u8]) -> Packet {
    let mut p = new(source, "wx");
    let data = if rest.len() > 8 { &rest[8..] } else { rest };
    p.info = format_weather(&lossy(data));
    p
}

/// Format an APRS weather string, falling back to the raw text if nothing parses.
fn format_weather(raw: &str) -> String {
    let s = parse_weather(raw);
    if s.is_empty() { raw.to_string() } else { s }
}

/// Parse the common APRS weather fields into a short readable summary.
fn parse_weather(s: &str) -> String {
    let b = s.as_bytes();
    let num = |at: usize, n: usize| -> Option<i32> {
        b.get(at..at + n)
            .and_then(|x| std::str::from_utf8(x).ok())
            .and_then(|x| x.trim().parse().ok())
    };

    let mut i = 0usize;
    let (mut wdir, mut wspd) = (None, None);
    // Leading wind as course/speed: ddd/sss
    if b.len() >= 7
        && b[3] == b'/'
        && b[0..3].iter().all(|c| c.is_ascii_digit())
        && b[4..7].iter().all(|c| c.is_ascii_digit())
    {
        wdir = num(0, 3);
        wspd = num(4, 3);
        i = 7;
    }

    let (mut temp, mut gust, mut hum, mut baro, mut cdir, mut cspd) =
        (None, None, None, None, None, None);
    while i < b.len() {
        match b[i] {
            b'c' => { cdir = num(i + 1, 3); i += 4; }
            b's' => { cspd = num(i + 1, 3); i += 4; }
            b'g' => { gust = num(i + 1, 3); i += 4; }
            b't' => { temp = num(i + 1, 3); i += 4; }
            b'h' => { hum = num(i + 1, 2); i += 3; }
            b'b' => { baro = num(i + 1, 5); i += 6; }
            b'r' | b'p' | b'P' | b'L' | b'l' | b'S' => i += 4, // rain / luminosity / snow
            _ => i += 1,
        }
    }

    let mut parts = Vec::new();
    if let Some(t) = temp {
        parts.push(format!("{t}°F"));
    }
    if let (Some(d), Some(sp)) = (wdir.or(cdir), wspd.or(cspd)) {
        let g = gust.filter(|&g| g > 0).map(|g| format!(" g{g}")).unwrap_or_default();
        parts.push(format!("wind {sp}mph@{d:03}{g}"));
    }
    if let Some(h) = hum {
        parts.push(format!("hum {}%", if h == 0 { 100 } else { h }));
    }
    if let Some(bp) = baro {
        parts.push(format!("{:.1}hPa", bp as f64 / 10.0));
    }
    parts.join("  ")
}

// --- position formats -------------------------------------------------------

fn parse_uncompressed(b: &[u8]) -> Option<((f64, f64), (u8, u8), String)> {
    if b.len() < 19 {
        return None;
    }
    let lat = parse_lat(&b[0..8])?;
    let lon = parse_lon(&b[9..18])?;
    let sym = (b[8], b[18]); // symbol table id, symbol code
    Some(((lat, lon), sym, lossy(&b[19..])))
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

fn parse_compressed(b: &[u8]) -> Option<((f64, f64), (u8, u8), String)> {
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
    let sym = (b[0], b[9]); // symbol table id, symbol code
    Some(((lat, lon), sym, lossy(&b[13..])))
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

// --- Mic-E ------------------------------------------------------------------

/// Decode the 6-char Mic-E destination: (abs latitude, north?, lon offset?, west?).
fn decode_mic_e_dest(d: &[u8]) -> Option<(f64, bool, bool, bool)> {
    let mut digits = [0u8; 6];
    for (i, &c) in d.iter().enumerate().take(6) {
        digits[i] = match c {
            b'0'..=b'9' => c - b'0',
            b'A'..=b'J' => c - b'A',
            b'P'..=b'Y' => c - b'P',
            b'K' | b'L' | b'Z' => 0, // ambiguity / space
            _ => return None,
        };
    }
    let is_high = |c: u8| (b'P'..=b'Z').contains(&c);
    let north = is_high(d[3]);
    let offset = is_high(d[4]);
    let west = is_high(d[5]);

    let deg = digits[0] as f64 * 10.0 + digits[1] as f64;
    let min = digits[2] as f64 * 10.0 + digits[3] as f64 + (digits[4] as f64 * 10.0 + digits[5] as f64) / 100.0;
    let lat = deg + min / 60.0;
    if lat > 90.0 {
        return None;
    }
    Some((lat, north, offset, west))
}

/// Decode the Mic-E info field: (signed longitude, speed kt, course deg, altitude m, comment).
fn decode_mic_e_info(info: &[u8], offset: bool, west: bool) -> Option<(f64, i32, i32, Option<i32>, String)> {
    if info.len() < 8 {
        return None;
    }
    let mut lon_deg = info[0] as i32 - 28;
    if offset {
        lon_deg += 100;
    }
    if (180..=189).contains(&lon_deg) {
        lon_deg -= 80;
    } else if (190..=199).contains(&lon_deg) {
        lon_deg -= 190;
    }
    if !(0..=179).contains(&lon_deg) {
        return None;
    }
    let mut lon_min = info[1] as i32 - 28;
    if lon_min >= 60 {
        lon_min -= 60;
    }
    if !(0..=59).contains(&lon_min) {
        return None;
    }
    let lon_hun = info[2] as i32 - 28;
    if !(0..=99).contains(&lon_hun) {
        return None;
    }
    let mut lon = lon_deg as f64 + (lon_min as f64 + lon_hun as f64 / 100.0) / 60.0;
    if west {
        lon = -lon;
    }

    let mut speed = (info[3] as i32 - 28) * 10 + (info[4] as i32 - 28) / 10;
    if speed >= 800 {
        speed -= 800;
    }
    let mut course = ((info[4] as i32 - 28) % 10) * 100 + (info[5] as i32 - 28);
    if course >= 400 {
        course -= 400;
    }

    let (alt, comment) = if info.len() > 8 {
        parse_mic_e_comment(&info[8..])
    } else {
        (None, String::new())
    };
    Some((lon, speed, course, alt, comment))
}

/// Pull an optional altitude out of the Mic-E comment tail and return the clean
/// text. Altitude is 3 base-91 chars followed by `}` (relative to -10000 m),
/// usually right after a leading type byte; we drop that whole group.
fn parse_mic_e_comment(b: &[u8]) -> (Option<i32>, String) {
    for i in 3..b.len().min(7) {
        if b[i] == b'}' {
            let (c0, c1, c2) = (b[i - 3], b[i - 2], b[i - 1]);
            if [c0, c1, c2].iter().all(|&c| (33..=126).contains(&c)) {
                let alt = (c0 as i32 - 33) * 8281 + (c1 as i32 - 33) * 91 + (c2 as i32 - 33) - 10000;
                return (Some(alt), strip_mic_e_extras(&lossy(&b[i + 1..])));
            }
        }
    }
    (None, strip_mic_e_extras(&lossy(b)))
}

/// Remove Mic-E trailing extras that some radios append after the human comment:
/// base-91 telemetry (`|..|`) and the DAO datum/precision extension (`!w..!`).
fn strip_mic_e_extras(s: &str) -> String {
    let mut out = s.to_string();

    // Leading Mic-E radio/type byte (Kenwood '>' or ']', Yaesu '`' or '\'').
    if matches!(out.chars().next(), Some('>' | ']' | '`' | '\'')) {
        out.remove(0);
    }

    // Telemetry and other Mic-E extras always sit at the end: cut from the
    // first '|' (base-91 telemetry marker, paired or not).
    if let Some(a) = out.find('|') {
        out.truncate(a);
    }

    // DAO extension: ! + (w|W) + 2 chars + !
    let dao = {
        let b = out.as_bytes();
        (0..b.len().saturating_sub(4)).find(|&i| {
            b[i] == b'!' && (b[i + 1] == b'w' || b[i + 1] == b'W') && b[i + 4] == b'!'
        })
    };
    if let Some(i) = dao {
        out.replace_range(i..i + 5, "");
    }

    out = out.trim().to_string();

    // A clearly space-separated trailing "_X" symbol token (e.g. "... path  _4").
    if let Some(idx) = out.rfind(" _") {
        if out[idx + 2..].chars().count() <= 1 {
            out.truncate(idx);
        }
    }

    let mut out = out.trim().to_string();
    // A lone "_X" token as the whole comment is just noise.
    if out.starts_with('_') && out.chars().count() <= 2 {
        out.clear();
    }
    out
}

// --- helpers ----------------------------------------------------------------

/// Great-circle distance in statute miles between two (lat, lon) points.
pub fn distance_mi(a: (f64, f64), b: (f64, f64)) -> f64 {
    const R_MI: f64 = 3958.7613;
    let (lat1, lon1) = a;
    let (lat2, lon2) = b;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let h = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R_MI * h.sqrt().asin()
}

/// Decode bytes to display text. Each byte maps to one Latin-1 character, and
/// every control character (C0 and C1) becomes a space, so raw APRS payload
/// bytes (Mic-E symbol/telemetry bytes, stray ESC, high bytes, etc.) can never
/// move the cursor or corrupt the terminal.
fn lossy(b: &[u8]) -> String {
    let s: String = b
        .iter()
        .map(|&byte| byte as char)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    s.trim().to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}
