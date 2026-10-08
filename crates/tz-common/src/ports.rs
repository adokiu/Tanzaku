use std::{fmt, str::FromStr};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortRanges(Vec<PortRange>);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PortRangeError {
    #[error("port range is empty")]
    Empty,
    #[error("invalid port range: {0}")]
    Invalid(String),
}

impl PortRanges {
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn new(mut ranges: Vec<PortRange>) -> Result<Self, PortRangeError> {
        if ranges.is_empty() {
            return Err(PortRangeError::Empty);
        }
        ranges.sort_unstable_by_key(|range| (range.start, range.end));
        let mut normalized: Vec<PortRange> = Vec::with_capacity(ranges.len());
        for range in ranges {
            if range.start == 0 || range.end == 0 || range.start > range.end {
                return Err(PortRangeError::Invalid(format!(
                    "{}-{}",
                    range.start, range.end
                )));
            }
            if let Some(last) = normalized.last_mut() {
                if range.start <= last.end.saturating_add(1) {
                    last.end = last.end.max(range.end);
                    continue;
                }
            }
            normalized.push(range);
        }
        Ok(Self(normalized))
    }

    pub fn ranges(&self) -> &[PortRange] {
        &self.0
    }

    pub fn contains(&self, port: u16) -> bool {
        self.0
            .binary_search_by(|range| {
                if port < range.start {
                    std::cmp::Ordering::Greater
                } else if port > range.end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    pub fn excludes(&self, exclusions: &Self) -> Self {
        let mut available = Vec::new();
        for range in &self.0 {
            let range_end = u32::from(range.end);
            let mut cursor = u32::from(range.start);
            for exclusion in exclusions
                .0
                .iter()
                .filter(|exclusion| exclusion.end >= range.start && exclusion.start <= range.end)
            {
                let exclusion_start = u32::from(exclusion.start);
                let exclusion_end = u32::from(exclusion.end);
                if exclusion_start > cursor {
                    available.push(PortRange {
                        start: cursor as u16,
                        end: (exclusion_start - 1) as u16,
                    });
                }
                cursor = cursor.max(exclusion_end + 1);
                if cursor > range_end {
                    break;
                }
            }
            if cursor <= range_end {
                available.push(PortRange {
                    start: cursor as u16,
                    end: range.end,
                });
            }
        }
        Self(available)
    }

    pub fn port_count(&self) -> u32 {
        self.0
            .iter()
            .map(|range| u32::from(range.end) - u32::from(range.start) + 1)
            .sum()
    }
}

impl FromStr for PortRanges {
    type Err = PortRangeError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut ranges = Vec::new();
        for part in input.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err(PortRangeError::Invalid(part.into()));
            }
            let (start, end) = match part.split_once('-') {
                Some((start, end)) if !end.contains('-') => (parse_port(start)?, parse_port(end)?),
                Some(_) => return Err(PortRangeError::Invalid(part.into())),
                None => {
                    let port = parse_port(part)?;
                    (port, port)
                }
            };
            ranges.push(PortRange { start, end });
        }
        Self::new(ranges)
    }
}

impl fmt::Display for PortRanges {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, range) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(",")?;
            }
            if range.start == range.end {
                write!(formatter, "{}", range.start)?;
            } else {
                write!(formatter, "{}-{}", range.start, range.end)?;
            }
        }
        Ok(())
    }
}

fn parse_port(input: &str) -> Result<u16, PortRangeError> {
    let port = input
        .parse::<u16>()
        .map_err(|_| PortRangeError::Invalid(input.into()))?;
    if port == 0 {
        return Err(PortRangeError::Invalid(input.into()));
    }
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_ranges() {
        let ranges: PortRanges = "20000-20010,20010-20020,30000".parse().unwrap();
        assert_eq!(ranges.to_string(), "20000-20020,30000");
        assert!(ranges.contains(20015));
        assert!(!ranges.contains(20021));
        assert_eq!(ranges.port_count(), 22);
    }

    #[test]
    fn subtracts_excluded_ports_without_expanding_the_range() {
        let ranges: PortRanges = "1-65535".parse().unwrap();
        let excludes: PortRanges = "1,80,443,65535".parse().unwrap();
        let available = ranges.excludes(&excludes);
        assert_eq!(available.port_count(), 65531);
        assert!(!available.contains(443));
        assert!(available.contains(444));
    }

    #[test]
    fn rejects_invalid_ranges() {
        for input in ["", "0", "5-3", "1-2-3", "65536", "1,,2"] {
            assert!(input.parse::<PortRanges>().is_err(), "{input}");
        }
    }
}
