//! Date parsing and entry filtering.

use anyhow::{Context, Result, bail};
use chrono::{Duration, Local, NaiveDate};
use regex::{Regex, RegexBuilder};

use crate::journal::Entry;

#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub tag: Option<String>,
    pub terms: Vec<String>,
    pub regex: bool,
    pub case_sensitive: bool,
    pub any: bool,
}

impl Filters {
    /// Does the entry satisfy the date and tag constraints?
    pub fn base_matches(&self, entry: &Entry) -> bool {
        if let Some(from) = self.from
            && entry.date.date() < from
        {
            return false;
        }
        if let Some(to) = self.to
            && entry.date.date() > to
        {
            return false;
        }
        if let Some(tag) = &self.tag {
            let wanted = tag.trim();
            let (want_symbol, want_name) = match wanted.chars().next() {
                Some(symbol @ ('#' | '@')) => (Some(symbol), &wanted[symbol.len_utf8()..]),
                _ => (None, wanted),
            };
            let found = entry.tags().iter().any(|candidate| {
                let mut chars = candidate.chars();
                let symbol = chars.next();
                let name: String = chars.collect();
                let symbol_ok = match want_symbol {
                    Some(want) => symbol == Some(want),
                    None => true,
                };
                symbol_ok && name.eq_ignore_ascii_case(want_name)
            });
            if !found {
                return false;
            }
        }
        true
    }
}

/// A compiled query: filters plus one regex per search term.
pub struct Query {
    filters: Filters,
    patterns: Vec<Regex>,
    combined: Option<Regex>,
}

impl Query {
    pub fn new(filters: Filters) -> Result<Self> {
        let build = |pattern: &str| -> Result<Regex> {
            RegexBuilder::new(pattern)
                .case_insensitive(!filters.case_sensitive)
                .build()
                .with_context(|| format!("invalid search pattern `{pattern}`"))
        };

        let mut patterns = Vec::new();
        for term in &filters.terms {
            patterns.push(build(&literal_or_regex(term, filters.regex))?);
        }

        // One alternation, so highlighting is a single safe pass over the text.
        let combined = if filters.terms.is_empty() {
            None
        } else {
            let alternatives = filters
                .terms
                .iter()
                .map(|t| format!("(?:{})", literal_or_regex(t, filters.regex)))
                .collect::<Vec<_>>()
                .join("|");
            Some(build(&alternatives)?)
        };

        Ok(Query {
            filters,
            patterns,
            combined,
        })
    }

    pub fn matches(&self, entry: &Entry) -> bool {
        if !self.filters.base_matches(entry) {
            return false;
        }
        if self.patterns.is_empty() {
            return true;
        }
        if self.filters.any {
            self.patterns.iter().any(|re| re.is_match(&entry.body))
        } else {
            self.patterns.iter().all(|re| re.is_match(&entry.body))
        }
    }

    /// How many times the query terms occur in this entry.
    pub fn occurrences(&self, entry: &Entry) -> usize {
        match &self.combined {
            Some(re) => re.find_iter(&entry.body).count(),
            None => 0,
        }
    }

    /// The regex used to highlight hits in rendered output.
    pub fn highlight(&self) -> Option<&Regex> {
        self.combined.as_ref()
    }

    /// The first line of the entry that contains a hit.
    pub fn matching_line<'a>(&self, entry: &'a Entry) -> &'a str {
        let Some(re) = &self.combined else {
            return entry.first_line_ref();
        };
        entry
            .body
            .lines()
            .find(|line| re.is_match(line))
            .map(|l| l.trim())
            .unwrap_or_else(|| entry.first_line_ref())
    }
}

fn literal_or_regex(term: &str, regex: bool) -> String {
    if regex {
        term.to_string()
    } else {
        regex::escape(term)
    }
}

fn start_of_today() -> NaiveDate {
    Local::now().date_naive()
}

/// Parse a date expression into an inclusive range.
///
/// Accepts `2026`, `2026-03`, `2026-03-14`, `today`, `yesterday`, and
/// relative forms such as `-7d`, `3w`, `2m`, `1y`.
pub fn parse_range(spec: &str) -> Result<(NaiveDate, NaiveDate)> {
    let spec = spec.trim();
    if spec.is_empty() {
        bail!("empty date");
    }
    let lower = spec.to_ascii_lowercase();
    let today = start_of_today();

    match lower.as_str() {
        "today" | "now" => return Ok((today, today)),
        "yesterday" => {
            let d = today - Duration::days(1);
            return Ok((d, d));
        }
        "tomorrow" => {
            let d = today + Duration::days(1);
            return Ok((d, d));
        }
        _ => {}
    }

    if let Some(caps) = relative_regex().captures(&lower) {
        let n: i64 = caps[1].parse().unwrap_or(0);
        let unit = &caps[2];
        let days = match unit {
            "d" => n,
            "w" => n * 7,
            "m" => n * 30,
            "y" => n * 365,
            _ => 0,
        };
        let d = today - Duration::days(days);
        return Ok((d, d));
    }

    let parts: Vec<&str> = spec
        .split(['-', '/', '.'])
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "+")
        .collect();
    let negative_year = spec.trim_start().starts_with('-');

    match parts.len() {
        1 => {
            let year: i32 = parts[0]
                .parse()
                .map_err(|_| anyhow::anyhow!("`{spec}` is not a date"))?;
            let year = if negative_year { -year } else { year };
            Ok((
                NaiveDate::from_ymd_opt(year, 1, 1).context("year out of range")?,
                NaiveDate::from_ymd_opt(year, 12, 31).context("year out of range")?,
            ))
        }
        2 => {
            let year: i32 = parts[0]
                .parse()
                .map_err(|_| anyhow::anyhow!("`{spec}` is not a date"))?;
            let month: u32 = parts[1]
                .parse()
                .map_err(|_| anyhow::anyhow!("`{spec}` is not a date"))?;
            let start = NaiveDate::from_ymd_opt(year, month, 1)
                .ok_or_else(|| anyhow::anyhow!("`{spec}` is not a date"))?;
            Ok((start, end_of_month(year, month)))
        }
        3 => {
            let year: i32 = parts[0]
                .parse()
                .map_err(|_| anyhow::anyhow!("`{spec}` is not a date"))?;
            let month: u32 = parts[1]
                .parse()
                .map_err(|_| anyhow::anyhow!("`{spec}` is not a date"))?;
            // The day may carry a time ("14 13:43"); keep only the day.
            let day_part = parts[2].split_whitespace().next().unwrap_or(parts[2]);
            let day: u32 = day_part
                .parse()
                .map_err(|_| anyhow::anyhow!("`{spec}` is not a date"))?;
            let d = NaiveDate::from_ymd_opt(year, month, day)
                .ok_or_else(|| anyhow::anyhow!("`{spec}` is not a date"))?;
            Ok((d, d))
        }
        _ => bail!("`{spec}` is not a date"),
    }
}

fn end_of_month(year: i32, month: u32) -> NaiveDate {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1).expect("valid month rollover") - Duration::days(1)
}

fn relative_regex() -> &'static Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^-?(\d+)\s*([dwmy])$").expect("valid regex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_absolute_dates() {
        assert_eq!(
            parse_range("2026").unwrap(),
            (
                NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()
            )
        );
        assert_eq!(
            parse_range("2026-02").unwrap(),
            (
                NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()
            )
        );
        assert_eq!(
            parse_range("2024-02").unwrap().1,
            NaiveDate::from_ymd_opt(2024, 2, 29).unwrap()
        );
        let (a, b) = parse_range("2026-03-14").unwrap();
        assert_eq!(a, b);
        // a time suffix is tolerated
        let (a, _) = parse_range("2026-03-14 21:30").unwrap();
        assert_eq!(a, NaiveDate::from_ymd_opt(2026, 3, 14).unwrap());
    }

    #[test]
    fn parses_relative_dates() {
        let today = Local::now().date_naive();
        assert_eq!(parse_range("today").unwrap().0, today);
        assert_eq!(
            parse_range("yesterday").unwrap().0,
            today - Duration::days(1)
        );
        assert_eq!(parse_range("-7d").unwrap().0, today - Duration::days(7));
        assert_eq!(parse_range("1w").unwrap().0, today - Duration::days(7));
    }

    #[test]
    fn rejects_nonsense() {
        assert!(parse_range("banana").is_err());
        assert!(parse_range("").is_err());
    }
}
