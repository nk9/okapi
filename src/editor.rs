use crate::{Args, FileAlias, FileInfo, MatchLine};
use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use crossterm::style::Stylize;
use regex::Regex;
use similar::{ChangeTag, TextDiff};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run_editor_session(
    args: &Args,
    label: &str,
    match_lines: Vec<MatchLine>,
    files: BTreeMap<FileAlias, FileInfo>,
) -> Result<()> {
    let tmp_dir = tempdir().context("creating temporary directory")?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let tmp: Utf8PathBuf = tmp_dir
        .path()
        .join(format!("edit-{}.okapi.txt", ts))
        .try_into()?;

    write_virtual_buffer(&tmp, label, &match_lines, &files)?;
    let original_text = fs::read_to_string(&tmp)?;

    launch_editor(args, &tmp)?;

    let new_text = fs::read_to_string(&tmp)?;
    if new_text == original_text {
        println!("No changes saved. Exiting.");
        return Ok(());
    }

    apply_changes(&new_text, &files)
}

fn launch_editor(args: &Args, path: &Utf8Path) -> Result<()> {
    let editor_cmd = args
        .editor
        .clone()
        .or_else(|| std::env::var("EDITOR").ok())
        .unwrap_or_else(|| "vim".to_string());

    let mut parts = editor_cmd.split_whitespace();
    let cmd = parts.next().context("empty editor command")?;
    let args_vec: Vec<_> = parts.chain(std::iter::once(path.as_ref())).collect();

    Command::new(cmd)
        .args(&args_vec)
        .status()
        .context(format!("launching editor: {}", editor_cmd))?;
    Ok(())
}

fn write_virtual_buffer(
    tmp: &Utf8Path,
    label: &str,
    match_lines: &[MatchLine],
    files: &BTreeMap<FileAlias, FileInfo>,
) -> Result<()> {
    let mut file = fs::File::create(tmp)?;
    writeln!(file, "# okapi – bulk editing buffer\n# {}\n#", label)?;
    writeln!(file, "# - Save and close to apply changes.")?;
    writeln!(
        file,
        "# - Unchanged lines and those starting with '#' are ignored."
    )?;
    writeln!(
        file,
        "# - Delete everything after the shade block (▓) to remove a line.\n#"
    )?;
    writeln!(file, "# --- Begin editable lines ---\n")?;

    let max_w = match_lines
        .iter()
        .map(|m| (m.lineno as f64).log10() as usize + 1)
        .max()
        .unwrap_or(1);
    let mut current_alias = None;
    let mut use_heavy = false;

    for m in match_lines {
        if current_alias != Some(m.alias) {
            current_alias = Some(m.alias);
            use_heavy = !use_heavy;
        }
        let pipe = if use_heavy { "▓" } else { "░" };
        writeln!(
            file,
            "{:>3} {:>width$} {} {}",
            m.alias,
            m.lineno,
            pipe,
            m.original_content,
            width = max_w
        )?;
    }

    writeln!(file, "\n# --- File Aliases ---")?;
    for (_, f) in files {
        writeln!(file, "# {:>3} = {}", f.alias, f.full_path)?;
    }
    Ok(())
}

fn apply_changes(new_text: &str, files: &BTreeMap<FileAlias, FileInfo>) -> Result<()> {
    let line_re = Regex::new(r"^\s*([A-Z]+)\s+(\d+)\s+[▓░]\s?(.*)$")?;
    let mut changes: HashMap<FileAlias, HashMap<usize, Option<String>>> = HashMap::new();
    let mut total_lines = 0;

    for line in new_text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        if line.chars().filter(|&c| c == '▓' || c == '░').count() > 1 {
            continue;
        }

        if let Some(cap) = line_re.captures(line) {
            total_lines += 1;
            let alias = FileAlias::from_str(cap.get(1).unwrap().as_str());
            let lineno: usize = cap.get(2).unwrap().as_str().parse()?;
            let new_content = cap.get(3).unwrap().as_str();

            if let Some(file) = files.get(&alias) {
                let orig_lines: Vec<&str> = file.original_content.lines().collect();
                if let Some(&orig) = orig_lines.get(lineno - 1) {
                    if new_content.trim().is_empty() {
                        changes.entry(alias).or_default().insert(lineno, None);
                    } else if orig != new_content {
                        changes
                            .entry(alias)
                            .or_default()
                            .insert(lineno, Some(new_content.to_string()));
                    }
                }
            }
        }
    }

    perform_file_updates(changes, files, total_lines)
}

fn perform_file_updates(
    updates: HashMap<FileAlias, HashMap<usize, Option<String>>>,
    files: &BTreeMap<FileAlias, FileInfo>,
    all_lines: usize,
) -> Result<()> {
    if updates.is_empty() {
        println!("No actual changes detected.");
        return Ok(());
    }

    let mut line_count = 0;
    let mut file_count = 0;

    for (alias, mut file_changes) in updates {
        let file = files.get(&alias).context("missing file alias")?;
        let current_mtime = fs::metadata(&file.full_path)?.modified()?;

        let mut lines: Vec<String>;

        if current_mtime != file.original_mtime {
            let current_content = fs::read_to_string(&file.full_path)?;
            let current_lines: Vec<&str> = current_content.lines().collect();
            let original_lines: Vec<&str> = file.original_content.lines().collect();

            let mut conflicts = Vec::new();
            let mut already_applied_indices = Vec::new();

            for (&idx, new_val) in &file_changes {
                let current_on_disk = current_lines.get(idx - 1).copied().unwrap_or("");
                let user_intended = new_val.as_deref().unwrap_or("");
                let original_state = original_lines.get(idx - 1).copied().unwrap_or("");

                if current_on_disk == user_intended {
                    already_applied_indices.push(idx);
                } else if current_on_disk != original_state {
                    conflicts.push((idx, original_state, user_intended));
                }
            }

            if !conflicts.is_empty() {
                eprintln!("\nConflict in {}: modified externally", file.path);
                for (idx, old, new) in conflicts {
                    print_diff(idx, old, new);
                }
                continue;
            }

            for idx in already_applied_indices {
                file_changes.remove(&idx);
                line_count += 1;
            }

            lines = current_lines.into_iter().map(|s| s.to_string()).collect();
        } else {
            lines = file
                .original_content
                .lines()
                .map(|s| s.to_string())
                .collect();
        }

        if !file_changes.is_empty() {
            let mut idx = 0;
            lines.retain_mut(|line| {
                idx += 1;
                if let Some(change) = file_changes.remove(&idx) {
                    line_count += 1;
                    if let Some(new_val) = change {
                        *line = new_val;
                        return true;
                    }
                    return false;
                }
                true
            });

            let mut output = lines.join("\n");
            if file.original_content.ends_with('\n') {
                output.push('\n');
            }

            fs::write(&file.full_path, output)?;
            file_count += 1;
            println!("Updated {}", file.path);
        } else if line_count > 0 {
            file_count += 1;
            println!("Verified {} (already up to date)", file.path);
        }
    }

    print_summary(line_count, file_count, all_lines, files.len());
    Ok(())
}

/// Prints a two-line character-level diff for a conflict
fn print_diff(lineno: usize, original: &str, updated: &str) {
    let diff = TextDiff::from_chars(original, updated);
    let changes: Vec<_> = diff.iter_all_changes().collect();

    // Line 1: Old version with removals in red
    print!(" orig: {:>4} ░ ", lineno);
    for change in &changes {
        match change.tag() {
            ChangeTag::Delete => print!("{}", change.value().red()),
            ChangeTag::Equal => print!("{}", change.value()),
            ChangeTag::Insert => {} // Skip additions in "old" view
        }
    }
    println!();

    // Line 2: New version with additions in green
    print!("okapi:  {:>4} > ░ ", lineno);
    for change in &changes {
        match change.tag() {
            ChangeTag::Insert => print!("{}", change.value().green()),
            ChangeTag::Equal => print!("{}", change.value()),
            ChangeTag::Delete => {} // Skip removals in "new" view
        }
    }
    println!();
}

fn print_summary(lines_chg: usize, files_chg: usize, lines_total: usize, files_total: usize) {
    let w = (lines_total as f64).log10().ceil() as usize;
    println!(
        "\n  Changed: {:>w$} line(s), {:>w$} file(s)",
        lines_chg, files_chg
    );
    println!(
        "Unchanged: {:>w$} line(s), {:>w$} file(s)",
        lines_total - lines_chg,
        files_total - files_chg
    );
}
