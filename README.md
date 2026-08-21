# claprs

An APRS tracker that lives in your terminal. It reads the live [APRS-IS](https://www.aprs-is.net/) stream (receive only) and shows you what is on the air near you, or one callsign you care about, with a `top`-style live table if you want one.

## Install

You need a Rust toolchain.

```sh
git clone https://github.com/benwoody/claprs
cd claprs
cargo install --path .
```

## Setup

```sh
claprs config set callsign W0ODL     # your call, for the APRS-IS login
claprs config set home 32.53,-93.70  # your lat,lon
```

Neither is strictly required. The login falls back to `N0CALL`, and you can always pass `--from lat,lon`. But setting them makes `here` and the distance column work without extra typing.

## Using it

```sh
claprs here              # stations within 50 mi of home
claprs here -t           # same thing as a live, sortable table
claprs near 100 -t       # wider radius
claprs call W0ODL        # follow one callsign (all SSIDs)
claprs watch W0ODL-7 N5OQT -t
claprs feed              # raw live feed around home
claprs last W0ODL-10     # last known position via aprs.fi (needs a key)
```

Add `-t` to any of the live commands for the full-screen table. Once you are in it:

```
up/dn  move        s  sort (recent / distance)
/      search      t  cycle type (mobile / wx / fixed / ...)
enter  detail      o  open the station on aprs.fi
p      pause       q  quit
```

The table decodes positions, including Mic-E (with speed, course, and altitude), puts a symbol emoji on each station, colors rows by distance, reads weather stations in plain English, and flashes new arrivals green. The detail popup adds bearing from home and a short position trail so you can watch a mobile move.

Drop the `-t` and you get the same decoded data as a scrolling log instead. Add `--raw` to anything to see the untouched APRS-IS lines.

## Config

Values are looked up in this order: command-line flag, then environment variable, then the config file.

| key | config file | env var |
| --- | --- | --- |
| callsign | `callsign` | `CLAPRS_CALLSIGN` |
| home | `home` | `CLAPRS_HOME` |
| server | `server` | `CLAPRS_SERVER` |
| aprs.fi key | `aprsfi-key` | `APRSFI_API_KEY` |

`claprs config path` prints where the file lives, `claprs config show` prints the current values. A free aprs.fi API key (https://aprs.fi/page/api) is only needed for `last`.

## A note on APRS-IS

claprs connects receive only (passcode `-1`, so it physically cannot transmit), holds a single connection, and always sends a server-side filter. That is the normal, expected way to read from APRS-IS. `last` is the only command that touches the aprs.fi web API, which is rate limited, so use it for spot checks rather than in a loop.

## License

MIT. See [LICENSE](LICENSE).
