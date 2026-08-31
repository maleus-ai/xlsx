//! ISO 8601 in, Excel serial out.
//!
//! The reader renders every date as `YYYY-MM-DDTHH:MM:SS.sssZ` and never goes
//! through a locale. This module is the same decision taken in the other
//! direction: the caller hands over a string it can read, and what lands in the
//! sheet is a serial plus a format, which is the only spelling Excel treats as a
//! date at all.
//!
//! The shape of the string decides the format the cell gets. `2024-03-25` is a
//! date and is shown as one; `2024-03-25T14:30:00Z` carries a time and is shown
//! with it. Guessing instead — showing a date because the time happens to be
//! midnight — would make the output depend on the data rather than on what the
//! caller wrote.

use rust_xlsxwriter::ExcelDateTime;

use super::error::WriteError;

/// A parsed timestamp, tagged with how it should be shown.
///
/// The tag is what picks the number format, and it comes from the string the
/// caller wrote rather than from the value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Stamp {
    /// `YYYY-MM-DD`.
    Date,
    /// `YYYY-MM-DD` plus a time.
    DateTime,
    /// A time on its own, `HH:MM:SS`.
    Time,
}

/// Parse an ISO 8601 timestamp into an Excel serial and the format it needs.
///
/// Accepts a date, a date and time separated by `T` or a space, and a bare
/// time. A trailing `Z` is allowed and means what the reader means by it: the
/// wall clock as written. Any other UTC offset is refused rather than dropped —
/// XLSX stores no timezone, so honouring `+02:00` would mean silently shifting
/// the value, and ignoring it would mean silently keeping the wrong one.
pub(crate) fn parse(input: &str) -> Result<(ExcelDateTime, Stamp), WriteError> {
    let text = input.trim();
    let body = strip_zone(text, input)?;

    let (date_part, time_part) = match body.find(['T', 't', ' ']) {
        Some(index) => (&body[..index], Some(&body[index + 1..])),
        None => (body, None),
    };

    // A bare time has colons where a date would have dashes.
    if time_part.is_none() && date_part.contains(':') {
        let (hour, minute, second) = time_fields(date_part, input)?;
        let stamp = ExcelDateTime::from_hms(hour, minute, second)
            .map_err(|error| invalid(input, error.to_string()))?;
        return Ok((stamp, Stamp::Time));
    }

    let (year, month, day) = date_fields(date_part, input)?;
    let date =
        ExcelDateTime::from_ymd(year, month, day).map_err(|e| invalid(input, e.to_string()))?;

    let Some(time_part) = time_part else {
        return Ok((date, Stamp::Date));
    };

    // A `T` with nothing after it is a truncated timestamp, not a date.
    if time_part.is_empty() {
        return Err(invalid(input, "a time separator with no time after it"));
    }

    let (hour, minute, second) = time_fields(time_part, input)?;
    let stamp = date
        .and_hms(hour, minute, second)
        .map_err(|error| invalid(input, error.to_string()))?;

    Ok((stamp, Stamp::DateTime))
}

/// Remove a trailing `Z`, and refuse any other offset.
fn strip_zone<'a>(text: &'a str, input: &str) -> Result<&'a str, WriteError> {
    if let Some(body) = text.strip_suffix(['Z', 'z']) {
        return Ok(body);
    }

    // `+02:00` or `-05:00`, but not the `-` inside `2024-03-25`: an offset sign
    // only ever appears after a time, so there is a colon ahead of it.
    let offset = text
        .char_indices()
        .skip(1)
        .find(|&(index, character)| {
            (character == '+' || character == '-') && text[..index].contains(':')
        })
        .is_some();

    if offset {
        return Err(invalid(
            input,
            "a UTC offset, and XLSX stores no timezone; convert to the wall \
             clock you want shown and pass it with a trailing Z, or with none",
        ));
    }

    Ok(text)
}

fn date_fields(part: &str, input: &str) -> Result<(u16, u8, u8), WriteError> {
    let mut fields = part.split('-');
    let year = number::<u16>(fields.next(), input, "year")?;
    let month = number::<u8>(fields.next(), input, "month")?;
    let day = number::<u8>(fields.next(), input, "day")?;

    if fields.next().is_some() {
        return Err(invalid(input, "more than three date fields"));
    }

    Ok((year, month, day))
}

fn time_fields(part: &str, input: &str) -> Result<(u16, u8, f64), WriteError> {
    let mut fields = part.split(':');
    let hour = number::<u16>(fields.next(), input, "hour")?;
    let minute = number::<u8>(fields.next(), input, "minute")?;

    // Seconds carry the fraction, so they are parsed as a float and the
    // milliseconds the reader emits survive the round trip.
    let second = match fields.next() {
        None => 0.0,
        Some(text) => text
            .parse::<f64>()
            .map_err(|_| invalid(input, format!("{text:?} is not a number of seconds")))?,
    };

    if fields.next().is_some() {
        return Err(invalid(input, "more than three time fields"));
    }
    if !second.is_finite() || second < 0.0 {
        return Err(invalid(input, "a second that is not a finite count"));
    }

    Ok((hour, minute, second))
}

fn number<T: std::str::FromStr>(
    field: Option<&str>,
    input: &str,
    name: &str,
) -> Result<T, WriteError> {
    let Some(text) = field else {
        return Err(invalid(input, format!("no {name}")));
    };
    text.parse()
        .map_err(|_| invalid(input, format!("{text:?} is not a {name}")))
}

fn invalid(input: &str, detail: impl Into<String>) -> WriteError {
    WriteError::InvalidDateTime {
        value: input.to_owned(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial(input: &str) -> f64 {
        parse(input)
            .map_err(|e| e.to_string())
            .expect("parses")
            .0
            .to_excel()
    }

    /// `ExcelDateTime` carries no `Debug`, so the success value is dropped
    /// before the error is taken out.
    fn refused(input: &str) -> WriteError {
        parse(input).map(|_| ()).unwrap_err()
    }

    fn is_refused(input: &str) -> bool {
        parse(input).map(|_| ()).is_err()
    }

    #[test]
    fn a_date_keeps_the_serial_the_reader_would_report() {
        // The reader's own tests pin 45376 to 2024-03-25; this is the same
        // number arrived at from the other side.
        assert_eq!(serial("2024-03-25"), 45376.0);
        assert_eq!(
            parse("2024-03-25").map_err(|e| e.to_string()).unwrap().1,
            Stamp::Date
        );
    }

    #[test]
    fn a_time_lands_on_the_fraction_of_the_day() {
        assert_eq!(serial("2024-03-25T12:00:00"), 45376.5);
        assert_eq!(
            parse("2024-03-25T12:00:00")
                .map_err(|e| e.to_string())
                .unwrap()
                .1,
            Stamp::DateTime
        );
    }

    #[test]
    fn the_reader_s_own_spelling_round_trips() {
        assert_eq!(serial("2024-03-25T00:00:00.000Z"), 45376.0);
    }

    #[test]
    fn milliseconds_survive() {
        let with_millis = serial("2024-03-25T00:00:00.500Z");
        assert!((with_millis - 45376.0 - 0.5 / 86_400.0).abs() < 1e-9);
    }

    #[test]
    fn a_space_separator_is_accepted() {
        assert_eq!(serial("2024-03-25 12:00:00"), 45376.5);
    }

    #[test]
    fn a_bare_time_is_a_time() {
        assert_eq!(
            parse("12:00:00").map_err(|e| e.to_string()).unwrap().1,
            Stamp::Time
        );
        assert_eq!(serial("12:00:00"), 0.5);
    }

    #[test]
    fn an_offset_is_refused_rather_than_dropped() {
        // Honouring it would shift the value, ignoring it would keep the wrong
        // one. Neither is something to do quietly.
        let error = refused("2024-03-25T12:00:00+02:00");
        assert!(matches!(error, WriteError::InvalidDateTime { .. }));
        assert!(error.to_string().contains("no timezone"), "{error}");
    }

    #[test]
    fn a_negative_offset_is_refused_too() {
        assert!(is_refused("2024-03-25T12:00:00-05:00"));
    }

    #[test]
    fn rubbish_is_refused_with_the_value_in_the_message() {
        let error = refused("not a date");
        assert!(error.to_string().contains("not a date"), "{error}");

        assert!(is_refused("2024-13-01"), "month 13");
        assert!(is_refused("2024-02-30"), "30 February");
        assert!(is_refused("2024-03-25T"), "truncated");
        assert!(is_refused(""), "empty");
    }
}
