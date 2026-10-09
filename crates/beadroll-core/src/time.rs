//! Event times: what an event id's timestamp reads as, the way Python's `event_time` prints it.

/// The year 2100 in milliseconds: a later timestamp is not a clock reading (Python's `MAX_MILLIS`).
pub const MAX_MILLIS: u64 = 4_102_444_800_000;

/// `millis` since 1970 in UTC, as `2026-10-09T15:04:05.123Z`; anything past [`MAX_MILLIS`] reads as
/// that instant, as Python's `event_time` clamps it.
pub fn event_time(millis: u64) -> String {
    let millis = millis.min(MAX_MILLIS);
    let (seconds, fraction) = (millis / 1000, millis % 1000);
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{fraction:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + u64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_print_as_python_event_time() {
        assert_eq!(event_time(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(event_time(951_782_400_007), "2000-02-29T00:00:00.007Z");
        assert_eq!(event_time(1_791_558_245_123), "2026-10-09T15:04:05.123Z");
        assert_eq!(event_time(MAX_MILLIS - 1), "2099-12-31T23:59:59.999Z");
        assert_eq!(event_time(MAX_MILLIS), "2100-01-01T00:00:00.000Z");
        assert_eq!(event_time(u64::MAX >> 16), "2100-01-01T00:00:00.000Z");
    }
}
