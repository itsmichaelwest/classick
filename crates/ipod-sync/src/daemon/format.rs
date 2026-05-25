//! RFC3339 timestamp emission for history entries. Hand-rolled so we
//! don't take a chrono dep just for this. Format is the strict subset
//! `YYYY-MM-DDTHH:MM:SSZ` (UTC, second precision, no fractional, no
//! offset variations).

/// Format an absolute unix-second timestamp as RFC3339 UTC.
/// Example: 1779559179 -> "2026-05-23T17:59:39Z".
pub fn rfc3339(unix_secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = unix_to_ymdhms(unix_secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Current time as RFC3339 UTC string. Convenience wrapper.
pub fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339(secs)
}

/// Convert unix seconds to (year, month, day, hour, minute, second)
/// in UTC. Public for testing.
pub fn unix_to_ymdhms(unix_secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let hh = (secs_of_day / 3600) as u32;
    let mm = ((secs_of_day % 3600) / 60) as u32;
    let ss = (secs_of_day % 60) as u32;

    // Civil-from-days, Howard Hinnant's algorithm:
    // http://howardhinnant.github.io/date_algorithms.html#civil_from_days
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64 + era * 400) as u32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hh, mm, ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formats_correctly() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamp_formats_correctly() {
        // 2026-05-23T17:59:39Z = 1779559179 (the user-encountered
        // timestamp from the M3 smoke log).
        assert_eq!(rfc3339(1_779_559_179), "2026-05-23T17:59:39Z");
    }

    #[test]
    fn leap_day_2024_formats_correctly() {
        // 2024-02-29T00:00:00Z = 1709164800
        assert_eq!(rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn non_leap_century_2100_formats_correctly() {
        // 2100-03-01T00:00:00Z = 4107542400 (2100 is NOT a leap year
        // — divisible by 100 but not 400).
        assert_eq!(rfc3339(4_107_542_400), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_now_is_well_formed() {
        let s = rfc3339_now();
        assert!(s.len() == 20, "expected 20-char fixed length, got: {s}");
        assert!(s.ends_with('Z'), "expected trailing Z, got: {s}");
        assert!(s.chars().nth(4) == Some('-'), "expected dash at pos 4: {s}");
        assert!(s.chars().nth(10) == Some('T'), "expected T at pos 10: {s}");
    }
}
