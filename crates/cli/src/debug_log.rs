use serde_json::Value;
use std::io::Write;
use std::sync::OnceLock;

pub fn emit(run_id: &str, hypothesis_id: &str, location: &str, message: &str, data: Value) {
    if std::env::var("ACE_DEBUG_LOG").is_err() {
        return;
    }

    let payload = serde_json::json!({
        "sessionId": "abcbb0",
        "runId": run_id,
        "hypothesisId": hypothesis_id,
        "location": location,
        "message": message,
        "data": data,
        "timestamp": chrono_ms(),
    });

    let log_path = log_path();
    announce_path_once(&log_path);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        && let Ok(line) = serde_json::to_string(&payload)
    {
        let _ = writeln!(f, "{}", line);
    }
}

fn announce_path_once(path: &std::path::Path) {
    static ONCE: OnceLock<()> = OnceLock::new();
    let _ = ONCE.get_or_init(|| {
        eprintln!("[ace debug] writing debug logs to {}", path.display());
    });
}

fn log_path() -> std::path::PathBuf {
    std::env::var("ACE_DEBUG_LOG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("debug.log"))
}

fn chrono_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
