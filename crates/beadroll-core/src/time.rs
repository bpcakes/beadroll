//! UTC timestamps derived from event ids.

/// The year 2100 in milliseconds: later timestamps are clamped to this ceiling.
pub const MAX_MILLIS: u64 = 4_102_444_800_000;

/// Milliseconds since 1970 formatted in UTC as `2026-10-09T15:04:05.123Z`, clamped to
/// [`MAX_MILLIS`].
pub fn event_time(millis: u64) -> String {
    let millis = millis.min(MAX_MILLIS);
    let (seconds, fraction) = (millis / 1000, millis % 1000);
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil(days);
    // Replay writes one for every event it applies, so it is put together digit by digit rather
    // than through `format!`: the same text, at a fraction of the cost.
    let mut out = String::with_capacity(24);
    let parts = [
        (year, 4, '-'),
        (month, 2, '-'),
        (day, 2, 'T'),
        (rest / 3600, 2, ':'),
        (rest % 3600 / 60, 2, ':'),
        (rest % 60, 2, '.'),
        (fraction, 3, 'Z'),
    ];
    for (value, width, after) in parts {
        padded(&mut out, value, width);
        out.push(after);
    }
    out
}

/// Appends `value` in decimal, zero-padded to `width` digits (`{value:0width$}`).
fn padded(out: &mut String, value: u64, width: usize) {
    let mut digits = [0u8; 20];
    let (mut rest, mut count) = (value, 0);
    for slot in &mut digits {
        *slot = b'0' + (rest % 10) as u8;
        rest /= 10;
        count += 1;
        if rest == 0 {
            break;
        }
    }
    for _ in count..width {
        out.push('0');
    }
    for digit in digits.iter().take(count).rev() {
        out.push(char::from(*digit));
    }
}

/// UTC seconds since the epoch formatted to whole seconds for last_sync, without a ceiling.
pub fn utc_stamp(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
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
    fn stamps_are_seconds_in_utc() {
        // The UTC representation of 1760047200 seconds since the epoch.
        assert_eq!(utc_stamp(1_760_047_200), "2025-10-09T22:00:00Z");
        assert_eq!(utc_stamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_stamp(4_102_444_801), "2100-01-01T00:00:01Z");
    }

    #[test]
    fn times_print_as_python_event_time() {
        assert_eq!(event_time(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(event_time(951_782_400_007), "2000-02-29T00:00:00.007Z");
        assert_eq!(event_time(1_791_558_245_123), "2026-10-09T15:04:05.123Z");
        assert_eq!(event_time(MAX_MILLIS - 1), "2099-12-31T23:59:59.999Z");
        assert_eq!(event_time(MAX_MILLIS), "2100-01-01T00:00:00.000Z");
        assert_eq!(event_time(u64::MAX >> 16), "2100-01-01T00:00:00.000Z");
    }

    #[test]
    fn times_print_as_format_would() {
        let formatted = |millis: u64| {
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
        };
        let mut millis = 0u64;
        while millis < MAX_MILLIS + 10_000 {
            assert_eq!(event_time(millis), formatted(millis), "{millis}");
            millis = millis * 3 / 2 + 7_919;
        }
        for millis in [
            1,
            9,
            10,
            99,
            100,
            999,
            1000,
            59_999,
            86_399_999,
            MAX_MILLIS - 1,
            u64::MAX,
        ] {
            assert_eq!(event_time(millis), formatted(millis), "{millis}");
        }
        let mut padded_text = String::new();
        padded(&mut padded_text, 12_345, 3);
        padded(&mut padded_text, 7, 3);
        padded(&mut padded_text, 0, 0);
        assert_eq!(padded_text, "123450070");
    }
}
