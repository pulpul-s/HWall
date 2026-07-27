use hwall_core::MonitorCollector;
use std::io::{self, Write};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

pub(crate) fn run_jsonl(mut collector: MonitorCollector, interval: Duration) -> ExitCode {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    loop {
        let snapshot = collector.snapshot(false);
        if let Err(error) = serde_json::to_writer(&mut out, &snapshot)
            .map_err(io::Error::other)
            .and_then(|_| writeln!(out))
            .and_then(|_| out.flush())
        {
            eprintln!("output failed: {error}");
            return ExitCode::FAILURE;
        }
        thread::sleep(interval);
    }
}
