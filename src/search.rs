// src/search.rs

use crate::{alias_iter, Args, FileAlias, FileInfo, MatchLine};
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use log::debug;
use regex::Regex;
use std::collections::BTreeMap;
use std::fs;
use std::process::Command;

pub fn run_ripgrep_search(args: &Args) -> Result<(Vec<MatchLine>, BTreeMap<FileAlias, FileInfo>, String)> {
    let pattern = args.pattern.as_ref().context("Pattern required for search")?;
    let mut cmd = Command::new("rg");
    cmd.args(["-n", "--ignore-files", "--column", "--no-heading", pattern]);

    let paths = if let Some(ref wd) = args.working_directory {
        args.paths.iter().map(|p| wd.join(p)).collect()
    } else {
        args.paths.clone()
    };
    cmd.args(&paths);

    if args.ignore_case { cmd.arg("--ignore-case"); }
    if !args.extra_args.is_empty() { cmd.args(&args.extra_args); }

    let output = cmd.output().context("failed to run ripgrep (is rg installed?)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    let matches = parse_rg_output(&stdout, args)?;
    let (files, match_lines) = finalize_search_data(matches, args)?;

    Ok((match_lines, files, format!("Regex: {}", pattern)))
}

fn parse_column_range(col_str: &str) -> Result<Vec<usize>> {
    let mut s = col_str.to_string();
    // Handle the shorthand ".." by providing boundaries
    if s.starts_with("..") { s.insert(0, '1'); }
    if s.ends_with("..") { s.push_str("200"); }

    // range_parser returns a Vec of all numbers included in the range(s)
    range_parser::parse_with::<usize>(&s, ";", "..")
        .context("invalid column range")
}

fn parse_rg_output(stdout: &str, args: &Args) -> Result<Vec<(Utf8PathBuf, usize, String)>> {
    let mut results = Vec::new();
    let valid_columns = args.columns.as_ref().map(|s| parse_column_range(s)).transpose()?;
    let exclude_res: Vec<Regex> = args.exclude.iter()
        .map(|p| regex::RegexBuilder::new(p).case_insensitive(args.ignore_case).build())
        .collect::<Result<Vec<_>, _>>()?;

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() < 4 { continue; }

        let (path_str, line_str, col_str, content) = (parts[0], parts[1], parts[2], parts[3]);
        let col_no = col_str.parse::<usize>()?;

        // Check if the current column is in the allowed set
        if let Some(ref allowed) = valid_columns {
            if !allowed.contains(&col_no) {
                debug!("Excluding {}:{} (col {}) - outside range", path_str, line_str, col_no);
                continue;
            }
        }

        if exclude_res.iter().any(|re| re.is_match(content)) { continue; }
        results.push((Utf8PathBuf::from(path_str), line_str.parse::<usize>()?, content.to_string()));
    }

    results.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    if results.len() > args.max_count { results.truncate(args.max_count); }
    Ok(results)
}

fn finalize_search_data(matches: Vec<(Utf8PathBuf, usize, String)>, args: &Args) -> Result<(BTreeMap<FileAlias, FileInfo>, Vec<MatchLine>)> {
    let mut files = BTreeMap::new();
    let mut path_to_alias = BTreeMap::new();
    let mut aliases = alias_iter();

    for (path, _, _) in &matches {
        if path_to_alias.contains_key(path) { continue; }
        let alias = aliases.next().context("exhausted 3-letter aliases")?;

        let full_path = args.working_directory.as_ref()
            .map(|wd| wd.join(path))
            .unwrap_or_else(|| path.clone());

        let content = fs::read_to_string(&full_path).with_context(|| format!("reading {}", full_path))?;
        let mtime = fs::metadata(&full_path)?.modified()?;

        path_to_alias.insert(path.clone(), alias);
        files.insert(alias, FileInfo {
            path: path.clone(), full_path, alias,
            original_content: content, original_mtime: mtime
        });
    }

    let match_lines = matches.into_iter().map(|(path, lineno, content)| {
        let alias = *path_to_alias.get(&path).expect("path must have alias");
        MatchLine { alias, lineno, original_content: content }
    }).collect();

    Ok((files, match_lines))
}
