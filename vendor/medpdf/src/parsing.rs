//! Page specification parsing (e.g. `"1-3,5,7-"`, `"all"`).

use nom::{
    branch::alt,
    character::complete::{char, digit1, multispace0},
    combinator::{all_consuming, map, map_res, opt},
    multi::separated_list1,
    sequence::{delimited, separated_pair},
    IResult,
};
use std::collections::BTreeSet;

use crate::error::{MedpdfError, Result};

#[derive(Debug, Clone, Copy)]
enum PageItem {
    Single(u32),
    Range(Option<u32>, Option<u32>),
}

fn parse_number(input: &str) -> IResult<&str, u32> {
    map_res(digit1, |s: &str| s.parse::<u32>())(input)
}
fn parse_range(input: &str) -> IResult<&str, PageItem> {
    map(
        separated_pair(
            opt(parse_number),
            delimited(multispace0, char('-'), multispace0),
            opt(parse_number),
        ),
        |(start, end)| PageItem::Range(start, end),
    )(input)
}
fn parse_single(input: &str) -> IResult<&str, PageItem> {
    map(parse_number, PageItem::Single)(input)
}
fn parse_item(input: &str) -> IResult<&str, PageItem> {
    alt((parse_range, parse_single))(input)
}
fn parse_spec_list(input: &str) -> IResult<&str, Vec<PageItem>> {
    separated_list1(delimited(multispace0, char(','), multispace0), parse_item)(input)
}

/// Parses a page specification string into a sorted vector of 1-based page numbers.
pub fn parse_page_spec(spec: &str, max_pages: u32) -> Result<Vec<u32>> {
    let mut pages = BTreeSet::new();
    let trimmed_spec = spec.trim();

    if trimmed_spec.eq_ignore_ascii_case("all") {
        return Ok((1..=max_pages).collect());
    }

    let parse_result = all_consuming(parse_spec_list)(trimmed_spec);

    match parse_result {
        Ok((_, items)) => {
            for item in items {
                match item {
                    PageItem::Single(num) => {
                        if num == 0 {
                            return Err(MedpdfError::new("Page numbers must be 1 or greater."));
                        }
                        // Skip pages beyond the document — acts as a filter
                        if num <= max_pages {
                            pages.insert(num);
                        }
                    }
                    PageItem::Range(start_opt, end_opt) => {
                        if max_pages == 0 && (start_opt.is_none() || end_opt.is_none()) {
                            return Err(MedpdfError::new(
                                "Cannot use open ranges on a document with no pages."
                            ));
                        }
                        let start = start_opt.unwrap_or(1);
                        let end = end_opt.unwrap_or(max_pages);
                        if start == 0 || end == 0 {
                            return Err(MedpdfError::new("Page numbers must be 1 or greater."));
                        }
                        // Only error on inverted range when both bounds are explicit
                        if start_opt.is_some() && end_opt.is_some() && start > end {
                            return Err(MedpdfError::new(format!(
                                "Invalid range: start ({}) is greater than end ({}).",
                                start, end
                            )));
                        }
                        // Clamp to actual page count — out-of-bounds pages are silently skipped
                        let clamped_end = end.min(max_pages);
                        if start <= clamped_end {
                            for i in start..=clamped_end {
                                pages.insert(i);
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            return Err(MedpdfError::new(format!(
                "Failed to parse page specification '{}': {}",
                spec, e
            )))
        }
    }
    Ok(pages.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_page() {
        assert_eq!(parse_page_spec("3", 5).unwrap(), vec![3]);
    }

    #[test]
    fn test_parse_range() {
        assert_eq!(parse_page_spec("2-4", 5).unwrap(), vec![2, 3, 4]);
    }

    #[test]
    fn test_parse_open_start_range() {
        assert_eq!(parse_page_spec("-3", 5).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_open_end_range() {
        assert_eq!(parse_page_spec("3-", 5).unwrap(), vec![3, 4, 5]);
    }

    #[test]
    fn test_parse_all() {
        assert_eq!(parse_page_spec("all", 3).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_all_case_insensitive() {
        assert_eq!(parse_page_spec("ALL", 3).unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_page_spec("All", 3).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_comma_separated() {
        assert_eq!(parse_page_spec("1,3,5", 5).unwrap(), vec![1, 3, 5]);
    }

    #[test]
    fn test_parse_mixed() {
        assert_eq!(parse_page_spec("1-3,5", 5).unwrap(), vec![1, 2, 3, 5]);
    }

    #[test]
    fn test_parse_out_of_range_clamped() {
        assert_eq!(parse_page_spec("1-100", 3).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_zero_page_error() {
        assert!(parse_page_spec("0", 5).is_err());
    }

    #[test]
    fn test_parse_inverted_range_error() {
        assert!(parse_page_spec("5-3", 5).is_err());
    }

    #[test]
    fn test_parse_whitespace() {
        assert_eq!(parse_page_spec(" 1 - 3 , 5 ", 5).unwrap(), vec![1, 2, 3, 5]);
    }

    #[test]
    fn test_parse_dedup() {
        assert_eq!(parse_page_spec("1,1,2", 5).unwrap(), vec![1, 2]);
    }

    #[test]
    fn test_parse_empty_string_error() {
        assert!(parse_page_spec("", 5).is_err());
    }

    #[test]
    fn test_parse_open_range_zero_pages_error() {
        assert!(parse_page_spec("1-", 0).is_err());
    }
}
