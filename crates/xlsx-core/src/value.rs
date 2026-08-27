//! Cell values.
//!
//! The typing decision that matters is the date one. In an XLSX a date is a
//! number plus a `numFmt` declared in `xl/styles.xml`; without that table
//! `45376` is indistinguishable from a quantity. `calamine` wires the format
//! table into its cell reader, so the distinction reaches us — this module's
//! job is to hand it on without ever going through a locale.

use calamine::{DataRef, ExcelDateTime};

/// A single cell, typed.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    /// No cell at this position, or a cell holding nothing.
    Empty,
    /// Text: a shared string, an inline string, or a cached string result.
    Text(String),
    /// Any number that is not formatted as a date.
    Number(f64),
    /// `TRUE` or `FALSE`.
    Bool(bool),
    /// A date, time or datetime, rendered as ISO 8601 in UTC — never a locale.
    ///
    /// XLSX stores no timezone, so the stored wall clock is reported verbatim
    /// with a `Z` suffix: what the sheet shows is what comes out.
    DateTime(String),
    /// A cell holding an Excel error, as its sheet spelling (`#DIV/0!`, `#N/A`).
    Error(String),
}

impl CellValue {
    /// Convert a `calamine` cell, borrowed from the workbook's shared string
    /// table, into an owned value.
    pub(crate) fn from_data_ref(value: DataRef<'_>) -> Self {
        match value {
            DataRef::Empty => Self::Empty,
            DataRef::Int(v) => Self::Number(v as f64),
            DataRef::Float(v) => Self::Number(v),
            DataRef::String(v) => Self::Text(v),
            DataRef::SharedString(v) => Self::Text(v.to_owned()),
            DataRef::Bool(v) => Self::Bool(v),
            DataRef::Error(v) => Self::Error(v.to_string()),
            // A `[hh]:mm:ss` cell is an elapsed time, not a point in time.
            // Reporting it as a datetime would put it on some arbitrary day.
            DataRef::DateTime(v) if v.is_duration() => Self::Text(iso_duration(v.as_f64())),
            DataRef::DateTime(v) => Self::DateTime(iso_datetime(&v)),
            DataRef::DateTimeIso(v) => Self::DateTime(v),
            DataRef::DurationIso(v) => Self::Text(v),
        }
    }
}

/// Render an Excel serial as `YYYY-MM-DDTHH:MM:SS.sssZ`.
///
/// `to_ymd_hms_milli` already accounts for the workbook's date system, the 1904
/// one included, and for the 1900 leap year bug. It reads no clock and no
/// timezone, which is what makes this reproducible across machines.
fn iso_datetime(value: &ExcelDateTime) -> String {
    let (year, month, day, hour, minute, second, milli) = value.to_ymd_hms_milli();
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}Z")
}

/// Render an elapsed time, in days, as an ISO 8601 duration.
///
/// Hours are not wrapped at 24: `[hh]:mm:ss` counts total hours, and so does
/// `PT30H`.
fn iso_duration(days: f64) -> String {
    let negative = days < 0.0;
    let total_millis = (days.abs() * 86_400_000.0).round() as u64;

    let millis = total_millis % 1_000;
    let total_seconds = total_millis / 1_000;
    let seconds = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = total_seconds / 3_600;

    let sign = if negative { "-" } else { "" };
    if millis == 0 {
        format!("{sign}PT{hours}H{minutes}M{seconds}S")
    } else {
        format!("{sign}PT{hours}H{minutes}M{seconds}.{millis:03}S")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::ExcelDateTimeType;

    #[test]
    fn renders_a_date_at_midnight_utc() {
        let value = ExcelDateTime::new(45376.0, ExcelDateTimeType::DateTime, false);
        assert_eq!(iso_datetime(&value), "2024-03-25T00:00:00.000Z");
    }

    #[test]
    fn renders_the_fractional_part_as_a_time() {
        let value = ExcelDateTime::new(45376.5, ExcelDateTimeType::DateTime, false);
        assert_eq!(iso_datetime(&value), "2024-03-25T12:00:00.000Z");
    }

    #[test]
    fn the_1904_system_lands_on_the_same_day() {
        let system_1900 = ExcelDateTime::new(45376.0, ExcelDateTimeType::DateTime, false);
        let system_1904 = ExcelDateTime::new(45376.0 - 1462.0, ExcelDateTimeType::DateTime, true);
        assert_eq!(iso_datetime(&system_1900), iso_datetime(&system_1904));
    }

    #[test]
    fn durations_do_not_wrap_at_twenty_four_hours() {
        assert_eq!(iso_duration(1.25), "PT30H0M0S");
        assert_eq!(iso_duration(0.0), "PT0H0M0S");
        assert_eq!(iso_duration(-0.5), "-PT12H0M0S");
    }

    #[test]
    fn durations_keep_milliseconds() {
        assert_eq!(iso_duration(1.5 / 86_400.0), "PT0H0M1.500S");
    }
}
