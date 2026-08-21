//! claprs: a command line APRS tracker.
//!
//! Follows stations and areas from your terminal using a read-only APRS-IS
//! connection, with an optional aprs.fi snapshot lookup.

mod aprsfi;
mod aprsis;
mod config;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use config::Config;

/// Command line APRS tracker for following stations and areas via APRS-IS.
#[derive(Parser)]
#[command(name = "claprs", version, about, long_about = None)]
struct Cli {
    /// Override the APRS-IS server (host:port)
    #[arg(long, global = true)]
    server: Option<String>,

    /// Override the callsign used for the (read-only) APRS-IS login
    #[arg(long, global = true)]
    callsign: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Follow one station live (SSIDs matched, e.g. `call W0ODL` catches -7, -10)
    Call {
        /// Callsign to follow
        callsign: String,
    },
    /// Live feed for a watchlist of callsigns (rich TUI table coming soon)
    Watch {
        /// Callsigns to watch
        callsigns: Vec<String>,
    },
    /// Stations within a radius of your home (or --from lat,lon)
    Near {
        /// Radius (in --unit, default miles)
        radius: f64,
        /// Center point as "lat,lon" (defaults to saved home)
        #[arg(long)]
        from: Option<String>,
        /// Distance unit: mi or km
        #[arg(long, default_value = "mi")]
        unit: String,
    },
    /// Stations near your saved home location
    Here {
        /// Radius (in --unit, default miles)
        #[arg(long, default_value_t = 50.0)]
        radius: f64,
        /// Distance unit: mi or km
        #[arg(long, default_value = "mi")]
        unit: String,
    },
    /// Raw live feed (optionally a custom APRS-IS filter)
    Feed {
        /// Custom APRS-IS server side filter (defaults to ~100mi around home)
        #[arg(long)]
        filter: Option<String>,
    },
    /// Instant last known position via the aprs.fi API (needs a free key)
    Last {
        /// Callsign to look up
        callsign: String,
    },
    /// Manage claprs configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the path to the config file
    Path,
    /// Show current configuration
    Show,
    /// Get one value (callsign | home | server | aprsfi-key)
    Get {
        key: String,
    },
    /// Set one value (callsign | home | server | aprsfi-key)
    Set {
        key: String,
        value: String,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("claprs: error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let g_server = cli.server.clone();
    let g_call = cli.callsign.clone();

    match cli.command {
        Commands::Call { callsign } => {
            let call = callsign.to_uppercase();
            let filter = format!("b/{call}*");
            run_stream(g_server, g_call, filter, format!("following {call}"))
        }
        Commands::Watch { callsigns } => {
            if callsigns.is_empty() {
                bail!("give at least one callsign, e.g. `claprs watch W0ODL-7 N5OQT`");
            }
            let buddies = callsigns
                .iter()
                .map(|c| format!("{}*", c.to_uppercase()))
                .collect::<Vec<_>>()
                .join("/");
            let filter = format!("b/{buddies}");
            eprintln!("(note: a rich TUI table is coming; for now this is a combined live feed)\n");
            run_stream(g_server, g_call, filter, format!("watching {}", callsigns.join(", ")))
        }
        Commands::Near { radius, from, unit } => {
            let center = match from.or_else(|| Config::load().ok().and_then(|c| c.resolve_home(None))) {
                Some(c) => c,
                None => bail!("no location: pass --from lat,lon or run `claprs config set home lat,lon`"),
            };
            let (lat, lon) = parse_latlon(&center)?;
            let km = to_km(radius, &unit)?;
            let filter = format!("r/{lat:.4}/{lon:.4}/{km:.0}");
            run_stream(g_server, g_call, filter, format!("within {radius} {unit} of {lat:.4},{lon:.4}"))
        }
        Commands::Here { radius, unit } => {
            let center = match Config::load()?.resolve_home(None) {
                Some(c) => c,
                None => bail!("no home set: run `claprs config set home lat,lon` first"),
            };
            let (lat, lon) = parse_latlon(&center)?;
            let km = to_km(radius, &unit)?;
            let filter = format!("r/{lat:.4}/{lon:.4}/{km:.0}");
            run_stream(g_server, g_call, filter, format!("within {radius} {unit} of home ({lat:.4},{lon:.4})"))
        }
        Commands::Feed { filter } => {
            let filter = match filter {
                Some(f) => f,
                None => {
                    let center = match Config::load()?.resolve_home(None) {
                        Some(c) => c,
                        None => bail!("feed needs a filter or a saved home: pass --filter '<aprs-is filter>' or set home"),
                    };
                    let (lat, lon) = parse_latlon(&center)?;
                    format!("r/{lat:.4}/{lon:.4}/160")
                }
            };
            run_stream(g_server, g_call, filter.clone(), format!("raw feed · {filter}"))
        }
        Commands::Last { callsign } => {
            let key = match Config::load()?.resolve_aprsfi_key(None) {
                Some(k) => k,
                None => bail!(
                    "no aprs.fi API key: get a free one at https://aprs.fi/page/api then run \
                     `claprs config set aprsfi-key <KEY>` (or export APRSFI_API_KEY=...)"
                ),
            };
            aprsfi::last(&key, &callsign.to_uppercase())
        }
        Commands::Config { action } => run_config(action),
    }
}

fn run_stream(cli_server: Option<String>, cli_call: Option<String>, filter: String, label: String) -> Result<()> {
    let cfg = Config::load()?;
    let server = cfg.resolve_server(cli_server);
    let login = cfg.resolve_callsign(cli_call);
    eprintln!("claprs · {label}");
    eprintln!("connecting to {server} as {login} (read-only) · filter: {filter} · Ctrl-C to stop\n");
    aprsis::stream(&server, &login, &filter, |line| {
        println!("{}  {}", chrono::Local::now().format("%H:%M:%S"), line);
    })
}

fn run_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Path => println!("{}", Config::path()?.display()),
        ConfigAction::Show => {
            let cfg = Config::load()?;
            println!("config file : {}", Config::path()?.display());
            println!("callsign    : {}", cfg.callsign.as_deref().unwrap_or("(unset)"));
            println!("home        : {}", cfg.home.as_deref().unwrap_or("(unset)"));
            println!("server      : {}", cfg.server.as_deref().unwrap_or("(default) rotate.aprs2.net:14580"));
            println!("aprsfi-key  : {}", if cfg.aprsfi_key.is_some() { "<set>" } else { "(unset)" });
        }
        ConfigAction::Get { key } => {
            let cfg = Config::load()?;
            let v = match key.replace('-', "_").as_str() {
                "callsign" => cfg.callsign,
                "home" => cfg.home,
                "server" => cfg.server,
                "aprsfi_key" => cfg.aprsfi_key.map(|_| "<set>".to_string()),
                other => bail!("unknown key '{other}' (callsign|home|server|aprsfi-key)"),
            };
            println!("{}", v.unwrap_or_else(|| "(unset)".to_string()));
        }
        ConfigAction::Set { key, value } => {
            let mut cfg = Config::load()?;
            match key.replace('-', "_").as_str() {
                "callsign" => cfg.callsign = Some(value.to_uppercase()),
                "home" => {
                    parse_latlon(&value)?;
                    cfg.home = Some(value);
                }
                "server" => cfg.server = Some(value),
                "aprsfi_key" => cfg.aprsfi_key = Some(value),
                other => bail!("unknown key '{other}' (callsign|home|server|aprsfi-key)"),
            }
            cfg.save()?;
            println!("saved to {}", Config::path()?.display());
        }
    }
    Ok(())
}

fn parse_latlon(s: &str) -> Result<(f64, f64)> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    if parts.len() != 2 {
        bail!("expected 'lat,lon' (e.g. 32.53,-93.70), got '{s}'");
    }
    let lat = parts[0].parse::<f64>().map_err(|_| anyhow::anyhow!("bad latitude '{}'", parts[0]))?;
    let lon = parts[1].parse::<f64>().map_err(|_| anyhow::anyhow!("bad longitude '{}'", parts[1]))?;
    Ok((lat, lon))
}

fn to_km(r: f64, unit: &str) -> Result<f64> {
    match unit.to_lowercase().as_str() {
        "mi" | "mile" | "miles" => Ok(r * 1.609_34),
        "km" | "k" => Ok(r),
        other => bail!("unknown unit '{other}': use mi or km"),
    }
}
