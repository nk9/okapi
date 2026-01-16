// Copyright 2025 Nick Kocharhook
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use clap::Parser;
use itertools::iproduct;
use log::{debug, error, warn};
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

mod file_alias;
use file_alias::FileAlias;

/// Edit all regex matches from many files in one buffer.
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// PCRE-compatible regex pattern (passed to ripgrep -P)
    pattern: String,

    /// Files or directories to search (passed to ripgrep)
    #[arg(value_name = "PATHS", num_args = 0..)]
    paths: Vec<Utf8PathBuf>,

    /// Editor command (default: "subl --wait")
    #[arg(short = 'd', long)]
    editor: Option<String>,

    /// Maximum number of total matches to include. Hard max at 18,278 due to 3-letter aliases.
    #[arg(short, long, default_value = "1000")]
    max_count: usize,

    /// Exclude pattern - matches that also match this regex will be filtered out
    #[arg(short, long)]
    exclude: Vec<String>,

    /// Case insensitive search
    #[arg(short, long)]
    ignore_case: bool,

    /// Working directory - prepend this to all paths before passing to ripgrep
    #[arg(short, long)]
    working_directory: Option<Utf8PathBuf>,

    /// Column range filter (e.g., "..35", "3-20", "15.."). Default max col is 200.
    #[arg(short, long)]
    columns: Option<String>,

    /// Arguments passed directly to ripgrep
    #[arg(last = true)]
    extra_args: Vec<String>,
}

#[derive(Debug)]
struct FileInfo {
    path: Utf8PathBuf,
    full_path: Utf8PathBuf,
    alias: FileAlias,
    original_content: String,
    original_mtime: SystemTime,
}

#[derive(Debug)]
struct MatchLine {
    alias: FileAlias,
    lineno: usize,
    original_content: String,
}

fn main() -> Result<()> {
    env_logger::init();

    let args = Args::parse();

    // Parse column range if provided
    let column_range = if let Some(mut col_str) = args.columns {
        if col_str.starts_with("..") {
            col_str.insert(0, '0');
        } else if col_str.ends_with("..") {
            col_str.push_str("200");
        }

        Some(
            range_parser::parse_with(col_str.as_str(), ";", "..")
                .context("invalid column range")?,
        )
    } else {
        None
    };

    // Run ripgrep to get matches
    let mut cmd = Command::new("rg");
    cmd.arg("-n")
        .arg("--ignore-files")
        .arg("--column")
        .arg("--no-heading")
        .arg(&args.pattern);

    // Prepend working directory to paths if provided
    let search_paths: Vec<Utf8PathBuf> = if let Some(ref wd) = args.working_directory {
        args.paths.iter().map(|p| wd.join(p)).collect()
    } else {
        args.paths.clone()
    };

    cmd.args(&search_paths);

    if args.ignore_case {
        cmd.arg("--ignore-case");
    }

    // Pass through the unknown arguments
    if !args.extra_args.is_empty() {
        cmd.args(&args.extra_args);
    }

    let output = cmd
        .output()
        .context("failed to run ripgrep (is rg installed?)")?;

    if !output.status.success() {
        debug!("ripgrep exited with status {:?}", output.status.code());
        debug!("Error: {:?}", &output.stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        eprintln!("No matches found.");
        return Ok(());
    }

    // Compile exclude pattern if provided
    let exclude_regexes: Result<Vec<Regex>> = args
        .exclude
        .iter()
        .map(|pat| {
            if args.ignore_case {
                regex::RegexBuilder::new(pat).case_insensitive(true).build()
            } else {
                Regex::new(pat)
            }
            .context(format!("invalid exclude pattern: {}", pat))
        })
        .collect();
    let exclude_regexes = exclude_regexes?;

    // Parse ripgrep output: "path:line:column:content"
    let mut matches: Vec<(Utf8PathBuf, usize, String)> = Vec::new();
    for line in stdout.lines() {
        if let Some((path, rest)) = line.split_once(':')
            && let Some((lineno, rest2)) = rest.split_once(':')
            && let Some((colno, content)) = rest2.split_once(':')
        {
            let mut path = Utf8PathBuf::from(path);

            // Strip working directory prefix if present
            if let Some(ref wd) = args.working_directory
                && let Ok(stripped) = path.strip_prefix(wd)
            {
                path = stripped.to_path_buf();
            }

            if let Ok(line_no) = lineno.parse::<usize>() {
                // Parse column number and apply column filter if provided
                if let Ok(col_no) = colno.parse::<usize>()
                    && let Some(ref range) = column_range
                    && !range.contains(&col_no)
                {
                    debug!(
                        "Excluding {}:{} (column {}) - outside range",
                        path, line_no, col_no
                    );
                    continue;
                }

                // Apply exclude filters if provided
                if exclude_regexes.iter().any(|re| re.is_match(content)) {
                    debug!("Excluding line {}:{} due to exclude pattern", path, line_no);
                    continue;
                }
                matches.push((path, line_no, content.to_string()));
            }
        }
    }

    if matches.is_empty() {
        println!("No matches found after filtering.");
        return Ok(());
    }

    // Apply max_count limit to total matches
    if matches.len() > args.max_count {
        eprintln!(
            "Truncating {} matches to {} (use --max-count to adjust)",
            matches.len(),
            args.max_count
        );
        matches.truncate(args.max_count);
    } else {
        println!("Showing {} matches", matches.len());
    }

    // Sort matches by filename then line number for stability
    matches.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    // Build file info with aliases
    let mut files: BTreeMap<FileAlias, FileInfo> = BTreeMap::new();
    let mut path_to_alias: BTreeMap<Utf8PathBuf, FileAlias> = BTreeMap::new();
    let mut alias_iter = alias_iter();

    for (path, _, _) in &matches {
        if !path_to_alias.contains_key(path) {
            // Get next alias or warn if we've run out
            let alias = match alias_iter.next() {
                Some(a) => a,
                None => {
                    error!(
                        "Too many files (there are only A..ZZZ). Stopping at file {}",
                        path
                    );
                    break;
                }
            };

            // Build full path for reading file
            let full_path = if let Some(ref wd) = args.working_directory {
                wd.join(path)
            } else {
                path.clone()
            };

            let content = fs::read_to_string(&full_path)
                .with_context(|| format!("reading original file {}", full_path))?;
            let metadata = fs::metadata(&full_path)
                .with_context(|| format!("reading metadata for {}", full_path))?;
            let mtime = metadata
                .modified()
                .with_context(|| format!("getting modification time for {}", full_path))?;

            path_to_alias.insert(path.clone(), alias);
            files.entry(alias).or_insert(FileInfo {
                path: path.clone(),
                full_path,
                alias,
                original_content: content,
                original_mtime: mtime,
            });
        }
    }

    // Build match lines
    let mut match_lines: Vec<MatchLine> = Vec::new();
    for (path, lineno, content) in matches {
        // Only include matches from files we have aliases for
        if let Some(&alias) = path_to_alias.get(&path) {
            match_lines.push(MatchLine {
                alias,
                lineno,
                original_content: content,
            });
        }
    }

    if match_lines.is_empty() {
        println!("No matches to edit.");
        return Ok(());
    }

    // Prepare the virtual editing buffer
    let tmp_dir = tempdir().context("creating temporary directory")?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let tmp: Utf8PathBuf = tmp_dir
        .path()
        .join(format!("edit-{}.okapi.txt", ts))
        .try_into()?;

    write_virtual_buffer(&tmp, &args.pattern, &match_lines, &files)?;

    // Keep original text for change detection
    let original = fs::read_to_string(&tmp)?;

    // Launch editor
    let editor_cmd = args.editor
        .or_else(|| std::env::var("EDITOR").ok())
        .unwrap_or_else(|| "vim".to_string());

    let mut parts = editor_cmd.split_whitespace();
    let cmd = parts.next().context("empty editor command")?;
    let args_vec: Vec<_> = parts.chain(std::iter::once(tmp.as_ref())).collect();

    Command::new(cmd)
        .args(&args_vec)
        .status()
        .context(format!("launching editor: {}", editor_cmd))?;

    // If file content changed, apply edits
    let new_text = fs::read_to_string(&tmp)?;
    if new_text == original {
        println!("No changes saved. Exiting.");
        return Ok(());
    }

    apply_changes(&new_text, &files)?;

    Ok(())
}

pub fn alias_iter() -> impl Iterator<Item = FileAlias> {
    let alphabet = 'A'..='Z';

    let singles = alphabet.clone().map(|c| FileAlias::new(&[c]));

    let doubles =
        iproduct!(alphabet.clone(), alphabet.clone()).map(|(c1, c2)| FileAlias::new(&[c1, c2]));

    let triples = iproduct!(alphabet.clone(), alphabet.clone(), alphabet.clone())
        .map(|(c1, c2, c3)| FileAlias::new(&[c1, c2, c3]));

    singles.chain(doubles).chain(triples)
}

fn write_virtual_buffer(
    tmp: &Utf8Path,
    regex: &str,
    match_lines: &[MatchLine],
    files: &BTreeMap<FileAlias, FileInfo>,
) -> Result<()> {
    let mut file = fs::File::create(tmp)?;

    writeln!(file, "# okapi – bulk regex editing buffer")?;
    writeln!(file, "# Regex: {regex}")?;
    writeln!(file, "# - Save and close to apply changes.")?;
    writeln!(file, "# - Unchanged lines and those starting with '#' are ignored.")?;
    writeln!(
        file,
        "{}\n{}",
        "# - Remove a line from the source file by deleting everything",
        "#   after the shade block (▓)."
    )?;
    writeln!(file, "#")?;
    writeln!(file, "# --- Begin editable lines ---")?;
    writeln!(file)?;

    let max_line_w = match_lines
        .iter()
        .map(|m| (m.lineno as f64).log10().ceil() as usize)
        .max()
        .unwrap_or(1);

    let mut current_file_alias = None;
    let mut use_heavy_pipe = false;

    for m in match_lines {
        // Switch pipe character when file changes
        let next_alias = Some(m.alias.clone());
        if current_file_alias != next_alias {
            current_file_alias = next_alias;
            use_heavy_pipe = !use_heavy_pipe;
        }

        let pipe = if use_heavy_pipe { "▓" } else { "░" };

        writeln!(
            file,
            "{:>3} {lineno:>width$} {pipe} {content}",
            m.alias,
            lineno = m.lineno,
            content = m.original_content,
            width = max_line_w
        )?;
    }

    writeln!(file)?;
    writeln!(file, "# --- File Aliases ---")?;
    for (_, f) in files {
        writeln!(file, "# {:>3} = {}", f.alias, f.full_path)?;
    }
    Ok(())
}

fn apply_changes(new_text: &str, files: &BTreeMap<FileAlias, FileInfo>) -> Result<()> {
    let line_re = Regex::new(r"^\s*([A-Z]+)\s+(\d+)\s+[▓░]\s?(.*)$")?;
    let all_files_count = files.len();
    let mut all_lines_count = 0;

    // Track changes: alias -> lineno -> Option<new_content>
    let mut files_to_update: HashMap<FileAlias, HashMap<usize, Option<String>>> = HashMap::new();

    for line in new_text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        let pipe_count = line.chars().filter(|&c| c == '▓' || c == '░').count();
        if pipe_count > 1 {
            error!("Detected concatenation, skipping all joined lines:");
            error!("  {}", line);
            continue;
        }

        if let Some(cap) = line_re.captures(line) {
            all_lines_count += 1;
            let alias_str = cap.get(1).unwrap().as_str();
            let alias = FileAlias::from_str(alias_str);
            let lineno: usize = cap.get(2).unwrap().as_str().parse()?;
            let new_content = cap.get(3).unwrap().as_str();

            // Directly look up the file by its alias in the `files` BTreeMap.
            if let Some(file) = files.get(&alias) {
                let original_lines: Vec<&str> = file.original_content.lines().collect();

                if let Some(&original_line) = original_lines.get(lineno - 1) {
                    let change = if new_content.trim().is_empty() {
                        debug!("Line deletion detected at {}:{}", file.path, lineno);
                        Some(None) // Represents a deletion
                    } else if original_line != new_content {
                        debug!("Change detected at {}:{}", file.path, lineno);
                        debug!("  Original: {:?}", original_line);
                        debug!("  New:      {:?}", new_content);
                        Some(Some(new_content.to_string())) // Represents a content change
                    } else {
                        debug!("No change at {}:{}", file.path, lineno);
                        debug!("  Both are: {:?}", original_line);
                        None // No change to record
                    };

                    if let Some(content) = change {
                        files_to_update
                            .entry(alias)
                            .or_default()
                            .insert(lineno, content);
                    }
                }
            }
        }
    }

    if files_to_update.is_empty() {
        println!("No actual changes detected.");
        return Ok(());
    }

    // Apply changes to each file
    let mut line_change_count = 0;
    let mut file_change_count = 0;
    for (file_alias, mut file_changes) in files_to_update {
        let Some(file) = files.get(&file_alias) else {
            error!("Couldn't find file with alias '{file_alias}'");
            continue;
        };

        // Check if file was modified since we started
        let current_metadata = fs::metadata(&file.full_path)
            .with_context(|| format!("reading current metadata for {}", file.full_path))?;
        let current_mtime = current_metadata
            .modified()
            .with_context(|| format!("getting current modification time for {}", file.full_path))?;

        if current_mtime != file.original_mtime {
            error!(
                "file {} was modified during editing session, skipping",
                file.path
            );
            continue;
        }

        // Preserve whether original had trailing newline
        let has_trailing_newline = file.original_content.ends_with('\n');

        let mut lines: Vec<String> = file
            .original_content
            .lines()
            .map(|s| s.to_string())
            .collect();

        let mut current_lineno = 0;
        lines.retain_mut(|line| {
            current_lineno += 1;
            if let Some(change) = file_changes.remove(&current_lineno) {
                line_change_count += 1;
                match change {
                    Some(new_content) => {
                        // Modify the line
                        *line = new_content;
                    }
                    // Remove the line by returning false.
                    None => return false,
                }
            }
            // Keep the line, orig or modified.
            true
        });

        // Any remaining changes are for line numbers outside the original file's range.
        for (lineno, _) in file_changes {
            warn!("line {lineno} out of range for {}", file.path);
        }

        // Reconstruct file with proper trailing newline handling
        let mut joined = lines.join("\n");
        if has_trailing_newline {
            joined.push('\n');
        }

        fs::write(&file.full_path, joined)
            .with_context(|| format!("writing changes back to {}", file.full_path))?;
        file_change_count += 1;

        println!("Updated {}", file.path);
    }

    // Write out summary. `all_lines_count` will always be the largest
    let w = (all_lines_count as f64).log10().ceil() as usize;
    println!(
        "\n  Changed: {:>w$} line(s), {:>w$} file(s)",
        line_change_count, file_change_count,
    );
    println!(
        "Unchanged: {:>w$} line(s), {:>w$} file(s)",
        all_lines_count - line_change_count,
        all_files_count - file_change_count,
    );

    Ok(())
}
