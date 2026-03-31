use crate::{util::parse_column_range, Args};
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use regex::Regex;
use std::env;

pub struct Config {
    pub working_directory: Utf8PathBuf,
    pub max_count: usize,
    pub columns: Option<Vec<usize>>,
    pub exclude: Vec<Regex>,
    pub ignore_case: bool,
    pub editor: String,
    pub pattern: Option<String>,
    pub paths: Vec<Utf8PathBuf>,
    pub extra_args: Vec<String>,
    pub file: Option<Utf8PathBuf>,
}

impl TryFrom<Args> for Config {
    type Error = anyhow::Error;

    fn try_from(args: Args) -> Result<Self> {
        Ok(Config {
            working_directory: resolve_working_directory(args.working_directory)?,
            max_count: args.max_count,
            columns: args
                .columns
                .as_deref()
                .map(parse_column_range)
                .transpose()?,
            exclude: compile_exclude_patterns(&args.exclude, args.ignore_case)?,
            ignore_case: args.ignore_case,
            editor: resolve_editor(args.editor),
            pattern: args.pattern,
            paths: args.paths,
            extra_args: args.extra_args,
            file: args.file,
        })
    }
}

fn resolve_working_directory(wd: Option<Utf8PathBuf>) -> Result<Utf8PathBuf> {
    let cwd = Utf8PathBuf::try_from(env::current_dir().context("getting current directory")?)
        .context("current directory is not valid UTF-8")?;
    match wd {
        None => Ok(cwd),
        Some(p) if p.is_absolute() => Ok(p),
        Some(p) => Ok(cwd.join(p)),
    }
}

fn compile_exclude_patterns(patterns: &[String], ignore_case: bool) -> Result<Vec<Regex>> {
    patterns
        .iter()
        .map(|p| {
            regex::RegexBuilder::new(p)
                .case_insensitive(ignore_case)
                .build()
                .with_context(|| format!("invalid exclude pattern: {}", p))
        })
        .collect()
}

fn resolve_editor(editor_arg: Option<String>) -> String {
    editor_arg
        .or_else(|| env::var("EDITOR").ok())
        .unwrap_or_else(|| "vim".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse_config(args: &[&str]) -> Result<Config> {
        Config::try_from(Args::parse_from(args))
    }

    #[test]
    fn test_working_directory_defaults_to_cwd() {
        let config = parse_config(&["okapi", "pattern"]).unwrap();
        assert!(config.working_directory.is_absolute());
    }

    #[test]
    fn test_columns_parsed() {
        let config = parse_config(&["okapi", "pattern", "-c", "1..3"]).unwrap();
        assert_eq!(config.columns, Some(vec![1, 2, 3]));
    }

    #[test]
    fn test_columns_none_by_default() {
        let config = parse_config(&["okapi", "pattern"]).unwrap();
        assert!(config.columns.is_none());
    }

    #[test]
    fn test_invalid_columns_errors_at_construction() {
        assert!(parse_config(&["okapi", "pattern", "-c", "abc"]).is_err());
    }

    #[test]
    fn test_editor_fallback_to_vim() {
        // Unset EDITOR to ensure we hit the vim fallback
        unsafe { env::remove_var("EDITOR") };
        let config = parse_config(&["okapi", "pattern"]).unwrap();
        assert_eq!(config.editor, "vim");
    }

    #[test]
    fn test_exclude_compiled() {
        let config = parse_config(&["okapi", "pattern", "-e", "foo"]).unwrap();
        assert_eq!(config.exclude.len(), 1);
        assert!(config.exclude[0].is_match("foobar"));
    }

    #[test]
    fn test_invalid_exclude_errors_at_construction() {
        assert!(parse_config(&["okapi", "pattern", "-e", "["]).is_err());
    }
}
