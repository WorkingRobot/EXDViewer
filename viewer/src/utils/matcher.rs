use std::{cell::RefCell, rc::Rc};

use itertools::Itertools;
use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

pub struct FuzzyMatcher(Rc<RefCell<FuzzyMatcherImpl>>);

struct FuzzyMatcherImpl {
    matcher: Matcher,
}

impl FuzzyMatcher {
    pub fn new() -> Self {
        let mut config = nucleo_matcher::Config::DEFAULT;
        config.prefer_prefix = true;
        Self(Rc::new(RefCell::new(FuzzyMatcherImpl {
            matcher: Matcher::new(config),
        })))
    }

    pub fn match_list<'a>(&self, pattern: &str, items: &[&'a str]) -> Vec<&'a str> {
        self.match_list_indirect(pattern, items.iter().copied(), |s| s)
    }

    pub fn match_list_indirect<T>(
        &self,
        pattern: &str,
        items: impl Iterator<Item = T>,
        converter: impl Fn(&T) -> &str,
    ) -> Vec<T> {
        if pattern.is_empty() {
            vec![]
        } else {
            let mut inner = self.0.borrow_mut();

            let pattern = Self::parse_pattern(pattern);

            let mut utf_buf = Vec::new();
            items
                .into_iter()
                .filter_map(|item| {
                    let item_str = converter(&item);
                    let item_len = item_str.len();
                    pattern
                        .indices(
                            Utf32Str::new(item_str, &mut utf_buf),
                            &mut inner.matcher,
                            &mut Vec::new(),
                        )
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

    fn parse_pattern(pattern: &str) -> Pattern {
        Pattern::parse(pattern, CaseMatching::Smart, Normalization::Smart)
    }
}
