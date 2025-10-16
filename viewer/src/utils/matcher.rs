use std::{cell::RefCell, num::NonZeroU32, rc::Rc};

use itertools::Itertools;
use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

pub struct FuzzyMatcher(Rc<RefCell<FuzzyMatcherImpl>>);

struct FuzzyMatcherImpl {
    matcher: Matcher,
    utf_buf: Vec<char>,
}

impl FuzzyMatcher {
    pub fn new() -> Self {
        let mut config = nucleo_matcher::Config::DEFAULT;
        config.prefer_prefix = true;
        Self(Rc::new(RefCell::new(FuzzyMatcherImpl {
            matcher: Matcher::new(config),
            utf_buf: Vec::new(),
        })))
    }

    pub fn match_list<'a>(&self, pattern: Option<&str>, items: &[&'a str]) -> Vec<&'a str> {
        self.match_list_indirect(pattern, items.iter().copied(), |s| s)
    }

    pub fn match_list_indirect<T>(
        &self,
        pattern: Option<&str>,
        items: impl Iterator<Item = T>,
        converter: impl Fn(&T) -> &str,
    ) -> Vec<T> {
        let Some(pattern) = pattern else {
            return items.collect();
        };
        if pattern.is_empty() {
            vec![]
        } else {
            let FuzzyMatcherImpl { matcher, utf_buf } = &mut *self.0.borrow_mut();

            let pattern = Self::parse_pattern(pattern);

            items
                .into_iter()
                .filter_map(|item| {
                    let item_str = converter(&item);
                    let item_len = item_str.len();
                    pattern
                        .indices(Utf32Str::new(item_str, utf_buf), matcher, &mut Vec::new())
                        .map(|score| (item, score, item_len))
                })
                .sorted_by(|(_, a_score, a_len), (_, b_score, b_len)| {
                    a_score
                        .cmp(b_score)
                        .reverse()
                        .then_with(|| a_len.cmp(b_len))
                })
                .map(|(s, _, _)| s)
                .collect_vec()
        }
    }

    pub fn score_one(&self, pattern: &Pattern, haystack: &str) -> Option<NonZeroU32> {
        let FuzzyMatcherImpl { matcher, utf_buf } = &mut *self.0.borrow_mut();

        pattern
            .score(Utf32Str::new(haystack, utf_buf), matcher)
            .and_then(NonZeroU32::new)
    }

    pub fn parse_pattern(pattern: &str) -> Pattern {
        Pattern::parse(pattern, CaseMatching::Smart, Normalization::Smart)
    }
}
