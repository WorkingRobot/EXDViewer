use std::{fmt::Display, ops::Deref};

use either::Either;
use nucleo_matcher::pattern::Pattern;
use regex_lite::Regex;
use wildmatch::WildMatch;

use crate::utils::FuzzyMatcher;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ComplexFilter {
    /// A simple key-value filter
    KeyEquals(FilterKey, FilterValue),
    /// Combine two filters with logical AND
    And(Vec<ComplexFilter>),
    // Combine two filters with logical OR
    Or(Vec<ComplexFilter>),
    /// Negate a filter with logical NOT
    Not(Box<ComplexFilter>),
}

impl ComplexFilter {
    pub fn has_fuzzy(&self) -> bool {
        match self {
            ComplexFilter::KeyEquals(_, v) => matches!(v, FilterValue::Fuzzy(_)),
            ComplexFilter::And(v) | ComplexFilter::Or(v) => v.iter().any(|f| f.has_fuzzy()),
            ComplexFilter::Not(f) => f.has_fuzzy(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FilterKey {
    /// Check the row ID for a match
    RowId,
    /// Check any column for a match
    /// If bool is true, all columns must match (AND), otherwise any column can match (OR)
    Column(Wildcard, bool),
}

/// Prepend a '!' to negate the filter
/// Can be surrounded by spaces
/// Examples:
/// - `=23` (equals 23)
/// - `!=23` (not equals 23)
/// - `^=Hello` (starts with "Hello")
/// - `$=World` (ends with "World")
/// - `*= "lo Wo"` (contains "lo Wo")
/// - `~ "Hlo Wrd"` (fuzzy match "Hlo Wrd")
/// - `?="H*o W?rld"` (wildcard match "H*o W?rld")
/// - `/="^Hello.*World$"` (regex match "^Hello.*World$")
/// - `=10..20` (range between 10 and 20, inclusive)
/// - `!$=Test` (not ends with "Test")
/// - `!/= "^Test.*"` (not regex match "^Test.*")
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FilterValue {
    /// Check if the value matches exactly
    /// Uses '='
    Equals(Either<String, i128>),

    /// Check if the value starts with the substring
    /// Uses '^='
    StartsWith(String),

    /// Check if the value ends with the substring
    /// Uses '$='
    EndsWith(String),

    /// Check if the value contains the substring
    /// Uses '*='
    Contains(String),

    /// Check if the value matches the fuzzy pattern (enables ordering of matches)
    /// Uses '~='
    Fuzzy(FuzzyWrapper),

    /// Check if the value matches the wildcard pattern (with * and ?)
    /// Uses '?='
    Wildcard(Wildcard),

    /// Check if the value matches the regular expression
    /// Uses '~'
    Regex(RegexWrapper),

    /// Check if the value is within a range (inclusive) with optional bounds (only for numeric values)
    /// Uses '|=' with '..' for the range
    Range(FilterRange),
}

#[derive(Debug, Clone)]
pub struct FuzzyWrapper(Pattern, String);

impl PartialEq for FuzzyWrapper {
    fn eq(&self, other: &Self) -> bool {
        self.1 == other.1
    }
}

impl Eq for FuzzyWrapper {}

impl std::hash::Hash for FuzzyWrapper {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.1.hash(state);
    }
}

impl From<String> for FuzzyWrapper {
    fn from(value: String) -> Self {
        Self(FuzzyMatcher::parse_pattern(&value), value)
    }
}

impl Deref for FuzzyWrapper {
    type Target = Pattern;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct Wildcard(WildMatch);

impl PartialEq for Wildcard {
    fn eq(&self, other: &Self) -> bool {
        self.0.pattern_chars() == other.0.pattern_chars()
    }
}

impl Eq for Wildcard {}

impl std::hash::Hash for Wildcard {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.pattern_chars().hash(state);
    }
}

impl From<WildMatch> for Wildcard {
    fn from(value: WildMatch) -> Self {
        Self(value)
    }
}

impl Deref for Wildcard {
    type Target = WildMatch;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Wildcard {
    pub fn is_catch_all(&self) -> bool {
        self.0.pattern_chars() == &['*']
    }
}

#[derive(Debug, Clone)]
pub struct RegexWrapper(Regex);

impl PartialEq for RegexWrapper {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

impl Eq for RegexWrapper {}

impl std::hash::Hash for RegexWrapper {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.as_str().hash(state);
    }
}

impl From<Regex> for RegexWrapper {
    fn from(value: Regex) -> Self {
        Self(value)
    }
}

impl Deref for RegexWrapper {
    type Target = Regex;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FilterRange {
    AtLeast(i128),
    AtMost(i128),
    Between(i128, i128),
}

impl FilterRange {
    pub fn contains(&self, value: i128) -> bool {
        match self {
            FilterRange::AtLeast(start) => value >= *start,
            FilterRange::AtMost(end) => value <= *end,
            FilterRange::Between(start, end) => value >= *start && value <= *end,
        }
    }
}

impl Display for FilterRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterRange::AtLeast(start) => write!(f, ">= {}", start),
            FilterRange::AtMost(end) => write!(f, "<= {}", end),
            FilterRange::Between(start, end) => write!(f, "{}..{}", start, end),
        }
    }
}
