mod sensors;
mod tui;
mod watch;

use clap::{Parser, Subcommand};
use hwall_core::render;
use hwall_core::{collect_snapshot, CollectOptions, CollectionProfile, MonitorCollector};
use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "hwall-cli",
    version,
    about = "Read-only Linux hardware inventory and telemetry"
)]
struct Cli {
    /// Include serial numbers, UUIDs, WWNs, and MAC addresses.
    #[arg(long, global = true)]
    sensitive: bool,

    /// Do not call optional read-only helper commands.
    #[arg(long, global = true)]
    no_helpers: bool,

    /// Include slower SMART/NVMe health collection; device access may require privileges.
    #[arg(long, global = true)]
    health: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print one human-readable report. This is the default command.
    Report {
        /// Show transport nodes, raw properties, source paths, and sensor limits.
        #[arg(short, long)]
        verbose: bool,
    },

    /// Export one machine-readable snapshot.
    Export {
        /// Pretty-print the JSON document.
        #[arg(long)]
        pretty: bool,
    },

    /// Print a compact sensor table with optional model-level filters.
    Sensors(sensors::Args),

    /// Open an interactive, continuously updating terminal monitor.
    Watch {
        /// Refresh interval, for example 500ms, 1s, or 2m. Minimum: 100ms.
        #[arg(short, long, default_value = "1s", value_parser = parse_duration)]
        interval: Duration,

        /// Re-run full discovery at this interval for hotplug and label changes.
        #[arg(long, default_value = "30s", value_parser = parse_duration)]
        rediscover: Duration,

        /// Refresh SMART/NVMe health at this interval when --health is enabled.
        #[arg(long, default_value = "30m", value_parser = parse_duration)]
        health_interval: Duration,

        /// Emit one compact JSON snapshot per line instead of opening the terminal UI.
        #[arg(long)]
        jsonl: bool,

        /// Start the terminal monitor in exhaustive diagnostic mode.
        #[arg(short, long)]
        verbose: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let base_options = CollectOptions {
        profile: CollectionProfile::Full,
        allow_helper_commands: !cli.no_helpers,
        include_sensitive: cli.sensitive,
        include_storage_health: cli.health,
    };

    match cli.command.unwrap_or(Command::Report { verbose: false }) {
        Command::Report { verbose } => {
            let snapshot = collect_snapshot(&base_options);
            print!("{}", render::human(&snapshot, verbose));
            ExitCode::SUCCESS
        }
        Command::Export { pretty } => {
            let snapshot = collect_snapshot(&base_options);
            let result = if pretty {
                serde_json::to_writer_pretty(io::stdout().lock(), &snapshot)
            } else {
                serde_json::to_writer(io::stdout().lock(), &snapshot)
            };
            match result {
                Ok(()) => {
                    println!();
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("failed to serialize snapshot: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Sensors(args) => sensors::run(args, base_options),
        Command::Watch {
            interval,
            rediscover,
            health_interval,
            jsonl,
            verbose,
        } => run_watch(
            base_options,
            interval,
            rediscover,
            health_interval,
            jsonl,
            verbose,
        ),
    }
}

fn run_watch(
    full_options: CollectOptions,
    interval: Duration,
    rediscover: Duration,
    health_interval: Duration,
    jsonl: bool,
    verbose: bool,
) -> ExitCode {
    let collector = MonitorCollector::new(full_options, rediscover, health_interval);

    if jsonl {
        return watch::run_jsonl(collector, interval);
    }

    if !io::stdout().is_terminal() {
        eprintln!("interactive watch requires a terminal; use --jsonl when piping output");
        return ExitCode::FAILURE;
    }

    match tui::run(collector, interval, verbose) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("watch failed: {error}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn parse_duration(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    let (raw, multiplier, unit_name) = if let Some(raw) = value.strip_suffix("ms") {
        (raw, 0.001, "millisecond")
    } else if let Some(raw) = value.strip_suffix('s') {
        (raw, 1.0, "second")
    } else if let Some(raw) = value.strip_suffix('m') {
        (raw, 60.0, "minute")
    } else {
        (value, 1.0, "second")
    };
    let number = raw
        .parse::<f64>()
        .map_err(|_| format!("invalid {unit_name} duration"))?;
    let seconds = number * multiplier;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("duration must be a positive finite value".to_owned());
    }
    let duration = Duration::from_secs_f64(seconds);
    if duration < Duration::from_millis(100) {
        return Err("minimum interval is 100ms".to_owned());
    }
    Ok(duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_intervals() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("2s").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
        assert!(parse_duration("50ms").is_err());
    }
}
