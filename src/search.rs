use crate::{alias_iter, Config, FileAlias, FileInfo, MatchLine};
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use log::debug;
use std::collections::BTreeMap;
use std::fs;
use std::process::{exit, Command};

pub fn run_ripgrep_search(
    config: &Config,
) -> Result<(Vec<MatchLine>, BTreeMap<FileAlias, FileInfo>, String)> {
    let pattern = config
        .pattern
        .as_ref()
        .context("Pattern required for search")?;
    let mut cmd = Command::new("rg");
    cmd.args(["-n", "--ignore-files", "--column", "--no-heading", pattern]);

    let paths: Vec<Utf8PathBuf> = config
        .paths
        .iter()
        .map(|p| {
            let expanded = shellexpand::tilde(p.as_str());
            let expanded_path = Utf8PathBuf::from(expanded.into_owned());

            if expanded_path.is_absolute() {
                expanded_path
            } else {
                config.working_directory.join(expanded_path)
            }
        })
        .collect();

    cmd.args(&paths);

    if config.ignore_case {
        cmd.arg("--ignore-case");
    }
    if !config.extra_args.is_empty() {
        cmd.args(&config.extra_args);
    }

    debug!("Running `{:?}`", &cmd);

    // 2. Execute command
    let output = cmd
        .output()
        .context("failed to run ripgrep (is rg installed?)")?;

    // 3. Handle ripgrep errors
    if !output.status.success() {
        // Ripgrep exit code 1 means "no matches found".
        // Any other non-zero code is a real error (invalid regex, etc.)
        if output.status.code() != Some(1) {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("ripgrep error:\n{}", stderr);

            // Exit the whole program with ripgrep's error code
            exit(output.status.code().unwrap_or(1));
        }
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let matches = parse_rg_output(&stdout, config)?;
    let (files, match_lines) = finalize_search_data(matches, config)?;

    Ok((match_lines, files, format!("Regex: {}", pattern)))
}

fn parse_rg_output(stdout: &str, config: &Config) -> Result<Vec<(Utf8PathBuf, usize, String)>> {
    let mut results = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() < 4 {
            continue;
        }

        let (path_str, line_str, col_str, content) = (parts[0], parts[1], parts[2], parts[3]);
        let col_no = col_str.parse::<usize>()?;

        // Check if the current column is in the allowed set
        if let Some(ref allowed) = config.columns {
            if !allowed.contains(&col_no) {
                debug!(
                    "Excluding {}:{} (col {}) - outside range",
                    path_str, line_str, col_no
                );
                continue;
            }
        }

        if config.exclude.iter().any(|re| re.is_match(content)) {
            continue;
        }
        results.push((
            Utf8PathBuf::from(path_str),
            line_str.parse::<usize>()?,
            content.to_string(),
        ));
    }

    results.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    if results.len() > config.max_count {
        results.truncate(config.max_count);
    }
    Ok(results)
}

fn finalize_search_data(
    matches: Vec<(Utf8PathBuf, usize, String)>,
    config: &Config,
) -> Result<(BTreeMap<FileAlias, FileInfo>, Vec<MatchLine>)> {
    let mut files = BTreeMap::new();
    let mut path_to_alias = BTreeMap::new();
    let mut aliases = alias_iter();

    for (path, _, _) in &matches {
        if path_to_alias.contains_key(path) {
            continue;
        }
        let alias = aliases.next().context("exhausted 3-letter aliases")?;

        let full_path = config.working_directory.join(path);

        let content =
            fs::read_to_string(&full_path).with_context(|| format!("reading {}", full_path))?;
        let mtime = fs::metadata(&full_path)?.modified()?;

        path_to_alias.insert(path.clone(), alias);
        files.insert(
            alias,
            FileInfo {
                path: path.clone(),
                full_path,
                alias,
                original_content: content,
                original_mtime: mtime,
            },
        );
    }

    let match_lines = matches
        .into_iter()
        .map(|(path, lineno, content)| {
            let alias = *path_to_alias.get(&path).expect("path must have alias");
            MatchLine {
                alias,
                lineno,
                original_content: content,
            }
        })
        .collect();

    Ok((files, match_lines))
}
