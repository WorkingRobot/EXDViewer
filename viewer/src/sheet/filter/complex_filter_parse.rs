use std::str::FromStr;

use either::Either;
use itertools::Itertools;
use pest::{Parser, iterators::Pair};
use pest_derive::Parser;
use regex_lite::Regex;
use wildmatch::WildMatch;

use crate::sheet::filter::complex_filter::{ComplexFilter, FilterKey, FilterRange, FilterValue};

#[derive(Parser)]
#[grammar = "sheet/filter/filter.pest"]
pub struct PestFilter;

impl FromStr for ComplexFilter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let pairs = PestFilter::parse(Rule::filter, s).map_err(|e| e.to_string())?;
        let pair = pairs
            .exactly_one()
            .map_err(|_| "Expected exactly one filter expression".to_string())?;
        parse_filter(pair)
    }
}

fn parse_filter(pair: Pair<'_, Rule>) -> Result<ComplexFilter, String> {
    assert_eq!(pair.as_rule(), Rule::filter);
    let (inner, eoi) = pair
        .into_inner()
        .collect_tuple()
        .ok_or_else(|| "Expected exactly one expression inside filter".to_string())?;
    assert_eq!(eoi.as_rule(), Rule::EOI);
    parse_or_expr(inner)
}

fn parse_or_expr(pair: Pair<'_, Rule>) -> Result<ComplexFilter, String> {
    assert_eq!(pair.as_rule(), Rule::or_expr);
    let mut ret = Vec::new();
    for expr in pair.into_inner() {
        match expr.as_rule() {
            Rule::and_expr => ret.push(parse_and_expr(expr)?),
            Rule::or => {}
            _ => unreachable!("Unexpected rule in or_expr: {:?}", expr.as_rule()),
        }
    }
    if ret.len() == 1 {
        Ok(ret.into_iter().exactly_one().unwrap())
    } else {
        Ok(ComplexFilter::Or(ret.into_iter().collect()))
    }
}

fn parse_and_expr(pair: Pair<'_, Rule>) -> Result<ComplexFilter, String> {
    assert_eq!(pair.as_rule(), Rule::and_expr);
    let mut ret = Vec::new();
    for expr in pair.into_inner() {
        match expr.as_rule() {
            Rule::not_expr => ret.push(parse_not_expr(expr)?),
            Rule::and => {}
            _ => unreachable!("Unexpected rule in and_expr: {:?}", expr.as_rule()),
        }
    }
    if ret.len() == 1 {
        Ok(ret.into_iter().exactly_one().unwrap())
    } else {
        Ok(ComplexFilter::And(ret.into_iter().collect()))
    }
}

fn parse_not_expr(pair: Pair<'_, Rule>) -> Result<ComplexFilter, String> {
    assert_eq!(pair.as_rule(), Rule::not_expr);
    let mut inner = pair.into_inner();
    let first = inner.next().ok_or("Empty not_expr")?;
    match first.as_rule() {
        Rule::not => {
            let next = inner
                .exactly_one()
                .map_err(|_| "Expected exactly one expression after NOT")?;
            let expr = parse_not_expr(next)?;
            Ok(if let ComplexFilter::Not(inner) = expr {
                // Double negation, just return the inner expression
                *inner
            } else {
                ComplexFilter::Not(Box::new(expr))
            })
        }
        Rule::primary => {
            if inner.count() != 0 {
                return Err("Unexpected extra tokens in not_expr".to_string());
            }
            parse_primary(first)
        }
        _ => unreachable!("Unexpected rule in not_expr: {:?}", first.as_rule()),
    }
}

fn parse_primary(pair: Pair<'_, Rule>) -> Result<ComplexFilter, String> {
    assert_eq!(pair.as_rule(), Rule::primary);
    let inner = pair.into_inner().exactly_one().map_err(|_| {
        "Expected exactly one expression inside primary (either paren_expr or simple_filter)"
            .to_string()
    })?;
    match inner.as_rule() {
        Rule::paren_expr => parse_paren_expr(inner),
        Rule::simple_filter => parse_simple_filter(inner),
        _ => unreachable!("Unexpected rule in primary: {:?}", inner.as_rule()),
    }
}

fn parse_paren_expr(pair: Pair<'_, Rule>) -> Result<ComplexFilter, String> {
    assert_eq!(pair.as_rule(), Rule::paren_expr);
    let (lparen, or_expr, rparen) = pair
        .into_inner()
        .collect_tuple()
        .ok_or_else(|| "Expected exactly three tokens in paren_expr".to_string())?;
    assert_eq!(lparen.as_rule(), Rule::LPAREN);
    assert_eq!(rparen.as_rule(), Rule::RPAREN);
    parse_or_expr(or_expr)
}

fn parse_simple_filter(pair: Pair<'_, Rule>) -> Result<ComplexFilter, String> {
    assert_eq!(pair.as_rule(), Rule::simple_filter);
    let mut inner = pair.into_inner();
    let key_pair = inner
        .next()
        .ok_or("Expected a key in simple_filter".to_string())?;
    let next = inner
        .next()
        .ok_or("Expected a not or comparator in simple_filter".to_string())?;
    let (is_negated, comparator) = match next.as_rule() {
        Rule::not => {
            let comparator_pair = inner
                .next()
                .ok_or("Expected a comparator after NOT in simple_filter".to_string())?;
            (true, comparator_pair)
        }
        Rule::comparator => (false, next),
        _ => unreachable!("Unexpected rule in simple_filter: {:?}", next.as_rule()),
    };

    let (value, is_strict_key) = parse_comparator(comparator)?;
    let key = parse_key(key_pair, is_strict_key)?;

    if is_negated {
        Ok(ComplexFilter::Not(Box::new(ComplexFilter::KeyEquals(
            key, value,
        ))))
    } else {
        Ok(ComplexFilter::KeyEquals(key, value))
    }
}

fn parse_key(pair: Pair<'_, Rule>, is_strict: bool) -> Result<FilterKey, String> {
    assert_eq!(pair.as_rule(), Rule::key);
    let inner = pair.into_inner().exactly_one().map_err(|_| {
        "Expected exactly one token inside key (either row_id or column)".to_string()
    })?;
    match inner.as_rule() {
        Rule::row_id => Ok(FilterKey::RowId),
        Rule::column => {
            let col_str = inner.as_str();
            Ok(FilterKey::Column(WildMatch::new(col_str).into(), is_strict))
        }
        _ => unreachable!("Unexpected rule in key: {:?}", inner.as_rule()),
    }
}

fn parse_comparator(pair: Pair<'_, Rule>) -> Result<(FilterValue, bool), String> {
    assert_eq!(pair.as_rule(), Rule::comparator);
    let mut pairs = pair.into_inner();

    let a = pairs
        .next()
        .ok_or_else(|| "Expected at least two tokens in comparator".to_string())?;
    let b = pairs
        .next()
        .ok_or_else(|| "Expected at least two tokens in comparator".to_string())?;
    let c = pairs.next(); // optional

    let (op, is_strict, value) = match (a.as_rule(), b.as_rule(), c.as_ref().map(|p| p.as_rule())) {
        (_, Rule::STRICT_KEY, Some(_)) => (a, true, c.unwrap()),
        (_, _, None) => (a, false, b),
        _ => return Err("Invalid comparator format".to_string()),
    };

    let value = match op.as_rule() {
        Rule::EQUALS => FilterValue::Equals(parse_strnum_value(value)?),
        Rule::STARTS_WITH => FilterValue::StartsWith(parse_string_value(value)?),
        Rule::ENDS_WITH => FilterValue::EndsWith(parse_string_value(value)?),
        Rule::CONTAINS => FilterValue::Contains(parse_string_value(value)?),
        Rule::FUZZY => FilterValue::Fuzzy(parse_string_value(value)?.into()),
        Rule::WILDCARD => FilterValue::Wildcard(WildMatch::new(&parse_string_value(value)?).into()),
        Rule::REGEX => FilterValue::Regex(parse_regex_value(value)?.into()),
        Rule::RANGE => {
            let ret = parse_range_value(value)?;
            match ret {
                FilterRange::Between(a, b) if a > b => {
                    return Err(format!("Invalid range: start {a} is greater than end {b}"));
                }
                FilterRange::Between(a, b) if a == b => FilterValue::Equals(Either::Right(a)),
                _ => FilterValue::Range(ret),
            }
        }
        _ => unreachable!("Unexpected operator in comparator: {:?}", op.as_rule()),
    };
    Ok((value, is_strict))
}

fn parse_strnum_value(pair: Pair<'_, Rule>) -> Result<Either<String, i128>, String> {
    assert_eq!(pair.as_rule(), Rule::strnum_value);
    let inner = pair.into_inner().exactly_one().map_err(|_| {
        "Expected exactly one token inside strnum_value (either number or string_value)".to_string()
    })?;
    match inner.as_rule() {
        Rule::number => Ok(Either::Right(parse_number(inner)?)),
        Rule::string_value => Ok(Either::Left(parse_string_value(inner)?)),
        _ => unreachable!("Unexpected rule in strnum_value: {:?}", inner.as_rule()),
    }
}

fn parse_string_value(pair: Pair<'_, Rule>) -> Result<String, String> {
    assert_eq!(pair.as_rule(), Rule::string_value);
    let inner = pair.into_inner().exactly_one().map_err(|_| {
        "Expected exactly one token inside string_value (either quoted_string or bare_string)"
            .to_string()
    })?;
    match inner.as_rule() {
        Rule::quoted_string => parse_quoted_string(inner),
        Rule::bare_string => Ok(inner.as_str().to_string()),
        _ => unreachable!("Unexpected rule in string_value: {:?}", inner.as_rule()),
    }
}

fn parse_regex_value(pair: Pair<'_, Rule>) -> Result<Regex, String> {
    assert_eq!(pair.as_rule(), Rule::regex_value);
    let inner = pair.into_inner().exactly_one().map_err(|_| {
        "Expected exactly one token inside regex_value (either regex or string_value)".to_string()
    })?;
    match inner.as_rule() {
        Rule::regex => parse_regex(inner),
        Rule::string_value => {
            let s = parse_string_value(inner)?;
            Regex::new(&s).map_err(|e| format!("Failed to compile regex from string: {e}"))
        }
        _ => unreachable!("Unexpected rule in regex_value: {:?}", inner.as_rule()),
    }
}

fn parse_range_value(pair: Pair<'_, Rule>) -> Result<FilterRange, String> {
    assert_eq!(pair.as_rule(), Rule::range_value);
    let inner = pair.into_inner().exactly_one().map_err(|_| {
        "Expected exactly one token inside range_value (either range or number_value)".to_string()
    })?;
    match inner.as_rule() {
        Rule::range => parse_range(inner),
        Rule::number => {
            let num = parse_number(inner)?;
            Ok(FilterRange::Between(num, num))
        }
        _ => unreachable!("Unexpected rule in range_value: {:?}", inner.as_rule()),
    }
}

fn parse_quoted_string(pair: Pair<'_, Rule>) -> Result<String, String> {
    assert_eq!(pair.as_rule(), Rule::quoted_string);
    let (quote, charseq) = pair.into_inner().collect_tuple().ok_or_else(|| {
        "Expected exactly two tokens inside quoted_string (quote and charseq)".to_string()
    })?;
    assert_eq!(quote.as_rule(), Rule::QUOTE);
    assert_eq!(charseq.as_rule(), Rule::quoted_charseq);
    unquote_string(charseq.as_str())
}

fn parse_bare_string(pair: Pair<'_, Rule>) -> Result<&'_ str, String> {
    assert_eq!(pair.as_rule(), Rule::bare_string);
    Ok(pair.as_str())
}

fn parse_regex(pair: Pair<'_, Rule>) -> Result<Regex, String> {
    assert_eq!(pair.as_rule(), Rule::regex);
    let (slash, str_value, flags) = pair.into_inner().collect_tuple().ok_or_else(|| {
        "Expected exactly three tokens inside regex (slash, string_value, flags)".to_string()
    })?;

    assert_eq!(slash.as_rule(), Rule::REGEX_SEPARATOR);
    assert_eq!(str_value.as_rule(), Rule::regex_charseq);
    assert_eq!(flags.as_rule(), Rule::regex_flags);

    let pattern = str_value.as_str();
    let flags_str = flags.as_str();
    let mut regex_builder = regex_lite::RegexBuilder::new(pattern);
    for flag in flags_str.chars() {
        match flag {
            'i' => {
                regex_builder.case_insensitive(true);
            }
            'm' => {
                regex_builder.multi_line(true);
            }
            's' => {
                regex_builder.dot_matches_new_line(true);
            }
            'U' => {
                regex_builder.swap_greed(true);
            }
            'R' => {
                regex_builder.crlf(true);
            }
            'x' => {
                regex_builder.ignore_whitespace(true);
            }
            'u' => {
                // regex_lite treats this as a no-op
            }
            other => {
                return Err(format!("Invalid regex flag: {other}"));
            }
        }
    }
    regex_builder
        .build()
        .map_err(|e| format!("Failed to build regex: {e}"))
}

fn parse_number(pair: Pair<'_, Rule>) -> Result<i128, String> {
    assert_eq!(pair.as_rule(), Rule::number);
    let num_str = pair.as_str();
    i128::from_str_radix(num_str, 10)
        .map_err(|e| format!("Failed to parse number '{num_str}': {e}"))
}

fn parse_range(pair: Pair<'_, Rule>) -> Result<FilterRange, String> {
    assert_eq!(pair.as_rule(), Rule::range);

    let mut pairs = pair.into_inner();
    let a = pairs
        .next()
        .ok_or_else(|| "Expected at least two tokens in range".to_string())?;
    let b = pairs
        .next()
        .ok_or_else(|| "Expected at least two tokens in range".to_string())?;
    let c = pairs.next(); // optional

    match (a.as_rule(), b.as_rule(), c.as_ref().map(|p| p.as_rule())) {
        (Rule::number, Rule::RANGE_SEPARATOR, Some(Rule::number)) => {
            let start = parse_number(a)?;
            let end = parse_number(c.unwrap())?;
            Ok(FilterRange::Between(start, end))
        }
        (Rule::RANGE_SEPARATOR, Rule::number, None) => {
            let end = parse_number(b)?;
            Ok(FilterRange::AtMost(end))
        }
        (Rule::number, Rule::RANGE_SEPARATOR, None) => {
            let start = parse_number(a)?;
            Ok(FilterRange::AtLeast(start))
        }
        _ => Err("Invalid range format".to_string()),
    }
}

fn unquote_string(s: &str) -> Result<String, String> {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // Escape sequence
            if let Some(escaped) = chars.next() {
                match escaped {
                    '"' => result.push('"'),
                    '\'' => result.push('\''),
                    '\\' => result.push('\\'),
                    'n' => result.push('\n'),
                    'r' => result.push('\r'),
                    't' => result.push('\t'),
                    other => {
                        return Err(format!("Invalid escape sequence: \\{other}"));
                    }
                }
            } else {
                return Err("Trailing backslash in string".to_string());
            }
        } else {
            result.push(c);
        }
    }
    Ok(result)
}

mod tests {
    use std::str::FromStr;

    fn test_filter(input: &str) {
        let filter = super::ComplexFilter::from_str(input);
        assert!(
            filter.is_ok(),
            "Filter {input:?} failed:\n{}",
            filter.err().unwrap()
        );
        println!("Filter {input:?} parsed as:\n{:#?}", filter.unwrap());
    }

    #[test]
    fn test_simple() {
        let filter_str = r#"Column1 ^= "Hello""#;
        test_filter(filter_str);
    }

    #[test]
    fn test_complex() {
        let filter_str = r#"(Column1 ^== "Hello" AND Column2 $= "World" && * = a) OR (# = 42)"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_tmp() {
        let filter_str =
            r#"not not not Column1 ^= "Hello" and a = 3 or (b != 4 and c = 4 or d = 3)"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_not() {
        let filter_str = r#"NOT (Column1 ^= "Hello" AND Column2 $= "World")"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_nested() {
        let filter_str = r#"((Column1 ^= "Hello" AND Column2 $= "World") OR (# = 42)) AND NOT (Column3 *= "Test")"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_wildcard() {
        let filter_str = r#"Column1 ?= "H*o W?rld""#;
        test_filter(filter_str);
    }

    #[test]
    fn test_regex() {
        let filter_str = r#"Column1 /= /^Hello.*World$/i"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_range() {
        let filter_str = r#"Column1 |= 10..20"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_negated_range() {
        let filter_str = r#"Column1 !|= 10..20"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_negated_equals() {
        let filter_str = r#"Column1 not = Test"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_negated_starts_with() {
        let filter_str = r#"Column1 not ^= Test"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_negated_ends_with() {
        let filter_str = r#"Column1 not $= Test"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_negated_contains() {
        let filter_str = r#"Column1 not *= Test"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_negated_fuzzy() {
        let filter_str = r#"Column1 not ~= Tst"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_negated_wildcard() {
        let filter_str = r#"Column1 not ?= "T?st*""#;
        test_filter(filter_str);
    }

    #[test]
    fn test_negated_regex() {
        let filter_str = r#"Column1 !/= /^Test.*$/i"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_strict_range() {
        let filter_str = r#"Column1 |== 10..=20"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_any_column() {
        let filter_str = r#"* ^= "Hello""#;
        test_filter(filter_str);
    }

    #[test]
    fn test_row_id() {
        let filter_str = r#"# = 42"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_row_id_not() {
        let filter_str = r#"# != 42"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_combined_not() {
        let filter_str = r#"NOT (# = 42 OR Column1 ^= "Hello")"#;
        test_filter(filter_str);
    }

    #[test]
    fn test_double_negation() {
        let filter_str = r#"NOT Column1 !^= "Hello""#;
        test_filter(filter_str);
    }

    #[test]
    fn test_whitespace() {
        let filter_str = "\tColumn1\t^=  'Hello' ";
        test_filter(filter_str);
    }
}
