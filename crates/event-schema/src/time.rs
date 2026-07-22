//! RFC 3339 timestamp formatting — dependency-free.
//!
//! Every producer of an event `ts` (the vendor adapters, the
//! mock-harness, and the orchestrator) shares THIS one implementation.
//! Each crate previously vendored its own copy of Howard Hinnant's
//! civil-from-days algorithm "to keep dependency footprints small";
//! one of those copies drifted by a wrong epoch constant (`719_162`
//! instead of `719_468`) and silently back-dated an entire vendor's
//! events by ~10 months. A single source of truth makes that class of
//! bug unrepresentable.
//! The upstream MIT attribution is retained in `THIRD_PARTY_NOTICES.md` and
//! `third_party_licenses/howard-hinnant-date-MIT.txt`.
//!
//! Pure integer arithmetic over `std` only — keeps this crate
//! runtime-free, exactly as the rest of `event-schema` is.

/// Format a Unix-epoch millisecond count as an RFC 3339 UTC string with
/// millisecond precision. Always emits the trailing `Z`.
///
/// Matches the `ts` format the event envelope requires
/// (`"ts": "2026-05-08T19:42:13.481Z"`). Correct for every Gregorian
/// date the program might encounter.
pub fn rfc3339_from_unix_ms(ms: u64) -> String {
    let total_secs = ms / 1000;
    let ms_part = ms % 1000;
    let days = (total_secs / 86_400) as i64;
    let secs_of_day = total_secs % 86_400;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;

    // civil_from_days — see https://howardhinnant.github.io/date_algorithms.html
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y_zero_based = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 {
        y_zero_based + 1
    } else {
        y_zero_based
    };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hour, minute, second, ms_part
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_zero() {
        assert_eq!(rfc3339_from_unix_ms(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn one_second_past_epoch() {
        assert_eq!(rfc3339_from_unix_ms(1000), "1970-01-01T00:00:01.000Z");
    }

    #[test]
    fn jan_1_2020() {
        // 1577836800 unix seconds = 2020-01-01T00:00:00 UTC
        assert_eq!(
            rfc3339_from_unix_ms(1_577_836_800_000),
            "2020-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn leap_day_2020() {
        // 1582934400 unix seconds = 2020-02-29T00:00:00 UTC
        assert_eq!(
            rfc3339_from_unix_ms(1_582_934_400_000),
            "2020-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn milliseconds_are_zero_padded() {
        assert_eq!(rfc3339_from_unix_ms(123), "1970-01-01T00:00:00.123Z");
        assert_eq!(rfc3339_from_unix_ms(7), "1970-01-01T00:00:00.007Z");
        assert_eq!(rfc3339_from_unix_ms(1_500), "1970-01-01T00:00:01.500Z");
    }

    #[test]
    fn boundary_to_second_day() {
        // 86400000 ms = exactly 1 day after epoch.
        assert_eq!(rfc3339_from_unix_ms(86_400_000), "1970-01-02T00:00:00.000Z");
    }

    #[test]
    fn ms_just_under_day_boundary() {
        assert_eq!(
            rfc3339_from_unix_ms(86_400_000 - 1),
            "1970-01-01T23:59:59.999Z"
        );
    }

    #[test]
    fn date_in_2026() {
        // 2026-01-01T00:00:00.000Z = 1767225600000 ms unix.
        assert_eq!(
            rfc3339_from_unix_ms(1_767_225_600_000),
            "2026-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn output_strings_compare_lexicographically_in_time_order() {
        // The schema field is a string, and replay queries sort
        // lexicographically; a fixed-width format keeps that ordering
        // chronological.
        let earlier = rfc3339_from_unix_ms(1_577_836_800_000);
        let later = rfc3339_from_unix_ms(1_582_934_400_000);
        assert!(earlier < later);
    }
}
