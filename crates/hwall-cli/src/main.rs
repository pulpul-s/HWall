mod sensors;
mod serve;
mod tui;
mod watch;

use clap::{Args, Parser, Subcommand, ValueEnum};
use hwall_app::{MIN_REFRESH_INTERVAL_MS, TerminalView, render_terminal_view};
use hwall_core::render;
use hwall_core::{
    CollectOptions, CollectionProfile, MonitorCollector, SnapshotStatistics, collect_snapshot,
};
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
    /// Print one human-readable mixed report.
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
    Watch(WatchArgs),

    /// Serve the latest JSON view over HTTP.
    Serve(serve::ServeArgs),
}

#[derive(Debug, Clone, Args)]
struct WatchArgs {
    /// Refresh interval, for example 500ms, 1s, or 2m. Minimum: 200ms.
    #[arg(short, long, default_value = "1s", value_parser = parse_duration)]
    interval: Duration,

    /// Re-run full discovery at this interval for hotplug and label changes.
    #[arg(long, default_value = "30s", value_parser = parse_duration)]
    rediscover: Duration,

    /// Refresh SMART/NVMe health at this interval when --health is enabled.
    #[arg(long, default_value = "30m", value_parser = parse_duration)]
    health_interval: Duration,

    /// Initial terminal view.
    #[arg(long, value_enum, default_value = "mixed")]
    view: ViewArg,

    /// Emit one compact JSON snapshot per line instead of opening the terminal UI.
    #[arg(long)]
    jsonl: bool,

    /// Start the terminal monitor in exhaustive diagnostic mode.
    #[arg(short, long)]
    verbose: bool,
}

impl Default for WatchArgs {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            rediscover: Duration::from_secs(30),
            health_interval: Duration::from_secs(30 * 60),
            view: ViewArg::Mixed,
            jsonl: false,
            verbose: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum ViewArg {
    #[default]
    Mixed,
    Sensors,
    Hardware,
}

impl From<ViewArg> for TerminalView {
    fn from(value: ViewArg) -> Self {
        match value {
            ViewArg::Mixed => Self::Mixed,
            ViewArg::Sensors => Self::Sensors,
            ViewArg::Hardware => Self::Hardware,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let base_options = CollectOptions {
        profile: CollectionProfile::Full,
        allow_helper_commands: !cli.no_helpers,
        include_sensitive: cli.sensitive,
        include_storage_health: cli.health,
    };

    match cli.command {
        None if io::stdout().is_terminal() => run_watch(base_options, WatchArgs::default()),
        None => print_report(base_options, false),
        Some(Command::Report { verbose }) => print_report(base_options, verbose),
        Some(Command::Export { pretty }) => export_snapshot(base_options, pretty),
        Some(Command::Sensors(args)) => sensors::run(args, base_options),
        Some(Command::Watch(args)) => run_watch(base_options, args),
        Some(Command::Serve(args)) => match serve::run(base_options, args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("HTTP server failed: {error}");
                ExitCode::FAILURE
            }
        },
    }
}

fn print_report(options: CollectOptions, verbose: bool) -> ExitCode {
    let snapshot = collect_snapshot(&options);
    if verbose {
        print!("{}", render::diagnostic(&snapshot, None));
    } else {
        print!(
            "{}",
            render_terminal_view(
                &snapshot,
                &SnapshotStatistics::default(),
                TerminalView::Mixed,
            )
        );
    }
    ExitCode::SUCCESS
}

fn export_snapshot(options: CollectOptions, pretty: bool) -> ExitCode {
    let snapshot = collect_snapshot(&options);
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

fn run_watch(full_options: CollectOptions, args: WatchArgs) -> ExitCode {
    let collector = MonitorCollector::new(full_options, args.rediscover, args.health_interval);

    if args.jsonl {
        return watch::run_jsonl(collector, args.interval);
    }

    if !io::stdout().is_terminal() {
        eprintln!("interactive watch requires a terminal; use --jsonl when piping output");
        return ExitCode::FAILURE;
    }

    match tui::run(collector, args.interval, args.verbose, args.view.into()) {
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
    if duration < Duration::from_millis(MIN_REFRESH_INTERVAL_MS) {
        return Err(format!("minimum interval is {MIN_REFRESH_INTERVAL_MS}ms"));
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
        assert!(parse_duration("100ms").is_err());
        assert_eq!(parse_duration("200ms").unwrap(), Duration::from_millis(200));
    }

    #[test]
    fn watch_defaults_to_the_mixed_view() {
        assert!(matches!(WatchArgs::default().view, ViewArg::Mixed));
    }
}
