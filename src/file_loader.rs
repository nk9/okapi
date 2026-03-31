use crate::{alias_iter, config::Config, FileAlias, FileInfo, MatchLine};
use anyhow::{bail, Context, Result};
use camino::Utf8PathBuf;
use rayon::prelude::*;
use regex::Regex;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Read};

pub fn load_from_list(
    list_path: &Utf8PathBuf,
    config: &Config,
) -> Result<(Vec<MatchLine>, BTreeMap<FileAlias, FileInfo>, String)> {
    let content = fs::read_to_string(list_path).context("reading list file")?;
    let label = format!("File: {}", list_path);
    let (matches, files) = parse_and_load(&content, config)?;
    Ok((matches, files, label))
}

pub fn load_from_stdin(
    config: &Config,
) -> Result<(Vec<MatchLine>, BTreeMap<FileAlias, FileInfo>, String)> {
    let mut buffer = String::new();
    io::stdin()
        .read_to_string(&mut buffer)
        .context("reading from stdin")?;
    let (matches, files) = parse_and_load(&buffer, config)?;
    Ok((matches, files, "STDIN".to_string()))
}

#[derive(Debug, PartialEq)]
enum InputFormat {
    Simple,  // path:lineno
    Vimgrep, // path:lineno:colno:text
}

struct ParsedEntry {
    path: Utf8PathBuf,
    lineno: usize,
    colno: Option<usize>,
}

fn detect_line_format(line: &str) -> InputFormat {
    let parts: Vec<&str> = line.splitn(4, ':').collect();
    if parts.len() >= 3 && parts[1].parse::<usize>().is_ok() && parts[2].parse::<usize>().is_ok() {
        InputFormat::Vimgrep
    } else {
        InputFormat::Simple
    }
}

fn parse_line_simple(line: &str, idx: usize) -> Result<ParsedEntry> {
    let (path_str, line_str) = line
        .rsplit_once(':')
        .with_context(|| format!("missing colon separator on line {}", idx + 1))?;
    let lineno = line_str
        .parse::<usize>()
        .with_context(|| format!("invalid line number on line {}", idx + 1))?;
    Ok(ParsedEntry {
        path: Utf8PathBuf::from(path_str),
        lineno,
        colno: None,
    })
}

fn parse_line_vimgrep(line: &str, idx: usize) -> Result<ParsedEntry> {
    let parts: Vec<&str> = line.splitn(4, ':').collect();
    if parts.len() < 3 {
        bail!(
            "vimgrep format requires at least path:lineno:colno on line {}",
            idx + 1
        );
    }
    let lineno = parts[1]
        .parse::<usize>()
        .with_context(|| format!("invalid line number on line {}", idx + 1))?;
    let colno = parts[2]
        .parse::<usize>()
        .with_context(|| format!("invalid column number on line {}", idx + 1))?;
    Ok(ParsedEntry {
        path: Utf8PathBuf::from(parts[0]),
        lineno,
        colno: Some(colno),
    })
}

fn parse_entries(content: &str) -> Result<(Vec<ParsedEntry>, InputFormat)> {
    let mut entries = Vec::new();
    let mut detected_format: Option<InputFormat> = None;

    for (idx, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line_format = detect_line_format(line);
        match &detected_format {
            None => {
                detected_format = Some(line_format);
            }
            Some(f) if *f != line_format => {
                bail!(
                    "mixed input formats on line {}; all lines must be \
                     path:lineno or path:lineno:colno:text",
                    idx + 1
                );
            }
            _ => {}
        }

        let entry = match detected_format.as_ref().unwrap() {
            InputFormat::Vimgrep => parse_line_vimgrep(line, idx)?,
            InputFormat::Simple => parse_line_simple(line, idx)?,
        };
        entries.push(entry);
    }

    Ok((entries, detected_format.unwrap_or(InputFormat::Simple)))
}

fn apply_column_filter(entries: Vec<ParsedEntry>, columns: &[usize]) -> Vec<ParsedEntry> {
    entries
        .into_iter()
        .filter(|e| match e.colno {
            Some(col) => columns.contains(&col),
            None => true,
        })
        .collect()
}

fn dedup_by_path_lineno(entries: Vec<ParsedEntry>) -> Vec<ParsedEntry> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .filter(|e| seen.insert((e.path.clone(), e.lineno)))
        .collect()
}

fn parse_and_load(
    content: &str,
    config: &Config,
) -> Result<(Vec<MatchLine>, BTreeMap<FileAlias, FileInfo>)> {
    let (entries, format) = parse_entries(content)?;

    if config.columns.is_some() && format == InputFormat::Simple {
        bail!(
            "-c/--columns requires vimgrep input format (path:lineno:colno:text), \
             but simple format (path:lineno) was detected"
        );
    }

    let entries = match &config.columns {
        Some(cols) => apply_column_filter(entries, cols),
        None => entries,
    };
    let entries = dedup_by_path_lineno(entries);

    let requests: Vec<(Utf8PathBuf, usize)> = entries
        .into_iter()
        .map(|e| {
            let full_path = if e.path.is_absolute() {
                e.path
            } else {
                config.working_directory.join(&e.path)
            };
            (full_path, e.lineno)
        })
        .collect();

    let unique_paths: Vec<Utf8PathBuf> = requests
        .iter()
        .map(|(p, _)| p.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let file_infos = load_files_parallel(unique_paths)?;
    let (files, path_to_alias) = assign_aliases(file_infos);
    let match_lines = build_match_lines(requests, &files, &path_to_alias, &config.exclude);

    Ok((match_lines, files))
}

fn load_files_parallel(paths: Vec<Utf8PathBuf>) -> Result<Vec<FileInfo>> {
    paths
        .into_par_iter()
        .map(|full_path| {
            let content = fs::read_to_string(&full_path)
                .with_context(|| format!("failed to read file: {}", full_path))?;
            let metadata = fs::metadata(&full_path)?;
            Ok(FileInfo {
                // We store the full absolute path in both fields to satisfy
                // the requirement that the alias section shows absolute paths.
                path: full_path.clone(),
                full_path,
                alias: FileAlias::new(&['A']),
                original_content: content,
                original_mtime: metadata.modified()?,
            })
        })
        .collect()
}

fn assign_aliases(
    mut infos: Vec<FileInfo>,
) -> (
    BTreeMap<FileAlias, FileInfo>,
    BTreeMap<Utf8PathBuf, FileAlias>,
) {
    infos.sort_by(|a, b| a.path.cmp(&b.path));
    let mut files = BTreeMap::new();
    let mut path_map = BTreeMap::new();
    let mut aliases = alias_iter();

    for mut info in infos {
        if let Some(alias) = aliases.next() {
            path_map.insert(info.full_path.clone(), alias);
            info.alias = alias;
            files.insert(alias, info);
        }
    }
    (files, path_map)
}

fn build_match_lines(
    reqs: Vec<(Utf8PathBuf, usize)>,
    files: &BTreeMap<FileAlias, FileInfo>,
    path_map: &BTreeMap<Utf8PathBuf, FileAlias>,
    exclude_res: &[Regex],
) -> Vec<MatchLine> {
    reqs.into_iter()
        .filter_map(|(path, lineno)| {
            let alias = path_map.get(&path)?;
            let file = files.get(alias)?;
            let line_content = file.original_content.lines().nth(lineno - 1)?;
            if exclude_res.iter().any(|re| re.is_match(line_content)) {
                return None;
            }
            Some(MatchLine {
                alias: *alias,
                lineno,
                original_content: line_content.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Args;
    use camino_tempfile::tempdir;
    use clap::Parser;

    fn make_config(extra: &[&str]) -> Config {
        let mut base = vec!["okapi", "dummy_pattern"];
        base.extend_from_slice(extra);
        Config::try_from(Args::parse_from(&base)).unwrap()
    }

    fn cols(s: &str) -> Vec<usize> {
        crate::util::parse_column_range(s).unwrap()
    }

    #[test]
    fn test_detect_format_simple() {
        assert_eq!(detect_line_format("src/foo.rs:42"), InputFormat::Simple);
    }

    #[test]
    fn test_detect_format_vimgrep() {
        assert_eq!(
            detect_line_format("src/foo.rs:42:7:let x = 1;"),
            InputFormat::Vimgrep
        );
    }

    #[test]
    fn test_parse_entries_simple() {
        let content = "foo.rs:1\nbar.rs:2\n";
        let (entries, fmt) = parse_entries(content).unwrap();
        assert_eq!(fmt, InputFormat::Simple);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].colno.is_none());
    }

    #[test]
    fn test_parse_entries_vimgrep() {
        let content = "foo.rs:1:5:hello\nfoo.rs:1:10:world\nbar.rs:3:1:other\n";
        let (entries, fmt) = parse_entries(content).unwrap();
        assert_eq!(fmt, InputFormat::Vimgrep);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].colno, Some(5));
    }

    #[test]
    fn test_parse_entries_mixed_format_errors() {
        let content = "foo.rs:1\nbar.rs:2:5:text\n";
        assert!(parse_entries(content).is_err());
    }

    #[test]
    fn test_dedup_keeps_first_colno() {
        let entries = vec![
            ParsedEntry {
                path: Utf8PathBuf::from("a.rs"),
                lineno: 1,
                colno: Some(5),
            },
            ParsedEntry {
                path: Utf8PathBuf::from("a.rs"),
                lineno: 1,
                colno: Some(10),
            },
            ParsedEntry {
                path: Utf8PathBuf::from("a.rs"),
                lineno: 2,
                colno: Some(1),
            },
        ];
        let deduped = dedup_by_path_lineno(entries);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].colno, Some(5));
    }

    #[test]
    fn test_column_filter_keeps_matching() {
        let entries = vec![
            ParsedEntry {
                path: Utf8PathBuf::from("a.rs"),
                lineno: 1,
                colno: Some(5),
            },
            ParsedEntry {
                path: Utf8PathBuf::from("a.rs"),
                lineno: 2,
                colno: Some(15),
            },
        ];
        let filtered = apply_column_filter(entries, &cols("5..10"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].lineno, 1);
    }

    #[test]
    fn test_column_filter_with_simple_format_errors() {
        let config = make_config(&["-c", "1..5"]);
        let content = "foo.rs:1\nbar.rs:2\n";
        assert!(parse_and_load(content, &config).is_err());
    }

    #[test]
    fn test_filter_then_dedup() {
        // col 5 filtered out, col 10 passes -> line 1 survives with col 10
        let entries = vec![
            ParsedEntry {
                path: Utf8PathBuf::from("a.rs"),
                lineno: 1,
                colno: Some(5),
            },
            ParsedEntry {
                path: Utf8PathBuf::from("a.rs"),
                lineno: 1,
                colno: Some(10),
            },
        ];
        let filtered = apply_column_filter(entries, &cols("8..12"));
        let deduped = dedup_by_path_lineno(filtered);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].colno, Some(10));
    }

    #[test]
    fn test_relative_path_resolution() {
        let dir = tempdir().unwrap();
        let wd = Utf8PathBuf::try_from(dir.path().to_path_buf()).unwrap();
        let target = wd.join("target.txt");
        fs::write(&target, "content").unwrap();
        let list_path = wd.join("list.txt");
        fs::write(&list_path, "target.txt:1").unwrap();

        let args = Args::parse_from(&["okapi", "-w", wd.as_str(), "--file", list_path.as_str()]);
        let config = Config::try_from(args).unwrap();
        let (matches, files, _) = load_from_list(&list_path, &config).unwrap();

        let alias = matches[0].alias;
        let info = files.get(&alias).unwrap();
        assert!(info.full_path.is_absolute());
        assert!(info.full_path.ends_with("target.txt"));
    }
}
