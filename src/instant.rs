//! Instants.
//!
//! A hand written reader and writer in place of the Python
//! datetime machinery. The reader accepts the family the Python
//! fromisoformat accepted, as probed against the Python runtime
//! before the port was written, extended and basic and week
//! calendar dates, any single character as the separator between
//! the date and the time, a fraction written with a dot or a
//! comma and truncated past six digits, and offsets whose hour
//! and minute and second components normalize inside a day. A
//! fraction is a fraction of a second wherever it stands, after
//! the hour or the minute or the second of the time or of the
//! offset, the Python parser's own reading. An offset fraction
//! counts only when some whole component of the offset is
//! nonzero and is dropped when they are all zero, again the
//! Python parser's own reading, probed case by case.

use std::cmp::Ordering;

use crate::value::{corrupt, py_repr_str, py_repr_value, Error, Result, Value};

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Days since the epoch day 1970-01-01 for a civil date, the
/// standard era based conversion.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let month_prime = (month as i64 + 9).rem_euclid(12);
    let day_of_year = (153 * month_prime + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era - 719468
}

/// The civil date standing at a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719468;
    let era = shifted.div_euclid(146097);
    let day_of_era = shifted - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// The ISO weekday of a day count, Monday 1 through Sunday 7.
/// The epoch day was a Thursday.
fn iso_weekday(days: i64) -> u32 {
    ((days + 3).rem_euclid(7) + 1) as u32
}

fn weeks_in_iso_year(year: i64) -> u32 {
    let jan1 = iso_weekday(days_from_civil(year, 1, 1));
    if jan1 == 4 || (jan1 == 3 && is_leap_year(year)) {
        53
    } else {
        52
    }
}

/// An instant, a wall reading with the offset it was read under.
/// Equality and order judge the UTC instant itself, as the Python
/// datetime comparison does, so two spellings of one instant
/// compare equal.
#[derive(Clone, Copy, Debug)]
pub struct Instant {
    days: i64,
    secs: u32,
    micros: u32,
    offset_micros: Option<i64>,
}

impl Instant {
    /// Build an instant from civil parts, refusing any part
    /// outside the ranges the parser itself accepts, so a caller
    /// handing absurd parts gets a typed refusal instead of a
    /// computation that panics or wraps. The ranges are the
    /// years 1 through 9999, the month and day of a real date,
    /// an hour under 24, a minute and a second under 60,
    /// microseconds under a second, and an offset under a day
    /// in either direction. The offset is the offset from UTC
    /// in microseconds, or None for a naive reading. The eight
    /// parts are the fields of the reading itself, so they stand
    /// as parameters rather than a parts structure.
    #[allow(clippy::too_many_arguments)]
    pub fn from_civil(
        year: i64,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        micros: u32,
        offset_micros: Option<i64>,
    ) -> Result<Instant> {
        let offset_in_range = match offset_micros {
            None => true,
            Some(offset) => offset > -86_400_000_000 && offset < 86_400_000_000,
        };
        let parts_in_range = (1..=9999).contains(&year)
            && (1..=12).contains(&month)
            && day >= 1
            && day <= days_in_month(year, month)
            && hour <= 23
            && minute <= 59
            && second <= 59
            && micros <= 999_999
            && offset_in_range;
        if !parts_in_range {
            return Err(Error::InvalidRequest(
                "the civil parts do not form an instant the switch can record".to_string(),
            ));
        }
        Ok(Instant {
            days: days_from_civil(year, month, day),
            secs: hour * 3600 + minute * 60 + second,
            micros,
            offset_micros,
        })
    }

    /// The instant at a count of microseconds since the Unix
    /// epoch, read against an offset in microseconds where one
    /// stands. This constructor reads no clock and refuses
    /// nothing. An instant it builds outside the recorded range
    /// is refused where it is rendered, by format_instant, as
    /// every instant outside the range is. A count past the
    /// storage's own edge builds the nearest day past the
    /// recorded range instead of wrapping into an unrelated
    /// day, so the refusal still fires. A caller that owns a
    /// clock read converts its reading through here (the command
    /// line is the one such caller in this repo), so the library
    /// itself reads no wall clock.
    pub fn from_epoch_micros(epoch_micros: i128, offset_micros: Option<i64>) -> Instant {
        let local = epoch_micros.saturating_add(offset_micros.unwrap_or(0) as i128);
        let days = local.div_euclid(86_400_000_000);
        let rest = local.rem_euclid(86_400_000_000);
        let days = days.clamp(
            days_from_civil(1, 1, 1) as i128 - 1,
            days_from_civil(9999, 12, 31) as i128 + 1,
        );
        Instant {
            days: days as i64,
            secs: (rest / 1_000_000) as u32,
            micros: (rest % 1_000_000) as u32,
            offset_micros,
        }
    }

    /// Microseconds since the epoch at the UTC instant itself.
    pub fn epoch_micros(&self) -> i128 {
        ((self.days as i128) * 86_400 + self.secs as i128) * 1_000_000 + self.micros as i128
            - self.offset_micros.unwrap_or(0) as i128
    }

    pub fn is_aware(&self) -> bool {
        self.offset_micros.is_some()
    }

    /// The instant a span of seconds later, the offset kept.
    pub fn plus_seconds(&self, seconds: i64) -> Instant {
        Instant::from_epoch_micros(
            self.epoch_micros() + seconds as i128 * 1_000_000,
            self.offset_micros,
        )
    }
}

impl PartialEq for Instant {
    fn eq(&self, other: &Self) -> bool {
        self.epoch_micros() == other.epoch_micros()
    }
}

impl Eq for Instant {}

impl PartialOrd for Instant {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Instant {
    fn cmp(&self, other: &Self) -> Ordering {
        self.epoch_micros().cmp(&other.epoch_micros())
    }
}

pub(crate) fn validate_now(now: &Instant) -> Result<()> {
    if now.offset_micros.is_none() {
        return Err(Error::InvalidRequest(
            "the current instant must be a timezone aware datetime".to_string(),
        ));
    }
    Ok(())
}

/// Format an instant in UTC, refusing one outside the recorded
/// range, the years 1 through 9999 the format holds.
pub fn format_instant(moment: &Instant) -> Result<String> {
    let utc = moment.epoch_micros();
    let minimum = (days_from_civil(1, 1, 1) as i128) * 86_400 * 1_000_000;
    let maximum = ((days_from_civil(9999, 12, 31) as i128) * 86_400 + 86_399) * 1_000_000 + 999_999;
    if utc < minimum || utc > maximum {
        return Err(Error::InvalidRequest(
            "the instant stands outside the range the switch can record".to_string(),
        ));
    }
    let days = utc.div_euclid(86_400_000_000);
    let rest = utc.rem_euclid(86_400_000_000);
    let (year, month, day) = civil_from_days(days as i64);
    let secs = rest / 1_000_000;
    let micros = rest % 1_000_000;
    let mut out = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    );
    if micros > 0 {
        out.push_str(&format!(".{micros:06}"));
    }
    out.push_str("+00:00");
    Ok(out)
}

fn digit2(bytes: &[u8], pos: usize) -> Option<(u32, usize)> {
    let first = *bytes.get(pos)?;
    let second = *bytes.get(pos + 1)?;
    if !first.is_ascii_digit() || !second.is_ascii_digit() {
        return None;
    }
    Some(((first - b'0') as u32 * 10 + (second - b'0') as u32, pos + 2))
}

fn parse_fraction(bytes: &[u8], pos: usize, allow_bare: bool) -> Option<(u32, usize)> {
    // The caller has seen the dot or comma. The digits past the
    // sixth are truncated. A separator with no digits after it
    // reads as a zero fraction only where the caller allows the
    // bare form and a zone designator stands right after the
    // separator, the Python parser's own reading, probed at
    // 2033-01-01T00:00:00 before +00:00 and before Z reading as
    // the whole second and the same text at the end of the
    // string refusing (DM-A41). The offset caller never allows
    // it, because nothing follows an offset in this grammar and
    // the parser refuses the bare separator there.
    let mut index = pos + 1;
    let start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == start {
        if allow_bare && matches!(bytes.get(index), Some(b'Z' | b'+' | b'-')) {
            return Some((0, index));
        }
        return None;
    }
    let mut micros: u32 = 0;
    let mut place: u32 = 100_000;
    for digit in &bytes[start..index] {
        if place == 0 {
            break;
        }
        micros += (*digit - b'0') as u32 * place;
        place /= 10;
    }
    Some((micros, index))
}

fn week_date_days(year: i64, week: u32, weekday: u32) -> Option<i64> {
    if week < 1 || week > weeks_in_iso_year(year) || !(1..=7).contains(&weekday) {
        return None;
    }
    let jan4 = days_from_civil(year, 1, 4);
    let week_one_monday = jan4 - (iso_weekday(jan4) as i64 - 1);
    let days = week_one_monday + (week as i64 - 1) * 7 + (weekday as i64 - 1);
    let (result_year, _, _) = civil_from_days(days);
    if !(1..=9999).contains(&result_year) {
        return None;
    }
    Some(days)
}

fn parse_date(bytes: &[u8], start: usize) -> Option<(i64, usize)> {
    if start + 4 > bytes.len() {
        return None;
    }
    let mut year: i64 = 0;
    for ch in bytes.iter().skip(start).take(4) {
        if !ch.is_ascii_digit() {
            return None;
        }
        year = year * 10 + (ch - b'0') as i64;
    }
    if year == 0 {
        return None;
    }
    let mut pos = start + 4;
    if pos < bytes.len() && bytes[pos] == b'-' {
        pos += 1;
        if pos < bytes.len() && bytes[pos] == b'W' {
            pos += 1;
            let (week, next) = digit2(bytes, pos)?;
            pos = next;
            let mut weekday = 1;
            if pos < bytes.len() && bytes[pos] == b'-' {
                pos += 1;
                let ch = *bytes.get(pos)?;
                if !ch.is_ascii_digit() {
                    return None;
                }
                weekday = (ch - b'0') as u32;
                pos += 1;
            }
            return week_date_days(year, week, weekday).map(|days| (days, pos));
        }
        let (month, next) = digit2(bytes, pos)?;
        pos = next;
        if pos < bytes.len() && bytes[pos] == b'-' {
            pos += 1;
            let (day, next) = digit2(bytes, pos)?;
            pos = next;
            if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
                return None;
            }
            return Some((days_from_civil(year, month, day), pos));
        }
        return None;
    }
    if pos < bytes.len() && bytes[pos] == b'W' {
        pos += 1;
        let (week, next) = digit2(bytes, pos)?;
        pos = next;
        let mut weekday = 1;
        if pos < bytes.len() && bytes[pos].is_ascii_digit() {
            weekday = (bytes[pos] - b'0') as u32;
            pos += 1;
        }
        return week_date_days(year, week, weekday).map(|days| (days, pos));
    }
    let (month, next) = digit2(bytes, pos)?;
    pos = next;
    let (day, next) = digit2(bytes, pos)?;
    pos = next;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some((days_from_civil(year, month, day), pos))
}

/// The clock tail the two instant grammars share, the fraction
/// and the colon separated and the bare digit minute and second
/// forms that may follow the hours of a time or of an offset,
/// stated once here (DM-A18). Each caller reads its own grammar
/// around the tail and bounds what the tail returns in its own
/// way. The tail returns the minutes and the seconds and the
/// micros it read and the position it reached. The instant
/// caller allows the bare fraction form of DM-A41 and the
/// offset caller does not, and the flag carries that ruling
/// into the fraction reader.
fn parse_clock_tail(
    bytes: &[u8],
    mut pos: usize,
    allow_bare_fraction: bool,
) -> Option<(u32, u32, u32, usize)> {
    let mut minutes = 0u32;
    let mut seconds = 0u32;
    let mut micros = 0u32;
    if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
        let (parsed, next) = parse_fraction(bytes, pos, allow_bare_fraction)?;
        micros = parsed;
        pos = next;
    } else if pos < bytes.len() && bytes[pos] == b':' {
        pos += 1;
        let (parsed, next) = digit2(bytes, pos)?;
        minutes = parsed;
        pos = next;
        if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
            let (parsed, next) = parse_fraction(bytes, pos, allow_bare_fraction)?;
            micros = parsed;
            pos = next;
        } else if pos < bytes.len() && bytes[pos] == b':' {
            pos += 1;
            let (parsed, next) = digit2(bytes, pos)?;
            seconds = parsed;
            pos = next;
            if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
                let (parsed, next) = parse_fraction(bytes, pos, allow_bare_fraction)?;
                micros = parsed;
                pos = next;
            }
        }
    } else if pos < bytes.len() && bytes[pos].is_ascii_digit() {
        let (parsed, next) = digit2(bytes, pos)?;
        minutes = parsed;
        pos = next;
        if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
            let (parsed, next) = parse_fraction(bytes, pos, allow_bare_fraction)?;
            micros = parsed;
            pos = next;
        } else if pos < bytes.len() && bytes[pos].is_ascii_digit() {
            let (parsed, next) = digit2(bytes, pos)?;
            seconds = parsed;
            pos = next;
            if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
                let (parsed, next) = parse_fraction(bytes, pos, allow_bare_fraction)?;
                micros = parsed;
                pos = next;
            }
        }
    }
    Some((minutes, seconds, micros, pos))
}

fn parse_offset(bytes: &[u8], start: usize) -> Option<(i64, usize)> {
    let sign: i64 = if bytes[start] == b'-' { -1 } else { 1 };
    let mut pos = start + 1;
    let (hours, next) = digit2(bytes, pos)?;
    pos = next;
    let (minutes, seconds, mut micros, pos) = parse_clock_tail(bytes, pos, false)?;
    let whole = hours * 3600 + minutes * 60 + seconds;
    if whole >= 86_400 {
        return None;
    }
    if whole == 0 {
        // The Python parser drops the fraction of an offset whose
        // whole components are all zero, probed at +00:00:00.5 and
        // at -00:00:00.25, both reading as a zero offset.
        micros = 0;
    }
    Some((sign * (whole as i64 * 1_000_000 + micros as i64), pos))
}

/// Parse an instant in the family the Python fromisoformat
/// accepted, probed case by case before the port was written.
/// None stands for every refusal the Python parser raises.
pub fn parse_instant(text: &str) -> Option<Instant> {
    let bytes = text.as_bytes();
    let (days, mut pos) = parse_date(bytes, 0)?;
    let (mut secs, micros) = (0u32, 0u32);
    let mut offset_micros: Option<i64> = None;
    if pos == bytes.len() {
        return Some(Instant {
            days,
            secs,
            micros,
            offset_micros,
        });
    }
    // The separator between the date and the time is any single
    // character, the Python parser's own reading, so the advance
    // is by the character's own length. Every other position in
    // the grammar accepts ASCII only, so a non-ASCII byte anywhere
    // else still refuses.
    pos += text[pos..].chars().next()?.len_utf8();
    let (hours, next) = digit2(bytes, pos)?;
    pos = next;
    let (minutes, seconds, micros, mut pos) = parse_clock_tail(bytes, pos, true)?;
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    secs = hours * 3600 + minutes * 60 + seconds;
    if pos == bytes.len() {
        return Some(Instant {
            days,
            secs,
            micros,
            offset_micros,
        });
    }
    if bytes[pos] == b'Z' {
        pos += 1;
        offset_micros = Some(0);
    } else if bytes[pos] == b'+' || bytes[pos] == b'-' {
        let (parsed, next) = parse_offset(bytes, pos)?;
        offset_micros = Some(parsed);
        pos = next;
    } else {
        return None;
    }
    if pos != bytes.len() {
        return None;
    }
    Some(Instant {
        days,
        secs,
        micros,
        offset_micros,
    })
}

/// Parse a stored instant field under the corruption messages
/// the Python reader raised for it.
pub(crate) fn parse_stored_instant(value: &Value) -> Result<Instant> {
    match value {
        Value::Str(text) => match parse_instant(text) {
            None => Err(corrupt(format!(
                "an instant field holds unparseable text {}",
                py_repr_str(text)
            ))),
            Some(instant) if !instant.is_aware() => Err(corrupt(format!(
                "an instant field holds a naive instant {}",
                py_repr_str(text)
            ))),
            Some(instant) => Ok(instant),
        },
        other => Err(corrupt(format!(
            "an instant field holds {} instead of text",
            py_repr_value(other)
        ))),
    }
}

/// Parse a stored instant field and take the field's text
/// beside it, the pair the stored readers carry together.
/// The text extraction cannot fail, and the reason stands
/// written here at the one seat that performs it:
/// parse_stored_instant succeeds only on a Str value, so a
/// parsed instant is text.
pub(crate) fn parse_stored_instant_text(value: &Value) -> Result<(Instant, String)> {
    let instant = parse_stored_instant(value)?;
    let text = value
        .as_str()
        .expect("a parsed instant is text")
        .to_string();
    Ok((instant, text))
}
