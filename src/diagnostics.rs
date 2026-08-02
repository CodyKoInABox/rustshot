//! Small local diagnostic log and support-report generator.

use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::PathBuf,
    sync::Mutex,
};

use chrono::{Local, SecondsFormat};
use rustshot::config::{config_path, Config};

const LOG_FILE_NAME: &str = "rustshot.log";
const OLD_LOG_FILE_NAME: &str = "rustshot.old.log";
const REPORT_FILE_NAME: &str = "rustshot-diagnostics.txt";
const MAX_LOG_BYTES: u64 = 1024 * 1024;
const MAX_LOG_MESSAGE_CHARS: usize = 8192;
const REPORT_LOG_LINES: usize = 40;
static LOG_LOCK: Mutex<()> = Mutex::new(());

/// Initializes bounded local logging and records the current application version.
pub fn initialize() {
    let _guard = LOG_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if let Err(error) = initialize_inner() {
        eprintln!("Rustshot could not initialize diagnostics: {error}");
    }
}

fn initialize_inner() -> io::Result<()> {
    let directory = directory();
    let log_path = prepare_log(&directory)?;
    append_line(
        &log_path,
        &format!("started Rustshot {}", env!("CARGO_PKG_VERSION")),
    )
}

fn prepare_log(directory: &std::path::Path) -> io::Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let log_path = directory.join(LOG_FILE_NAME);
    if fs::metadata(&log_path).is_ok_and(|metadata| metadata.len() >= MAX_LOG_BYTES) {
        let old_log = directory.join(OLD_LOG_FILE_NAME);
        if old_log.exists() {
            fs::remove_file(&old_log)?;
        }
        fs::rename(&log_path, old_log)?;
    }
    Ok(log_path)
}

/// Records an error without collecting screenshots or clipboard contents.
pub fn record_error(title: &str, description: &str) {
    record(&format!(
        "ERROR {title}: {}",
        description.replace(['\r', '\n'], " ")
    ));
}

/// Records a concise application lifecycle event.
pub fn record(message: &str) {
    let _guard = LOG_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let directory = directory();
    if let Ok(log_path) = prepare_log(&directory) {
        let bounded: String = message.chars().take(MAX_LOG_MESSAGE_CHARS).collect();
        let _ = append_line(&log_path, &bounded);
    }
}

fn append_line(path: &std::path::Path, message: &str) -> io::Result<()> {
    let timestamp = Local::now().to_rfc3339_opts(SecondsFormat::Millis, false);
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{timestamp} {message}")
}

/// Writes a support report beside the bounded log and returns its path.
pub fn write_report(config: &Config) -> io::Result<PathBuf> {
    let _guard = LOG_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let directory = directory();
    fs::create_dir_all(&directory)?;
    let log_path = directory.join(LOG_FILE_NAME);
    let recent_log = fs::read_to_string(&log_path)
        .ok()
        .map(|log| tail_lines(&log, REPORT_LOG_LINES))
        .unwrap_or_else(|| "No diagnostic log is available.\n".to_owned());
    let report = report_text(config, &log_path, &recent_log);
    let report_path = directory.join(REPORT_FILE_NAME);
    fs::write(&report_path, report)?;
    Ok(report_path)
}

/// Returns the directory containing Rustshot's log and generated support report.
pub fn directory() -> PathBuf {
    config_path()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::env::temp_dir().join("Rustshot"))
}

fn tail_lines(contents: &str, count: usize) -> String {
    let lines: Vec<_> = contents.lines().collect();
    let start = lines.len().saturating_sub(count);
    let mut tail = lines[start..].join("\n");
    tail.push('\n');
    tail
}

fn report_text(config: &Config, log_path: &std::path::Path, recent_log: &str) -> String {
    let mut report = String::new();
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let _ = writeln!(report, "Rustshot diagnostics");
    let _ = writeln!(report, "====================");
    let _ = writeln!(report, "Version: {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        report,
        "Target: {}-{}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );
    let _ = writeln!(report, "Build profile: {profile}");
    let _ = writeln!(report, "Configuration schema: {}", config.config_version);
    let _ = writeln!(
        report,
        "Configuration path: {}",
        config_path().map_or_else(
            |error| format!("unavailable ({error})"),
            |path| path.display().to_string()
        )
    );
    let _ = writeln!(report, "Diagnostic log: {}", log_path.display());
    let _ = writeln!(
        report,
        "Autosave directory: {}",
        config.autosave_directory.display()
    );
    let _ = writeln!(
        report,
        "Full-screen shortcut: {}",
        config.fullscreen_shortcut
    );
    let _ = writeln!(report, "Region shortcut: {}", config.region_shortcut);
    let _ = writeln!(report, "Image format: {:?}", config.image_format);
    let _ = writeln!(report, "Monitor scope: {:?}", config.monitor_scope);
    let _ = writeln!(report, "Include cursor: {}", config.include_cursor);
    let _ = writeln!(report, "Region auto-copy: {}", config.region_auto_copy);
    let _ = writeln!(report, "Region autosave: {}", config.region_autosave);
    let _ = writeln!(report);
    let _ = writeln!(report, "Recent diagnostic log");
    let _ = writeln!(report, "---------------------");
    report.push_str(recent_log);
    report
}

#[cfg(test)]
mod tests {
    use super::{report_text, tail_lines};
    use rustshot::config::Config;
    use std::path::Path;

    #[test]
    fn diagnostic_report_contains_version_schema_and_recent_log() {
        let report = report_text(
            &Config::default(),
            Path::new(r"C:\Rustshot\rustshot.log"),
            "a recent error\n",
        );

        assert!(report.contains("Version: 1.0.0"));
        assert!(report.contains("Configuration schema: 1"));
        assert!(report.contains("a recent error"));
    }

    #[test]
    fn log_tail_is_bounded_to_the_requested_line_count() {
        assert_eq!(tail_lines("one\ntwo\nthree\n", 2), "two\nthree\n");
    }
}
