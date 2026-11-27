//! Timestamps. Every time the store records is a UTC string of one fixed
//! shape, `YYYY-MM-DDTHH:MM:SS.mmmZ`, so that string order is time order.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Error, Result};

/// Where "now" comes from.
#[derive(Debug, Clone, Default)]
pub enum Clock {
    /// The system clock.
    #[default]
    System,
    /// A fixed instant, for reproducible runs and tests.
    Fixed(String),
}

impl Clock {
    /// A clock fixed at `instant`, which must be a UTC timestamp such as
    /// `2026-11-27T12:00:00Z`.
    pub fn fixed(instant: &str) -> Result<Clock> {
        Ok(Clock::Fixed(normalise(instant)?))
    }

    /// The current instant.
    pub fn now(&self) -> String {
        match self {
            Clock::Fixed(instant) => instant.clone(),
            Clock::System => {
                let since_epoch = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default();
                format_utc(since_epoch.as_secs(), since_epoch.subsec_millis())
            }
        }
    }
}

/// Formats seconds since the Unix epoch as a UTC timestamp.
fn format_utc(seconds: u64, millis: u32) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3_600,
        (rest % 3_600) / 60,
        rest % 60
    )
}

/// Brings a UTC timestamp or a plain date to the fixed shape. Accepts
/// `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM:SSZ` and fractional seconds.
pub fn normalise(instant: &str) -> Result<String> {
    let invalid = || {
        Error::InvalidInput(format!(
            "`{instant}` is not a UTC date or timestamp such as 2026-11-27 or 2026-11-27T12:00:00Z"
        ))
    };
    let digits =
        |text: &str, len: usize| text.len() == len && text.bytes().all(|b| b.is_ascii_digit());
    let (date, time) = match instant.split_once('T') {
        Some((date, time)) => (date, time.strip_suffix('Z').ok_or_else(invalid)?),
        None => (instant, "00:00:00"),
    };
    let date_parts: Vec<&str> = date.split('-').collect();
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let clock_parts: Vec<&str> = clock.split(':').collect();
    let valid = date_parts.len() == 3
        && digits(date_parts[0], 4)
        && digits(date_parts[1], 2)
        && digits(date_parts[2], 2)
        && clock_parts.len() == 3
        && clock_parts.iter().all(|part| digits(part, 2))
        && !fraction.is_empty()
        && fraction.bytes().all(|b| b.is_ascii_digit());
    if !valid {
        return Err(invalid());
    }
    let millis: String = fraction.chars().chain("000".chars()).take(3).collect();
    Ok(format!("{date}T{clock}.{millis}Z"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_utc(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_utc(951_782_400, 7), "2000-02-29T00:00:00.007Z");
        assert_eq!(format_utc(1_795_780_800, 0), "2026-11-27T12:00:00.000Z");
    }

    #[test]
    fn normalises_dates_and_timestamps() {
        assert_eq!(normalise("2022-03-08").unwrap(), "2022-03-08T00:00:00.000Z");
        assert_eq!(
            normalise("2022-03-08T12:00:00Z").unwrap(),
            "2022-03-08T12:00:00.000Z"
        );
        assert_eq!(
            normalise("2023-06-13T13:10:24.5Z").unwrap(),
            "2023-06-13T13:10:24.500Z"
        );
        for bad in [
            "",
            "yesterday",
            "2022-3-8",
            "2022-03-08T12:00:00",
            "2022-03-08T12:00Z",
        ] {
            assert!(normalise(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn string_order_is_time_order() {
        let earlier = normalise("2022-03-07T12:00:00Z").unwrap();
        let later = normalise("2022-03-08").unwrap();
        assert!(earlier < later);
    }
}
