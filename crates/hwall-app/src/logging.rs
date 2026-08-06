use crate::{SensorRow, project_dirs};
use hwall_core::render::escape_delimited;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    #[default]
    Csv,
    JsonLines,
}

impl LogFormat {
    pub const ALL: [Self; 2] = [Self::Csv, Self::JsonLines];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::JsonLines => "jsonl",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::JsonLines => "JSON Lines",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.id() == value)
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogScope {
    All,
    #[default]
    Visible,
    Favorites,
}

impl LogScope {
    pub const ALL: [Self; 3] = [Self::All, Self::Visible, Self::Favorites];

    pub const fn id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Visible => "visible",
            Self::Favorites => "favorites",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::All => "All sensors",
            Self::Visible => "Visible sensors",
            Self::Favorites => "Favorite sensors",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scope| scope.id() == value)
    }
}

pub fn default_log_directory() -> PathBuf {
    project_dirs()
        .map(|dirs| dirs.data_local_dir().join("logs"))
        .unwrap_or_else(|| PathBuf::from("logs"))
}

pub fn timestamped_log_path(directory: impl AsRef<Path>, format: LogFormat) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let extension = format.id();
    directory
        .as_ref()
        .join(format!("hwall-{timestamp}.{extension}"))
}

pub struct LogFileWriter {
    writer: BufWriter<File>,
    format: LogFormat,
}

impl LogFileWriter {
    pub fn create(path: impl AsRef<Path>, format: LogFormat) -> io::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        if format == LogFormat::Csv {
            writeln!(
                writer,
                "timestamp_ms,row_id,sensor,original_sensor,current,minimum,maximum,average,status"
            )?;
        }
        Ok(Self { writer, format })
    }

    pub fn write_sample(&mut self, timestamp_ms: u128, rows: &[SensorRow]) -> io::Result<()> {
        match self.format {
            LogFormat::Csv => write_csv_sample(&mut self.writer, timestamp_ms, rows)?,
            LogFormat::JsonLines => {
                let current_rows = rows
                    .iter()
                    .filter(|row| row.kind != crate::RowKind::Sensor || row.current_sample)
                    .collect::<Vec<_>>();
                let record = serde_json::json!({
                    "timestamp_ms": timestamp_ms,
                    "rows": current_rows,
                });
                serde_json::to_writer(&mut self.writer, &record)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                writeln!(self.writer)?;
            }
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[derive(Debug)]
enum Command {
    Sample {
        timestamp_ms: u128,
        rows: Vec<SensorRow>,
    },
    Stop,
}

pub struct LogWorker {
    tx: SyncSender<Command>,
    errors: Receiver<String>,
    thread: Option<thread::JoinHandle<()>>,
}

impl LogWorker {
    pub fn start(path: impl Into<PathBuf>, format: LogFormat) -> io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let (tx, rx) = mpsc::sync_channel(4);
        let (error_tx, error_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("hwall-logger".to_owned())
            .spawn(move || {
                if let Err(error) = run_logger(&path, format, rx) {
                    let _ = error_tx.send(error.to_string());
                }
            })?;
        Ok(Self {
            tx,
            errors: error_rx,
            thread: Some(handle),
        })
    }

    pub fn sample(&self, timestamp_ms: u128, rows: Vec<SensorRow>) -> bool {
        match self.tx.try_send(Command::Sample { timestamp_ms, rows }) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    pub fn try_error(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }

    pub fn stop(mut self) {
        let _ = self.tx.send(Command::Stop);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for LogWorker {
    fn drop(&mut self) {
        let _ = self.tx.try_send(Command::Stop);
    }
}

fn run_logger(path: &Path, format: LogFormat, rx: Receiver<Command>) -> io::Result<()> {
    let mut writer = LogFileWriter::create(path, format)?;
    while let Ok(command) = rx.recv() {
        match command {
            Command::Sample { timestamp_ms, rows } => {
                writer.write_sample(timestamp_ms, &rows)?;
                writer.flush()?;
            }
            Command::Stop => break,
        }
    }
    writer.flush()
}

fn write_csv_sample(
    writer: &mut impl Write,
    timestamp_ms: u128,
    rows: &[SensorRow],
) -> io::Result<()> {
    for row in rows
        .iter()
        .filter(|row| row.kind == crate::RowKind::Sensor && row.current_sample)
    {
        writeln!(
            writer,
            "{},{},{},{},{},{},{},{},{}",
            timestamp_ms,
            escape_delimited(&row.id, ','),
            escape_delimited(&row.label, ','),
            escape_delimited(&row.original_label, ','),
            escape_delimited(&row.current, ','),
            escape_delimited(&row.minimum, ','),
            escape_delimited(&row.maximum, ','),
            escape_delimited(&row.average, ','),
            escape_delimited(&row.status, ','),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_scope_ids_round_trip() {
        for format in LogFormat::ALL {
            assert_eq!(LogFormat::from_id(format.id()), Some(format));
        }
        for scope in LogScope::ALL {
            assert_eq!(LogScope::from_id(scope.id()), Some(scope));
        }
        assert_eq!(LogFormat::from_id("unknown"), None);
        assert_eq!(LogScope::from_id("unknown"), None);
    }

    #[test]
    fn timestamped_paths_use_the_selected_format() {
        let csv = timestamped_log_path("logs", LogFormat::Csv);
        let jsonl = timestamped_log_path("logs", LogFormat::JsonLines);
        assert_eq!(csv.parent(), Some(Path::new("logs")));
        assert_eq!(
            csv.extension().and_then(|value| value.to_str()),
            Some("csv")
        );
        assert_eq!(
            jsonl.extension().and_then(|value| value.to_str()),
            Some("jsonl")
        );
    }
}
