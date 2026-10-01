//! `x events`: a live stream of system changes.
//!
//! Poll-based and honest by construction: every round samples the same
//! managers the other commands read, diffs consecutive rounds and prints
//! only the differences. There is no kernel subscription anywhere, so what
//! the stream shows is exactly what re-running `x ps` / `x port` / … would
//! show. A family whose read fails is reported as a warning line, and its
//! baseline is silently re-established when it recovers, so a flaky read
//! never manufactures fake open/close events.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use x_core::error::{Error, Result};
use x_core::events::{diff, EventType, SystemEvent, SystemSnapshot};
use x_core::SystemContext;

use crate::format::{OutputFormat, Renderer};

/// Arguments for `x events`.
#[derive(Debug, clap::Args)]
pub struct EventsArgs {
    /// Poll interval in seconds.
    #[arg(long, default_value_t = 2.0)]
    pub interval: f64,
    /// Stop after this many polls instead of running until Ctrl-C.
    #[arg(long)]
    pub count: Option<usize>,
    /// Watched families: process, connection, usb, mount, service, or all.
    ///
    /// Default: everything except usb — usb sampling shells out on Windows
    /// (Get-PnpDevice, ~1.6 s per call) and is too heavy to poll.
    #[arg(long, value_name = "LIST")]
    pub types: Option<String>,
}

/// Route `x events`.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    args: &EventsArgs,
) -> Result<i32> {
    let types = parse_types(args.types.as_deref())?;
    watch(context, renderer, &types, args.interval, args.count)
}

/// Poll, diff, print — until `--count` polls are done or Ctrl-C arrives.
fn watch(
    context: &SystemContext,
    renderer: &mut Renderer,
    types: &[EventType],
    interval: f64,
    count: Option<usize>,
) -> Result<i32> {
    let offset = context
        .system
        .info()
        .ok()
        .and_then(|info| info.utc_offset_seconds)
        .unwrap_or(0);
    let interval = if interval.is_finite() {
        interval.max(0.05)
    } else {
        2.0
    };
    let json = renderer.format() == OutputFormat::Json;

    let mut previous: Option<SystemSnapshot> = None;
    let mut polls = 0usize;
    loop {
        let current = SystemSnapshot::sample(context, types);
        let time = x_core::format_timestamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            offset,
        );
        if json {
            let events = previous
                .as_ref()
                .map(|before| diff(before, &current))
                .unwrap_or_default();
            if !events.is_empty() || !current.failures().is_empty() {
                let failures: Vec<Failure> = current
                    .failures()
                    .iter()
                    .map(|(family, reason)| Failure {
                        family: family.name(),
                        reason,
                    })
                    .collect();
                renderer.always_json(&PollReport {
                    time,
                    events: &events,
                    failures,
                })?;
            }
        } else {
            match &previous {
                None => renderer.line(format!("[{time}] {}", baseline(&current, interval)))?,
                Some(before) => {
                    for event in diff(before, &current) {
                        renderer.line(format!(
                            "[{time}] {} {}",
                            event.op.symbol(),
                            event.summary
                        ))?;
                    }
                }
            }
            for (family, reason) in current.failures() {
                renderer.line(format!(
                    "[{time}] ! {} sampling failed: {reason}",
                    family.name()
                ))?;
            }
        }
        renderer.flush().map_err(|e| {
            Error::new(
                x_core::ErrorKind::System,
                format!("events output failed: {e}"),
            )
        })?;
        previous = Some(current);
        polls += 1;
        if count.is_some_and(|target| polls >= target) {
            return Ok(0);
        }
        std::thread::sleep(Duration::from_secs_f64(interval));
    }
}

/// One poll round as one JSON document.
#[derive(Debug, serde::Serialize)]
struct PollReport<'a> {
    time: String,
    events: &'a [SystemEvent],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    failures: Vec<Failure<'a>>,
}

/// A family this round could not read.
#[derive(Debug, serde::Serialize)]
struct Failure<'a> {
    #[serde(rename = "type")]
    family: &'a str,
    reason: &'a str,
}

/// The line a fresh stream starts with: what it watches and how often.
fn baseline(snapshot: &SystemSnapshot, interval: f64) -> String {
    let counts: Vec<String> = snapshot
        .counts()
        .iter()
        .map(|(family, count)| format!("{} {count}", family.name()))
        .collect();
    if counts.is_empty() {
        return format!("watching no categories (every sample failed); polling every {interval}s");
    }
    format!("watching {}; polling every {interval}s", counts.join(", "))
}

/// `--types` tokens → families. The default is [`EventType::DEFAULT`].
fn parse_types(spec: Option<&str>) -> Result<Vec<EventType>> {
    let Some(spec) = spec else {
        return Ok(EventType::DEFAULT.to_vec());
    };
    let mut types: Vec<EventType> = Vec::new();
    for token in spec.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if token.eq_ignore_ascii_case("all") {
            for family in EventType::ALL {
                if !types.contains(&family) {
                    types.push(family);
                }
            }
            continue;
        }
        match EventType::parse(token) {
            Some(family) if !types.contains(&family) => types.push(family),
            Some(_) => {}
            None => {
                return Err(Error::invalid_input(format!(
                    "unknown event type {token:?}; valid: process, connection, usb, mount, service, all"
                )))
            }
        }
    }
    if types.is_empty() {
        return Err(Error::invalid_input("no event types selected"));
    }
    Ok(types)
}
