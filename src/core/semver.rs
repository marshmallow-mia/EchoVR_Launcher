//! Semantic versions (semver.org 2.0.0) for mods: their order and the ranges catalogues and
//! blocks name. The same rules as the server's echo_mods_catalogue.py, so both sides always
//! agree on which version is newest and which ones a range takes.
//!
//! * Order: MAJOR.MINOR.PATCH, then a version without prerelease above one with; prerelease
//!   identifiers compare numbers below words, and a longer list wins when all before are
//!   equal. Build metadata (`+…`) doesn't count.
//! * Ranges: `*` or `all`, an exact version, comparators (`>=1.2.0 <2.0.0`), `^1.2.0`, `~1.2.0`.
//!   A prerelease is in a range only when the range names a prerelease of the same
//!   MAJOR.MINOR.PATCH (or exactly, or `*`), unless every version is asked for (blocks).

use std::cmp::Ordering;

/// One prerelease identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ident {
    Number(u64),
    Word(String),
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Ident::Number(a), Ident::Number(b)) => a.cmp(b),
            (Ident::Number(_), Ident::Word(_)) => Ordering::Less,
            (Ident::Word(_), Ident::Number(_)) => Ordering::Greater,
            (Ident::Word(a), Ident::Word(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A semantic version, without its build metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pre: Vec<Ident>,
}

impl Version {
    /// Pure: `text` as a semantic version, or `None` (no `v` in front, no leading zeros).
    pub fn parse(text: &str) -> Option<Version> {
        let text = match text.split_once('+') {
            Some((v, build))
                if build
                    .split('.')
                    .all(|i| !i.is_empty() && i.chars().all(ident_char)) =>
            {
                v
            }
            Some(_) => return None,
            None => text,
        };
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = core.split('.').map(number);
        let (major, minor, patch) = (numbers.next()??, numbers.next()??, numbers.next()??);
        if numbers.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(p) => p
                .split('.')
                .map(|i| {
                    if i.is_empty() || !i.chars().all(ident_char) {
                        None
                    } else if i.chars().all(|c| c.is_ascii_digit()) {
                        number(i).map(Ident::Number)
                    } else {
                        Some(Ident::Word(i.to_string()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Version {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// Whether it has a prerelease part (`1.2.0-beta.1`).
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    fn core(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core().cmp(&other.core()).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

/// A number without leading zeros.
fn number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}

/// Pure: whether `a` is a newer version than `b`. False when either isn't a semantic version.
#[cfg(test)]
pub fn newer(a: &str, b: &str) -> bool {
    matches!((Version::parse(a), Version::parse(b)), (Some(a), Some(b)) if a > b)
}

/// Pure: whether `a` and `b` are the same version (build metadata aside).
#[cfg(test)]
pub fn same(a: &str, b: &str) -> bool {
    matches!((Version::parse(a), Version::parse(b)), (Some(a), Some(b)) if a == b)
}

/// Pure: whether the range `range` is well-formed.
#[cfg(test)]
pub fn valid_range(range: &str) -> bool {
    let range = range.trim();
    range == "*"
        || range == "all"
        || (!range.is_empty() && range.split_whitespace().all(|p| comparator(p).is_some()))
}

fn comparator(part: &str) -> Option<(&str, Version)> {
    let op = [">=", "<=", ">", "<", "=", "^", "~"]
        .into_iter()
        .find(|op| part.starts_with(op))
        .unwrap_or("");
    Some((op, Version::parse(&part[op.len()..])?))
}

/// Pure: whether `version` is in `range` (see the module docs). `prereleases`: take every
/// version in the range, prereleases too (blocks). A malformed range or version takes nothing.
pub fn in_range(version: &str, range: &str, prereleases: bool) -> bool {
    let Some(v) = Version::parse(version) else {
        return false;
    };
    let range = range.trim();
    if range == "*" || range == "all" || range.is_empty() {
        return true;
    }
    let mut named_pre = false;
    for part in range.split_whitespace() {
        let Some((op, t)) = comparator(part) else {
            return false;
        };
        if t.is_prerelease() && t.core() == v.core() {
            named_pre = true;
        }
        let ok = match op {
            "" | "=" => {
                named_pre = true;
                v == t
            }
            ">=" => v >= t,
            ">" => v > t,
            "<=" => v <= t,
            "<" => v < t,
            _ => {
                // ^ and ~: from the version up to (not including) the next breaking one.
                let upper = match op {
                    "~" => (t.major, t.minor + 1, 0),
                    _ if t.major > 0 => (t.major + 1, 0, 0),
                    _ if t.minor > 0 => (0, t.minor + 1, 0),
                    _ => (0, 0, t.patch + 1),
                };
                v >= t && v.core() < upper
            }
        };
        if !ok {
            return false;
        }
    }
    !v.is_prerelease() || named_pre || prereleases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_as_the_spec_does() {
        // semver.org 11.4, ascending.
        let order = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
        ];
        let mut shuffled: Vec<Version> = order
            .iter()
            .rev()
            .map(|v| Version::parse(v).unwrap())
            .collect();
        shuffled.sort();
        let sorted: Vec<Version> = order.iter().map(|v| Version::parse(v).unwrap()).collect();
        assert_eq!(shuffled, sorted);
    }

    #[test]
    fn build_metadata_doesnt_count() {
        assert!(same("1.0.0+a", "1.0.0+b"));
        assert!(!same("1.0.0", "1.0.0-rc.1"));
        assert!(newer("1.0.1", "1.0.0+zzz"));
        assert!(!Version::parse("1.0.0+build-5").unwrap().is_prerelease());
    }

    #[test]
    fn refuses_what_isnt_semver() {
        for bad in [
            "1.0",
            "v1.0.0",
            "01.0.0",
            "1.0.0-01",
            "1.0.0-",
            "1.0.0+",
            "1.0.0-a..b",
            "1.2.3.4",
            "",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn ranges() {
        assert!(in_range("1.2.3", ">=1.2.0 <2.0.0", false));
        assert!(!in_range("2.0.0", ">=1.2.0 <2.0.0", false));
        assert!(in_range("1.9.0", "^1.2.0", false));
        assert!(!in_range("2.0.0", "^1.2.0", false));
        assert!(in_range("0.2.5", "^0.2.1", false));
        assert!(!in_range("0.3.0", "^0.2.1", false));
        assert!(in_range("1.2.9", "~1.2.0", false));
        assert!(!in_range("1.3.0", "~1.2.0", false));
        assert!(in_range("1.2.3", "1.2.3", false));
        assert!(in_range("4.1.0+2", ">=4.1.0", false));
        assert!(in_range("1.0.0", "*", false));
        assert!(!in_range("1.0.0", ">=x", false));
        assert!(!in_range("x", "*", false));
    }

    #[test]
    fn prereleases_only_where_named_unless_blocking() {
        assert!(!in_range("1.3.0-beta.1", ">=1.2.0", false));
        assert!(in_range("1.3.0-beta.2", ">=1.3.0-beta.1", false));
        assert!(in_range("1.3.0-beta.1", ">=1.2.0", true));
        assert!(in_range("1.3.0-beta.1", "all", false));
        assert!(in_range("1.3.0-beta.1", "1.3.0-beta.1", false));
    }

    #[test]
    fn valid_ranges() {
        for good in [
            "*",
            "all",
            ">=1.0.0",
            ">=4.1.0 <6.0.0",
            "^1.2.0",
            "~0.1.0",
            "1.2.3-rc.1",
        ] {
            assert!(valid_range(good), "{good}");
        }
        for bad in ["", ">= 1.0.0", "4.1", "latest"] {
            assert!(!valid_range(bad), "{bad}");
        }
    }
}
