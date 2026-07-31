use chrono::{DateTime, Duration, Local, Utc};

/// 当前时间戳（毫秒）
pub fn current_time_millis() -> i64 {
    Utc::now().timestamp_millis()
}

/// 当前时间戳（纳秒）
pub fn current_time_nanos() -> i64 {
    Utc::now().timestamp_nanos_opt().unwrap_or(0)
}

/// 格式化时间戳为字符串
pub fn format_timestamp(ts_millis: i64) -> String {
    if let Some(dt) = DateTime::from_timestamp_millis(ts_millis) {
        dt.with_timezone(&Local)
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string()
    } else {
        "N/A".to_string()
    }
}

/// 格式化 Duration 为可读字符串
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.2}s", ms as f64 / 1000.0)
    } else {
        let secs = ms / 1000;
        format!("{}m{}s", secs / 60, secs % 60)
    }
}

/// 计算已经过的时间（毫秒）
pub fn elapsed_millis(since_millis: i64) -> u64 {
    let elapsed = current_time_millis() - since_millis;
    if elapsed < 0 { 0 } else { elapsed as u64 }
}

/// 两个时间戳之间的 Duration
pub fn duration_between(start_ms: i64, end_ms: i64) -> Duration {
    Duration::milliseconds(end_ms - start_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_time_is_reasonable() {
        let ts = current_time_millis();
        assert!(ts > 1700000000000); // after 2023
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration_ms(500), "500ms");
        assert!(format_duration_ms(5000).contains('s'));
        assert!(format_duration_ms(120000).contains('m'));
    }

    #[test]
    fn test_elapsed() {
        let past = current_time_millis() - 1000;
        let elapsed = elapsed_millis(past);
        assert!(elapsed >= 1000);
    }
}
