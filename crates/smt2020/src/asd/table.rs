//! AutoSched text tables: tab-separated, UTF-16LE with BOM or UTF-8, a cell starting with `~`
//! comments out the rest of its line. Cell access reports `file:line` with every error.

use std::fmt::Display;
use std::path::Path;

use des_core::{DAY, HOUR, MINUTE, SECOND, Time};

use super::Error;
use crate::data::Dist;

pub(super) const COMMENT: &str = "~";

pub(super) fn read_text(dir: &Path, file: &str) -> Result<String, Error> {
    let bytes = std::fs::read(dir.join(file)).map_err(|e| Error::new(format!("{file}: {e}")))?;
    decode(&bytes).ok_or_else(|| Error::new(format!("{file}: not UTF-16LE (with BOM) or UTF-8")))
}

fn decode(bytes: &[u8]) -> Option<String> {
    match bytes {
        [0xFF, 0xFE, utf16 @ ..] => {
            if utf16.len() % 2 != 0 {
                return None;
            }
            let units: Vec<u16> = utf16
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16(&units).ok()
        }
        [0xEF, 0xBB, 0xBF, utf8 @ ..] => String::from_utf8(utf8.to_vec()).ok(),
        _ => String::from_utf8(bytes.to_vec()).ok(),
    }
}

pub(super) struct Table {
    file: String,
    header: Vec<String>,
    lines: Vec<Line>,
}

struct Line {
    number: usize,
    cells: Vec<String>,
}

impl Table {
    pub(super) fn read(dir: &Path, file: &str) -> Result<Self, Error> {
        Self::parse(file, &read_text(dir, file)?)
    }

    fn parse(file: &str, text: &str) -> Result<Self, Error> {
        let mut lines = text.lines().enumerate().filter_map(|(index, line)| {
            let cells: Vec<String> = line
                .split('\t')
                .map(str::trim)
                .take_while(|cell| !cell.starts_with(COMMENT))
                .map(str::to_owned)
                .collect();
            cells.iter().any(|cell| !cell.is_empty()).then(|| Line {
                number: index + 1,
                cells,
            })
        });
        let header = lines
            .next()
            .ok_or_else(|| Error::new(format!("{file}: no header line")))?
            .cells;
        Ok(Self {
            file: file.to_owned(),
            header,
            lines: lines.collect(),
        })
    }

    pub(super) fn opt_col(&self, name: &str) -> Option<usize> {
        self.header.iter().position(|column| column == name)
    }

    pub(super) fn cols<const N: usize>(&self, names: [&str; N]) -> Result<[usize; N], Error> {
        let mut cols = [0; N];
        for (col, name) in cols.iter_mut().zip(names) {
            *col = self
                .opt_col(name)
                .ok_or_else(|| Error::new(format!("{}: no column {name}", self.file)))?;
        }
        Ok(cols)
    }

    pub(super) fn rows(&self) -> impl Iterator<Item = Row<'_>> {
        self.lines.iter().map(move |line| Row { table: self, line })
    }
}

#[derive(Clone, Copy)]
pub(super) struct Row<'a> {
    table: &'a Table,
    line: &'a Line,
}

impl<'a> Row<'a> {
    pub(super) fn error(&self, message: impl Display) -> Error {
        Error::new(format!(
            "{}:{}: {message}",
            self.table.file, self.line.number
        ))
    }

    fn column(&self, col: usize) -> &'a str {
        self.table.header.get(col).map_or("?", String::as_str)
    }

    /// Cell text; `None` if empty.
    pub(super) fn opt(&self, col: usize) -> Option<&'a str> {
        self.line
            .cells
            .get(col)
            .map(String::as_str)
            .filter(|cell| !cell.is_empty())
    }

    pub(super) fn text(&self, col: usize) -> Result<&'a str, Error> {
        self.opt(col)
            .ok_or_else(|| self.error(format_args!("{} is empty", self.column(col))))
    }

    /// Fails if a cell outside `cols` is filled (continuation rows).
    pub(super) fn only(&self, cols: &[usize]) -> Result<(), Error> {
        match (0..self.line.cells.len()).find(|col| !cols.contains(col) && self.opt(*col).is_some())
        {
            Some(col) => Err(self.error(format_args!("unexpected {}", self.column(col)))),
            None => Ok(()),
        }
    }

    fn invalid(&self, col: usize, expected: &str) -> Error {
        self.error(format_args!(
            "{} = {:?} is not {expected}",
            self.column(col),
            self.opt(col).unwrap_or("")
        ))
    }

    /// Non-negative decimal number.
    pub(super) fn number(&self, col: usize) -> Result<f64, Error> {
        self.text(col)?
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| self.invalid(col, "a non-negative number"))
    }

    pub(super) fn count(&self, col: usize) -> Result<u32, Error> {
        self.text(col)?
            .parse()
            .map_err(|_| self.invalid(col, "a non-negative integer"))
    }

    /// Percentage in (0, 100] as a probability.
    pub(super) fn percent(&self, col: usize) -> Result<f64, Error> {
        let percent = self.number(col)?;
        if percent > 0.0 && percent <= 100.0 {
            Ok(percent / 100.0)
        } else {
            Err(self.invalid(col, "a percentage in (0, 100]"))
        }
    }

    /// `yes` or `no`; empty means `no`.
    pub(super) fn flag(&self, col: usize) -> Result<bool, Error> {
        match self.opt(col) {
            None | Some("no") => Ok(false),
            Some("yes") => Ok(true),
            Some(_) => Err(self.invalid(col, "yes or no")),
        }
    }

    /// Number in the time unit named in `unit` (`min`, `hr` or `day`), rounded to whole ms.
    pub(super) fn duration(&self, value: usize, unit: usize) -> Result<Time, Error> {
        let unit_ms = match self.text(unit)? {
            "min" => MINUTE,
            "hr" => HOUR,
            "day" => DAY,
            _ => return Err(self.invalid(unit, "min, hr or day")),
        };
        Ok((self.number(value)? * unit_ms as f64).round() as Time)
    }

    pub(super) fn opt_duration(&self, value: usize, unit: usize) -> Result<Option<Time>, Error> {
        match self.opt(value) {
            Some(_) => self.duration(value, unit).map(Some),
            None if self.opt(unit).is_none() => Ok(None),
            None => Err(self.error(format_args!(
                "{} without {}",
                self.column(unit),
                self.column(value)
            ))),
        }
    }

    /// `MM/DD/YY HH:MM:SS` as ms after `epoch` (a [`civil_ms`] value).
    pub(super) fn date(&self, col: usize, epoch: Time) -> Result<Time, Error> {
        civil_ms(self.text(col)?)
            .map(|ms| ms - epoch)
            .ok_or_else(|| self.invalid(col, "a MM/DD/YY HH:MM:SS date"))
    }

    /// Distribution named in column `kind` (`constant` when the file has no such column) with
    /// parameters `value` (and `value2` for `uniform`) in `unit`.
    pub(super) fn dist(
        &self,
        kind: Option<usize>,
        value: usize,
        value2: Option<usize>,
        unit: usize,
    ) -> Result<Dist, Error> {
        let kind = kind.map_or(Ok("constant"), |col| self.text(col))?;
        let second = value2.and_then(|col| self.opt(col).map(|_| col));
        let dist = match (kind, second) {
            ("constant", None) => Dist::Constant(self.duration(value, unit)?),
            ("exponential", None) => Dist::Exponential {
                mean: self.duration(value, unit)?,
            },
            ("uniform", Some(value2)) => {
                let (mean, half_width) =
                    (self.duration(value, unit)?, self.duration(value2, unit)?);
                if half_width > mean {
                    return Err(self.error("uniform half-width exceeds the mean"));
                }
                Dist::Uniform { mean, half_width }
            }
            _ => return Err(self.error(format_args!("unsupported {kind} distribution parameters"))),
        };
        Ok(dist)
    }
}

/// `MM/DD/YY HH:MM:SS`, year 20YY, as ms since 1970-01-01 00:00:00.
pub(super) fn civil_ms(text: &str) -> Option<Time> {
    let (date, time) = text.split_once(' ')?;
    let [month, day, year] = two_digit_fields(date, '/')?;
    let [hour, minute, second] = two_digit_fields(time, ':')?;
    let year = 2000 + year;
    let valid = (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour < 24
        && minute < 60
        && second < 60;
    valid.then(|| {
        days_from_civil(year, month, day) * DAY + hour * HOUR + minute * MINUTE + second * SECOND
    })
}

fn two_digit_fields(text: &str, separator: char) -> Option<[i64; 3]> {
    let mut fields = text.split(separator).map(|field| {
        (field.len() == 2 && field.bytes().all(|b| b.is_ascii_digit()))
            .then(|| field.parse::<i64>().ok())
            .flatten()
    });
    let parsed = [fields.next()??, fields.next()??, fields.next()??];
    fields.next().is_none().then_some(parsed)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf16le_and_utf8() {
        let text = "A\tB\r\n1\t2\r\n";
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode(&utf16).as_deref(), Some(text));
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'x']).as_deref(), Some("x"));
        assert_eq!(decode(b"x").as_deref(), Some("x"));
        assert_eq!(decode(&[0xFF, 0xFE, 0x41]), None);
    }

    #[test]
    fn parses_cells_comments_and_line_numbers() {
        let table = Table::parse("t.txt", "A\tB\tC\n\n x \t~1\t2\n\t\t\ny\t\tz\n").unwrap();
        let [a, b, c] = table.cols(["A", "B", "C"]).unwrap();
        let rows: Vec<_> = table.rows().collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].opt(a), rows[0].opt(b), rows[0].opt(c)),
            (Some("x"), None, None)
        );
        assert_eq!(
            (rows[1].opt(a), rows[1].opt(b), rows[1].opt(c)),
            (Some("y"), None, Some("z"))
        );
        assert_eq!(
            rows[1].text(b).unwrap_err().to_string(),
            "t.txt:5: B is empty"
        );
        assert!(table.cols(["D"]).is_err());
    }

    #[test]
    fn converts_values() {
        let table = Table::parse(
            "t.txt",
            "V\tW\tU\tK\tF\tD\n0.0426\t1\tmin\tuniform\tyes\t02/29/20 12:30:15\n",
        )
        .unwrap();
        let row = table.rows().next().unwrap();
        let [v, w, u, k, f, d] = table.cols(["V", "W", "U", "K", "F", "D"]).unwrap();
        assert_eq!(row.duration(v, u).unwrap(), 2_556);
        assert_eq!(
            row.dist(Some(k), w, Some(v), u).unwrap(),
            Dist::Uniform {
                mean: MINUTE,
                half_width: 2_556
            }
        );
        assert!(row.dist(None, w, Some(v), u).is_err());
        assert!(row.flag(f).unwrap());
        assert_eq!(row.percent(w).unwrap(), 0.01);
        let epoch = civil_ms("01/01/20 00:00:00").unwrap();
        assert_eq!(
            row.date(d, epoch).unwrap(),
            59 * DAY + 12 * HOUR + 30 * MINUTE + 15 * SECOND
        );
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_ms("01/01/18 00:00:00"), Some(1_514_764_800_000));
        let span = civil_ms("12/31/21 00:00:00").unwrap() - civil_ms("01/01/18 00:00:00").unwrap();
        assert_eq!(span, 1_460 * DAY);
        assert_eq!(civil_ms("02/29/19 00:00:00"), None);
        assert_eq!(civil_ms("13/01/18 00:00:00"), None);
        assert_eq!(civil_ms("1/01/18 00:00:00"), None);
        assert_eq!(civil_ms("01/01/2018 00:00:00"), None);
    }
}
