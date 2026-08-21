# claprs

A command line APRS tracker for following stations and areas from your terminal, powered by a read-only [APRS-IS](https://www.aprs-is.net/) connection (with an optional [aprs.fi](https://aprs.fi/) snapshot lookup).

## Install

Requires a Rust toolchain

```sh
git clone https://github.com/benwoody/claprs
cd claprs
cargo install --path .
# or just: cargo run -- <args>
```

## Quick start

```sh
claprs config set callsign W0ODL     # used for the read-only APRS-IS login
claprs config set home 32.53,-93.70  # your lat,lon, for `near` and `here`

claprs call W0ODL         # follow one station live (all SSIDs)
claprs here               # everything within 50 mi of home
claprs near 25 --unit km  # everything within 25 km of home
claprs watch W0ODL-7 N5OQT
claprs feed               # raw live feed around home
claprs last W0ODL-10      # instant last known position (needs aprs.fi key)
```

## Commands

| Command | What it does |
| --- | --- |
| `call <CALL>` | Follow one station live (SSIDs matched) |
| `watch <CALL...>` | Combined live feed for a watchlist (TUI table coming soon) |
| `near <RADIUS>` | Stations within a radius of home, or `--from lat,lon` |
| `here` | Stations near your saved home (`--radius`, `--unit`) |
| `feed` | Raw live feed, or a custom `--filter` |
| `last <CALL>` | Instant last known position via aprs.fi (needs a key) |
| `config` | `path`, `show`, `get <key>`, `set <key> <value>` |

Run `claprs help` or `claprs <command> --help` for details.

## Configuration

Values resolve in this order: **command line flag > environment variable > config file**.

| Key | Config file | Env var |
| --- | --- | --- |
| callsign | `callsign` | `CLAPRS_CALLSIGN` |
| home | `home` | `CLAPRS_HOME` |
| server | `server` | `CLAPRS_SERVER` |
| aprs.fi key | `aprsfi-key` | `APRSFI_API_KEY` |

The config file lives at the path shown by `claprs config path`. 

Get a free aprs.fi API key at <https://aprs.fi/page/api> (only needed for `last`).

## Being a good neighbor

claprs connects to APRS-IS **receive only** (passcode `-1`, so it can never transmit), opens a **single** connection, and always uses a server side **filter**. That is the sanctioned way to consume APRS-IS. The `last` command uses the aprs.fi web API, which is rate limited, so use it sparingly.

## License

Released under the [MIT License](LICENSE).
