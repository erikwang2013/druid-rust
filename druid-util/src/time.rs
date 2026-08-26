use chrono::{DateTime, Duration, Local, Utc};

/// 当前时间戳（毫秒）
pub fn current_time_millis() -> i64 {
    Utc::now().timestamp_millis()
}

/// 当前时间戳（纳秒）
pub fn current_time_nanos() -> i64 {
    Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| Utc::now().timestamp_millis() * 1_000_000)
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
    if elapsed < 0 {
        0
    } else {
        elapsed as u64
    }
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
        assert!(ts < 3000000000000); // before 2065（防极端回归）
    }

    #[test]
    fn test_current_time_nanos() {
        let nanos = current_time_nanos();
        let millis = current_time_millis();
        // 纳秒精度读数总是 >= 毫秒读数 * 1e6
        assert!(nanos >= millis * 1_000_000);
        // 且在同一秒量级内（差值 < 1 秒）
        assert!(nanos - millis * 1_000_000 < 1_000_000_000);
    }

    #[test]
    fn test_format_timestamp() {
        // ts=0 → 本地时区 1970-01-01 00:00:00.000（或 UTC- 时区为 1969-12-31）
        let s = format_timestamp(0);
        assert_eq!(s.len(), 23);
        assert!(s.ends_with(".000"));
        assert!(s.starts_with("1969-12-31") || s.starts_with("1970-01-01"));
        // 常规时间戳格式校验
        let now = format_timestamp(current_time_millis());
        assert!(now.len() == 23 && now.contains('-') && now.contains(':'));
    }

    #[test]
    fn test_format_timestamp_out_of_range() {
        assert_eq!(format_timestamp(i64::MAX), "N/A");
        assert_eq!(format_timestamp(i64::MIN), "N/A");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration_ms(500), "500ms");
        assert_eq!(format_duration_ms(0), "0ms");
        assert_eq!(format_duration_ms(999), "999ms");
        assert_eq!(format_duration_ms(1000), "1.00s");
        assert_eq!(format_duration_ms(59_999), "60.00s"); // 四舍五入进位
        assert_eq!(format_duration_ms(60_000), "1m0s");
        assert_eq!(format_duration_ms(61_500), "1m1s");
        assert_eq!(format_duration_ms(3_660_000), "61m0s");
    }

    #[test]
    fn test_elapsed() {
        let past = current_time_millis() - 1000;
        let elapsed = elapsed_millis(past);
        assert!(elapsed >= 1000);
    }

    #[test]
    fn test_elapsed_future_clamps_to_zero() {
        let future = current_time_millis() + 1000;
        assert_eq!(elapsed_millis(future), 0);
    }

    #[test]
    fn test_duration_between() {
        assert_eq!(duration_between(1000, 2000), Duration::milliseconds(1000));
        assert_eq!(duration_between(2000, 1000), Duration::milliseconds(-1000));
        assert_eq!(duration_between(5000, 5000), Duration::milliseconds(0));
    }
}
