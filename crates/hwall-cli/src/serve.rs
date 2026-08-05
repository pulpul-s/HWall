use crate::{parse_duration, sensors, ViewArg};
use clap::Args;
use hwall_core::{CollectOptions, MonitorCollector, Snapshot};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

const REDISCOVER_INTERVAL: Duration = Duration::from_secs(30);
const HEALTH_INTERVAL: Duration = Duration::from_secs(30 * 60);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST_LINE_BYTES: usize = 4096;

#[derive(Debug, Args)]
pub(crate) struct ServeArgs {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:8765")]
    listen: SocketAddr,

    /// Snapshot refresh interval, for example 500ms, 1s, or 2m. Minimum: 200ms.
    #[arg(short, long, default_value = "1s", value_parser = parse_duration)]
    interval: Duration,

    /// JSON view to serve.
    #[arg(long, value_enum, default_value = "sensors")]
    view: ViewArg,
}

pub(crate) fn run(options: CollectOptions, args: ServeArgs) -> io::Result<()> {
    let ServeArgs {
        listen,
        interval,
        view,
    } = args;
    let listener = TcpListener::bind(listen)?;
    let collector = MonitorCollector::new(options, REDISCOVER_INTERVAL, HEALTH_INTERVAL);
    let snapshot = serialize_snapshot(&collector.initial_snapshot(), view)?;
    let cache = Arc::new(RwLock::new(snapshot));
    let refresh_cache = Arc::clone(&cache);

    thread::Builder::new()
        .name("hwall-api-collector".to_owned())
        .spawn(move || refresh_snapshots(collector, refresh_cache, interval, view))?;

    eprintln!("serving HWall JSON on http://{listen}/");
    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, &cache) {
                    eprintln!("HTTP request failed: {error}");
                }
            }
            Err(error) => eprintln!("failed to accept HTTP connection: {error}"),
        }
    }
    Ok(())
}

fn refresh_snapshots(
    mut collector: MonitorCollector,
    cache: Arc<RwLock<Vec<u8>>>,
    interval: Duration,
    view: ViewArg,
) {
    let mut next_refresh = Instant::now() + interval;
    loop {
        thread::sleep(next_refresh.saturating_duration_since(Instant::now()));
        let started = Instant::now();
        let snapshot = collector.snapshot(false);
        match serialize_snapshot(&snapshot, view) {
            Ok(snapshot) => match cache.write() {
                Ok(mut cached) => *cached = snapshot,
                Err(_) => {
                    eprintln!("snapshot cache is unavailable");
                    return;
                }
            },
            Err(error) => eprintln!("failed to serialize snapshot: {error}"),
        }

        let scheduled = started + interval;
        let finished = Instant::now();
        next_refresh = if scheduled <= finished {
            finished
        } else {
            scheduled
        };
    }
}

fn serialize_snapshot(snapshot: &Snapshot, view: ViewArg) -> io::Result<Vec<u8>> {
    match view {
        ViewArg::Mixed => serde_json::to_vec(snapshot),
        ViewArg::Sensors => serde_json::to_vec(&sensors::json_value(snapshot)),
        ViewArg::Hardware => {
            let mut hardware = snapshot.clone();
            for device in &mut hardware.devices {
                device.sensors.clear();
            }
            serde_json::to_vec(&hardware)
        }
    }
    .map_err(io::Error::other)
}

fn handle_connection(mut stream: TcpStream, cache: &RwLock<Vec<u8>>) -> io::Result<()> {
    stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CLIENT_TIMEOUT))?;

    let request_line = read_request_line(&mut stream)?;
    match request_line.as_deref().map(classify_request) {
        Some(Request::Snapshot) => {
            let body = cache
                .read()
                .map_err(|_| io::Error::other("snapshot cache is unavailable"))?
                .clone();
            write_response(&mut stream, "200 OK", "application/json", &body, None)
        }
        Some(Request::NotFound) => write_response(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"Not found\n",
            None,
        ),
        Some(Request::MethodNotAllowed) => write_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            b"Method not allowed\n",
            Some("Allow: GET\r\n"),
        ),
        Some(Request::Invalid) | None => write_response(
            &mut stream,
            "400 Bad Request",
            "text/plain; charset=utf-8",
            b"Bad request\n",
            None,
        ),
    }
}

fn read_request_line(stream: &mut TcpStream) -> io::Result<Option<String>> {
    let mut buffer = [0_u8; MAX_REQUEST_LINE_BYTES];
    let mut length = 0;

    while length < buffer.len() {
        let read = stream.read(&mut buffer[length..])?;
        if read == 0 {
            break;
        }
        length += read;
        if buffer[..length].contains(&b'\n') {
            break;
        }
    }

    let Some(end) = buffer[..length].iter().position(|byte| *byte == b'\n') else {
        return Ok(None);
    };
    let Ok(line) = std::str::from_utf8(&buffer[..end]) else {
        return Ok(None);
    };
    Ok(Some(line.trim_end_matches('\r').to_owned()))
}

#[derive(Debug, PartialEq, Eq)]
enum Request {
    Snapshot,
    NotFound,
    MethodNotAllowed,
    Invalid,
}

fn classify_request(line: &str) -> Request {
    let mut parts = line.split_ascii_whitespace();
    let Some(method) = parts.next() else {
        return Request::Invalid;
    };
    let Some(target) = parts.next() else {
        return Request::Invalid;
    };
    let Some(version) = parts.next() else {
        return Request::Invalid;
    };
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Request::Invalid;
    }
    if method != "GET" {
        return Request::MethodNotAllowed;
    }
    if target != "/" {
        return Request::NotFound;
    }
    Request::Snapshot
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    extra_headers: Option<&str>,
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{}\r\n",
        body.len(),
        extra_headers.unwrap_or_default(),
    )?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use hwall_core::{Device, DeviceClass, Identification, Sensor, SensorKind, Unit};
    use serde_json::Value;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        serve: ServeArgs,
    }

    fn snapshot() -> Snapshot {
        let mut snapshot = Snapshot::new();
        snapshot.captured_at_unix_ms = 123;
        let mut device = Device::new("cpu:0", DeviceClass::Cpu, "Test CPU");
        device.sensors.push(Sensor::new(
            "cpu:0:temperature:package",
            "Package",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(42.0),
            "/test",
            Identification::KernelLabel,
        ));
        snapshot.devices.push(device);
        snapshot
    }

    #[test]
    fn sensors_are_the_default_view() {
        let args = TestCli::try_parse_from(["test"]).unwrap().serve;
        assert!(matches!(args.view, ViewArg::Sensors));
    }

    #[test]
    fn views_serialize_only_the_requested_data() {
        let snapshot = snapshot();

        let sensors: Value =
            serde_json::from_slice(&serialize_snapshot(&snapshot, ViewArg::Sensors).unwrap())
                .unwrap();
        assert!(sensors.get("devices").is_none());
        assert_eq!(sensors["sensors"][0]["value"].as_f64(), Some(42.0));

        let hardware: Value =
            serde_json::from_slice(&serialize_snapshot(&snapshot, ViewArg::Hardware).unwrap())
                .unwrap();
        assert!(hardware.get("sensors").is_none());
        assert!(hardware["devices"][0].get("sensors").is_none());

        let mixed: Value =
            serde_json::from_slice(&serialize_snapshot(&snapshot, ViewArg::Mixed).unwrap())
                .unwrap();
        assert!(mixed.get("sensors").is_none());
        assert_eq!(
            mixed["devices"][0]["sensors"][0]["value"].as_f64(),
            Some(42.0)
        );
    }

    #[test]
    fn only_getting_the_root_returns_the_snapshot() {
        assert_eq!(classify_request("GET / HTTP/1.1"), Request::Snapshot);
        assert_eq!(classify_request("GET /other HTTP/1.1"), Request::NotFound);
        assert_eq!(
            classify_request("POST / HTTP/1.1"),
            Request::MethodNotAllowed
        );
    }

    #[test]
    fn malformed_request_lines_are_rejected() {
        assert_eq!(classify_request(""), Request::Invalid);
        assert_eq!(classify_request("GET /"), Request::Invalid);
        assert_eq!(classify_request("GET / HTTP/2"), Request::Invalid);
        assert_eq!(classify_request("GET / HTTP/1.1 extra"), Request::Invalid);
    }
}
