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
    let (mut line_count, mut file_count) = (0, 0);

    for (alias, changes) in updates {
        let f = files.get(&alias).context("missing file alias")?;
        let on_disk = fs::read_to_string(&f.full_path)?;

        match resolve_file_changes(&on_disk, &f.original_content, &changes) {
            Err(conflicts) => {
                eprintln!("\nConflict in {}: modified externally", f.path);
                for (i, o, n) in conflicts {
                    print_diff(i, &o, &n);
                }
            }
            Ok((new_text, affected)) => {
                if let Some(txt) = new_text {
                    fs::write(&f.full_path, txt)?;
                    println!("Updated {}", f.path);
                } else if affected > 0 {
                    println!("Verified {} (already up to date)", f.path);
                }
                line_count += affected;
                file_count += 1;
            }
        }
    }

    print_summary(line_count, file_count, all_lines, files.len());
    Ok(())
}

fn resolve_file_changes(
    on_disk: &str,
    original: &str,
    changes: &HashMap<usize, Option<String>>,
) -> Result<(Option<String>, usize), Vec<(usize, String, String)>> {
    let mut conflicts = Vec::new();
    let mut modified = false;
    let disk_lines: Vec<&str> = on_disk.lines().collect();
    let orig_lines: Vec<&str> = original.lines().collect();

    for (&idx, user_val) in changes {
        let disk = disk_lines.get(idx - 1).copied().unwrap_or("");
        let orig = orig_lines.get(idx - 1).copied().unwrap_or("");
        let user = user_val.as_deref().unwrap_or("");

        if disk != user {
            if disk == orig {
                modified = true;
            } else {
                conflicts.push((idx, orig.to_string(), user.to_string()));
            }
        }
    }

    if !conflicts.is_empty() {
        return Err(conflicts);
    }
    if !modified {
        return Ok((None, changes.len()));
    }

    let mut idx = 0;
    let mut final_lines: Vec<String> = disk_lines.iter().map(|s| s.to_string()).collect();
    final_lines.retain_mut(|line| {
        idx += 1;
        match changes.get(&idx) {
            Some(Some(new_val)) => {
                *line = new_val.clone();
                true
            }
            Some(None) => false,
            None => true,
        }
    });

    let mut output = final_lines.join("\n");
    if original.ends_with('\n') {
        output.push('\n');
    }
    Ok((Some(output), changes.len()))
}

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
