//! AutoSched AP reports (`*.rep`) of the reference runs: UTF-16LE tab-separated tables with a
//! header line; `~` lines carry the report times.

use std::fs;
use std::path::Path;

pub struct Report {
    file: String,
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Report {
    pub fn read(dir: &Path, file: &str) -> Result<Self, String> {
        let bytes = fs::read(dir.join(file)).map_err(|error| format!("{file}: {error}"))?;
        let text = match bytes.strip_prefix(&[0xFF, 0xFE]) {
            Some(utf16) => {
                let units: Vec<u16> = utf16
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                String::from_utf16(&units).map_err(|_| format!("{file}: not UTF-16LE"))?
            }
            None => String::from_utf8(bytes).map_err(|_| format!("{file}: not UTF-8"))?,
        };
        let mut lines = text
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('~'))
            .map(|line| {
                line.split('\t')
                    .map(|cell| cell.trim().to_owned())
                    .collect()
            });
        let header = lines
            .next()
            .ok_or_else(|| format!("{file}: no header line"))?;
        Ok(Self {
            file: file.to_owned(),
            header,
            rows: lines.collect(),
        })
    }

    /// The last reporting period: the window ending with the reference run.
    pub fn last_period(&self) -> Result<&str, String> {
        self.rows
            .last()
            .and_then(|row| row.first())
            .map(String::as_str)
            .ok_or_else(|| format!("{}: no rows", self.file))
    }

    /// Per row of `period`: the `key` cell and the `columns` as numbers (durations
    /// `HOURS:MM:SS` in hours). Rows with an empty cell, such as the finished initial WIP, are left
    /// out.
    pub fn table(
        &self,
        period: &str,
        key: &str,
        columns: &[&str],
    ) -> Result<Vec<(String, Vec<f64>)>, String> {
        let column = |name: &str| {
            self.header
                .iter()
                .position(|column| column == name)
                .ok_or_else(|| format!("{}: no column {name}", self.file))
        };
        let key = column(key)?;
        let columns = columns
            .iter()
            .map(|&name| column(name))
            .collect::<Result<Vec<_>, _>>()?;
        let mut table = Vec::new();
        for row in self.rows.iter().filter(|row| row[0] == period) {
            let values: Option<Vec<f64>> = columns
                .iter()
                .map(|&column| row.get(column).and_then(|cell| number(cell)))
                .collect();
            if let Some(values) = values {
                table.push((row[key].clone(), values));
            }
        }
        Ok(table)
    }
}

/// A number, or a duration `HOURS:MM:SS` in hours; `None` if empty or neither.
fn number(cell: &str) -> Option<f64> {
    match cell.split(':').collect::<Vec<_>>()[..] {
        [value] => value.parse().ok(),
        [hours, minutes, seconds] => {
            let field = |text: &str| text.parse::<f64>().ok();
            Some(field(hours)? + field(minutes)? / 60.0 + field(seconds)? / 3600.0)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_are_numbers_or_durations() {
        assert_eq!(number("89.29"), Some(89.29));
        assert_eq!(number("1235:09:00"), Some(1235.15));
        assert_eq!(number(""), None);
        assert_eq!(number("O_Lot_3"), None);
    }
}
