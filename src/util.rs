use anyhow::{Context, Result};

pub const MAX_COLUMN: usize = 1024;

pub fn parse_column_range(col_str: &str) -> Result<Vec<usize>> {
    let mut s = col_str.to_string();
    if s.starts_with("..") {
        s.insert(0, '1');
    }
    if s.ends_with("..") {
        s.push_str(&MAX_COLUMN.to_string());
    }
    range_parser::parse_with::<usize>(&s, ",", "..").context("invalid column range")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_column_range_expansion() {
        assert_eq!(parse_column_range("1..3").unwrap(), vec![1, 2, 3]);

        let start = parse_column_range("..3").unwrap();
        assert_eq!(start, vec![1, 2, 3]);

        let end = parse_column_range("1020..").unwrap();
        assert_eq!(end, vec![1020, 1021, 1022, 1023, 1024]);

        let multi = parse_column_range("1..2,5..6").unwrap();
        assert_eq!(multi, vec![1, 2, 5, 6]);
    }

    #[test]
    fn test_invalid_range() {
        assert!(parse_column_range("abc").is_err());
    }
}
