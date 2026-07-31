use hwall_core::{MonitorCollector, MonitorPoll, MonitorRequestResult, MonitorWorker};
use std::io::{self, Write};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(5);

pub(crate) fn run_jsonl(collector: MonitorCollector, interval: Duration) -> ExitCode {
    let worker = match MonitorWorker::spawn(collector) {
        Ok(worker) => worker,
        Err(error) => {
            eprintln!("failed to start collector worker: {error}");
            return ExitCode::FAILURE;
        }
    };
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut next_start = Instant::now();

    loop {
        thread::sleep(next_start.saturating_duration_since(Instant::now()));
        let started = loop {
            let started = Instant::now();
            match worker.request(false) {
                MonitorRequestResult::Accepted => break started,
                MonitorRequestResult::Busy => thread::sleep(WORKER_POLL_INTERVAL),
                MonitorRequestResult::Disconnected => {
                    eprintln!("collector worker stopped unexpectedly");
                    return ExitCode::FAILURE;
                }
            }
        };

        let snapshot = loop {
            match worker.poll() {
                MonitorPoll::Update(update) if update.storage_health_device_ids.is_empty() => {
                    break update.snapshot;
                }
                MonitorPoll::Update(_) | MonitorPoll::Idle => {
                    thread::sleep(WORKER_POLL_INTERVAL);
                }
                MonitorPoll::Disconnected => {
                    eprintln!("collector worker stopped unexpectedly");
                    return ExitCode::FAILURE;
                }
            }
        };

        if let Err(error) = serde_json::to_writer(&mut out, &snapshot)
            .map_err(io::Error::other)
            .and_then(|_| writeln!(out))
            .and_then(|_| out.flush())
        {
            eprintln!("output failed: {error}");
            return ExitCode::FAILURE;
        }

        next_start = next_sample_deadline(started, Instant::now(), interval);
    }
}

fn next_sample_deadline(started: Instant, finished: Instant, interval: Duration) -> Instant {
    let scheduled = started + interval;
    if scheduled <= finished {
        finished
    } else {
        scheduled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_time_is_subtracted_from_the_interval() {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(1);
        let finished = start + Duration::from_millis(341);

        assert_eq!(
            deadline.saturating_duration_since(finished),
            Duration::from_millis(659),
        );
    }

    #[test]
    fn missed_deadlines_do_not_add_an_extra_sleep() {
        let start = Instant::now();
        let finished = start + Duration::from_millis(341);

        assert_eq!(
            next_sample_deadline(start, finished, Duration::from_millis(200)),
            finished,
        );
    }

    #[test]
    fn cadence_restarts_after_an_overrun_without_catching_up() {
        let start = Instant::now();
        let overrun_finished = start + Duration::from_millis(341);
        let restarted = next_sample_deadline(start, overrun_finished, Duration::from_millis(200));
        let next_finished = restarted + Duration::from_millis(150);
        let next_deadline =
            next_sample_deadline(restarted, next_finished, Duration::from_millis(200));

        assert_eq!(next_deadline, restarted + Duration::from_millis(200));
        assert_eq!(
            next_deadline.saturating_duration_since(next_finished),
            Duration::from_millis(50),
        );
    }
}
