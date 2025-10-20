use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use clap::Parser;
use itertools::iproduct;
use itertools::Itertools;
use log::debug;
use regex::Regex;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Edit all regex matches from many files in one buffer.
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// PCRE-compatible regex pattern (passed to ugrep -P)
    pattern: String,

    /// Files or directories to search (passed to ugrep)
    #[arg(value_name = "PATHS", num_args = 0..)]
    paths: Vec<Utf8PathBuf>,

    /// Editor command (default: "subl --wait")
    #[arg(short = 'd', long, default_value = "subl --wait")]
    editor: String,

    /// Maximum number of total matches to include. Hard max at 18,278
    #[arg(short, long, default_value = "150")]
    max_count: usize,

    /// Exclude pattern - matches that also match this regex will be filtered out
    #[arg(short, long)]
    exclude: Option<String>,

    /// Case insensitive search
    #[arg(short, long)]
    ignore_case: bool,

    /// Working directory - prepend this to all paths before passing to ugrep
    #[arg(short, long)]
    working_directory: Option<Utf8PathBuf>,

    /// Column range filter (e.g., "0-35", "3-20")
    #[arg(short, long)]
    columns: Option<String>,
}

#[derive(Debug)]
struct FileInfo {
    path: Utf8PathBuf,
    full_path: Utf8PathBuf,
    alias: String,
    original_content: String,
    original_mtime: SystemTime,
}

#[derive(Debug)]
struct MatchLine {
    file_idx: usize,
    lineno: usize,
    original_content: String,
}

fn main() -> Result<()> {
    env_logger::init();

    let args = Args::parse();

    // Parse column range if provided
    let column_range = if let Some(ref col_str) = args.columns {
        Some(range_parser::parse(col_str.as_str()).context("invalid column range")?)
    } else {
        None
    };

    // Run ugrep to get matches
    let mut cmd = Command::new("ugrep");
    cmd.arg("-nrkP").arg("--ignore-files").arg(&args.pattern);

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

    let output = cmd
        .output()
        .context("failed to run ugrep (is ugrep installed?)")?;

    if !output.status.success() {
        eprintln!("ugrep exited with status {:?}", output.status.code());
        eprintln!("Error: {:?}", &output.stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        println!("No matches found.");
        return Ok(());
    }

    // Compile exclude pattern if provided
    let exclude_re = args
        .exclude
        .as_ref()
        .map(|pat| {
            if args.ignore_case {
                regex::RegexBuilder::new(pat).case_insensitive(true).build()
            } else {
                Regex::new(pat)
            }
        })
        .transpose()
        .context("invalid exclude pattern")?;

    // Parse ugrep output: "path:line:column:content"
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

                // Apply exclude filter if provided
                if let Some(ref exclude_re) = exclude_re
                    && exclude_re.is_match(content)
                {
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
    let truncated = if matches.len() > args.max_count {
        debug!("Truncating {} matches to {}", matches.len(), args.max_count);
        matches.truncate(args.max_count);
        true
    } else {
        false
    };

    // Sort matches by filename then line number for stability
    matches.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    // Build file info with aliases
    let mut files: Vec<FileInfo> = Vec::new();
    let mut path_to_idx: BTreeMap<Utf8PathBuf, usize> = BTreeMap::new();
    let mut alias_iter = alias_iter();

    for (path, _, _) in &matches {
        if !path_to_idx.contains_key(path) {
            let idx = files.len();

            // Get next alias or warn if we've run out
            let alias = match alias_iter.next() {
                Some(a) => a,
                None => {
                    eprintln!(
                        "Warning: Too many files (there are only A..ZZZ). Stopping at file {}",
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

            files.push(FileInfo {
                path: path.clone(),
                full_path,
                alias,
                original_content: content,
                original_mtime: mtime,
            });
            path_to_idx.insert(path.clone(), idx);
        }
    }

    // Build match lines
    let mut match_lines: Vec<MatchLine> = Vec::new();
    for (path, lineno, content) in matches {
        // Only include matches from files we have aliases for
        if let Some(&file_idx) = path_to_idx.get(&path) {
            match_lines.push(MatchLine {
                file_idx,
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
        .join(format!("fixall-edit-{}.fixall.txt", ts))
        .try_into()?;

    write_virtual_buffer(&tmp, &args.pattern, &match_lines, &files)?;

    // Warn if matches were truncated
    if truncated {
        eprintln!(
            "Warning: Matches truncated to {} (use --max-count to adjust)",
            args.max_count
        );
    }

    // Keep original text for change detection
    let original = fs::read_to_string(&tmp)?;

    // Launch editor (e.g. subl --wait <file>)
    let mut parts = args.editor.split_whitespace();
    let cmd = parts.next().context("empty editor command")?;
    let args_vec: Vec<_> = parts.chain(std::iter::once(tmp.as_ref())).collect();

    Command::new(cmd)
        .args(&args_vec)
        .status()
        .context("launching editor")?;

    // If file content changed, apply edits
    let new_text = fs::read_to_string(&tmp)?;
    if new_text == original {
        println!("No changes saved. Exiting.");
        return Ok(());
    }

    apply_changes(&new_text, &files)?;

    println!("Applied edits successfully.");
    Ok(())
}

/// Generate alternating-length aliases (A, AA, B, AB, C, AC, …)
pub fn alias_iter() -> impl Iterator<Item = String> {
    let alphabet = 'A'..='Z';

    // 1. Create iterators that produce owned Strings, not borrowing any local variables.
    //    We clone the `alphabet` range for each product.
    let singles = alphabet.clone().map(|c| c.to_string());

    let doubles =
        iproduct!(alphabet.clone(), alphabet.clone()).map(|(c1, c2)| format!("{}{}", c1, c2));

    let triples = iproduct!(alphabet.clone(), alphabet.clone(), alphabet.clone())
        .map(|(c1, c2, c3)| format!("{}{}{}", c1, c2, c3));

    // 2. Eagerly collect all generated strings into a Vec.
    let all_strings: Vec<String> = singles.chain(doubles).chain(triples).collect();

    // 3. Build the final iterator chain by consuming the vector.
    //    Since we use `into_iter()`, the entire subsequent chain operates on owned data.
    let final_sequence: Vec<String> = all_strings
        .into_iter()
        .chunks(26)
        .into_iter()
        .map(|chunk| chunk.collect_vec())
        .chunks(2)
        .into_iter()
        .flat_map(|mut pair_of_chunks| {
            let first = pair_of_chunks.next().unwrap();
            let second = pair_of_chunks.next().unwrap_or_default();

            first.into_iter().interleave(second.into_iter())
        })
        .collect();

    // Return a simple iterator over the now-owned final sequence.
    final_sequence.into_iter()
}

fn write_virtual_buffer(
    tmp: &Utf8Path,
    regex: &str,
    match_lines: &[MatchLine],
    files: &[FileInfo],
) -> Result<()> {
    let mut file = fs::File::create(tmp)?;

    writeln!(file, "# fixall – bulk regex editing buffer")?;
    writeln!(file, "# Regex: {regex}")?;
    writeln!(file, "# Save and close to apply changes.")?;
    writeln!(file, "# Lines starting with '#' are ignored.")?;
    writeln!(file, "#")?;
    writeln!(file, "# --- Begin editable lines ---")?;
    writeln!(file)?;

    let max_line_len = match_lines
        .iter()
        .map(|m| m.lineno.to_string().len())
        .max()
        .unwrap_or(1);

    for m in match_lines {
        let alias = &files[m.file_idx].alias;
        writeln!(
            file,
            "{alias:>3} {lineno:>width$} | {content}",
            lineno = m.lineno,
            content = m.original_content,
            width = max_line_len
        )?;
    }

    writeln!(file)?;
    writeln!(file, "# --- File Aliases ---")?;
    for f in files {
        writeln!(file, "# {:>3} = {}", f.alias, f.full_path)?;
    }

    Ok(())
}

fn apply_changes(new_text: &str, files: &[FileInfo]) -> Result<()> {
    let line_re = Regex::new(r"^\s*([A-Z]+)\s+(\d+)\s+\|\s(.*)$")?;

    // Build alias -> file index map
    let alias_to_idx: BTreeMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(idx, f)| (f.alias.as_str(), idx))
        .collect();

    // Track changes: (file_idx, lineno) -> new_content
    let mut changes: BTreeMap<(usize, usize), String> = BTreeMap::new();

    for line in new_text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        if let Some(cap) = line_re.captures(line) {
            let alias = cap.get(1).unwrap().as_str();
            let lineno: usize = cap.get(2).unwrap().as_str().parse()?;
            let new_content = cap.get(3).unwrap().as_str();

            if let Some(&file_idx) = alias_to_idx.get(alias) {
                let file = &files[file_idx];

                // Get the original line from the file
                let original_lines: Vec<&str> = file.original_content.lines().collect();

                if let Some(&original_line) = original_lines.get(lineno - 1) {
                    // Only track if content changed
                    if original_line != new_content {
                        debug!("Change detected at {}:{}", file.path, lineno);
                        debug!("  Original: {:?}", original_line);
                        debug!("  New:      {:?}", new_content);
                        changes.insert((file_idx, lineno), new_content.to_string());
                    } else {
                        debug!("No change at {}:{}", file.path, lineno);
                        debug!("  Both are: {:?}", original_line);
                    }
                }
            }
        }
    }

    if changes.is_empty() {
        println!("No actual changes detected.");
        return Ok(());
    }

    // Group changes by file
    let mut files_to_update: BTreeMap<usize, Vec<(usize, String)>> = BTreeMap::new();
    for ((file_idx, lineno), content) in changes {
        files_to_update
            .entry(file_idx)
            .or_default()
            .push((lineno, content));
    }

    // Apply changes to each file
    for (file_idx, file_changes) in files_to_update {
        let file = &files[file_idx];

        // Check if file was modified since we started
        let current_metadata = fs::metadata(&file.full_path)
            .with_context(|| format!("reading current metadata for {}", file.full_path))?;
        let current_mtime = current_metadata
            .modified()
            .with_context(|| format!("getting current modification time for {}", file.full_path))?;

        if current_mtime != file.original_mtime {
            eprintln!(
                "Error: file {} was modified during editing session, skipping",
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

        // Apply changes
        for (lineno, new_content) in file_changes {
            if let Some(line_slot) = lines.get_mut(lineno - 1) {
                *line_slot = new_content;
            } else {
                eprintln!("Warning: line {lineno} out of range for {}", file.path);
            }
        }

        // Reconstruct file with proper trailing newline handling
        let mut joined = lines.join("\n");
        if has_trailing_newline {
            joined.push('\n');
        }

        fs::write(&file.full_path, joined)
            .with_context(|| format!("writing changes back to {}", file.full_path))?;

        println!("Updated {}", file.path);
    }

    Ok(())
}
