use crate::config::Config;
use crate::{FileAlias, FileInfo, MatchLine};
use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use crossterm::style::Stylize;
use log::info;
use regex::Regex;
use similar::{ChangeTag, TextDiff};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::process::{Command, ExitStatus};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run_editor_session(
    config: &Config,
    label: &str,
    match_lines: Vec<MatchLine>,
    files: BTreeMap<FileAlias, FileInfo>,
) -> Result<()> {
    let tmp_dir = tempdir().context("creating temporary directory")?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let tmp_path: Utf8PathBuf = tmp_dir
        .path()
        .join(format!("edit-{}.okapi.txt", ts))
        .try_into()?;

    write_virtual_buffer(&tmp_path, label, &match_lines, &files)?;
    let original_text = fs::read_to_string(&tmp_path)?;

    let status = launch_editor(config, &tmp_path)?;

    let new_text = fs::read_to_string(&tmp_path)?;
    if new_text == original_text {
        println!("No changes saved. Exiting.");
        return Ok(());
    }

    // 1. Parse the changes into memory first
    let (updates, total_lines) = parse_changes(&new_text, &files)?;
    let change_count = updates.values().map(|m| m.len()).sum::<usize>();

    if change_count == 0 {
        println!("No functional changes detected. Exiting.");
        return Ok(());
    }

    // 2. Determine if we should prompt the user
    let mut should_persist = status.success();

    if !status.success() {
        println!(
            "{} Editor did not exit cleanly, but some changes were already saved.\nIf you abort, the saved buffer will be moved to a temporary file.",
            "WARNING:".yellow().bold()
        );
        should_persist = prompt_user(format!("Persist {} changes anyway?", change_count))?;
    }

    // 3. Act on decision
    if should_persist {
        perform_file_updates(updates, &files, total_lines)
    } else {
        // Move buffer file so it isn't deleted when tmp_dir drops
        let abandoned_path = std::env::temp_dir().join(format!("okapi-abandoned-{}.txt", ts));
        fs::copy(&tmp_path, &abandoned_path)?;

        println!("\nChanges abandoned.");
        println!(
            "The virtual buffer was saved to: {}",
            abandoned_path.display()
        );
        Ok(())
    }
}

fn prompt_user(msg: String) -> Result<bool> {
    print!("\n{} [y/N]: ", msg);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    Ok(input == "y" || input == "yes")
}

fn launch_editor(config: &Config, path: &Utf8Path) -> Result<ExitStatus> {
    let mut parts = config.editor.split_whitespace();
    let cmd = parts.next().context("empty editor command")?;
    let args_vec: Vec<_> = parts.chain(std::iter::once(path.as_ref())).collect();
    info!("Launching {} {}", cmd, args_vec.join(" "));

    let status = Command::new(cmd)
        .args(&args_vec)
        .status()
        .context(format!("launching editor: {}", config.editor))?;

    Ok(status)
}

// Split the old apply_changes into two: parse and perform
fn parse_changes(
    new_text: &str,
    files: &BTreeMap<FileAlias, FileInfo>,
) -> Result<(HashMap<FileAlias, HashMap<usize, Option<String>>>, usize)> {
    let line_re = Regex::new(r"^\s*([A-Z]+)\s+(\d+)\s+[▓░]\s?(.*)$")?;
    let mut raw: HashMap<FileAlias, HashMap<usize, Vec<Option<String>>>> = HashMap::new();
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
                    let new_val = if new_content.trim().is_empty() {
                        None
                    } else if orig != new_content {
                        Some(new_content.to_string())
                    } else {
                        continue;
                    };
                    raw.entry(alias)
                        .or_default()
                        .entry(lineno)
                        .or_default()
                        .push(new_val);
                }
            }
        }
    }

    let updates = resolve_duplicates(raw, files)?;
    Ok((updates, total_lines))
}

fn resolve_duplicates(
    raw: HashMap<FileAlias, HashMap<usize, Vec<Option<String>>>>,
    files: &BTreeMap<FileAlias, FileInfo>,
) -> Result<HashMap<FileAlias, HashMap<usize, Option<String>>>> {
    let mut updates: HashMap<FileAlias, HashMap<usize, Option<String>>> = HashMap::new();
    let mut had_conflict = false;

    for (alias, lines) in raw {
        let mut resolved = HashMap::new();
        for (lineno, versions) in lines {
            let distinct: HashSet<&Option<String>> = versions.iter().collect();
            if distinct.len() > 1 {
                had_conflict = true;
                let orig = files
                    .get(&alias)
                    .and_then(|f| f.original_content.lines().nth(lineno - 1))
                    .unwrap_or("");
                print_duplicate_conflict(alias, lineno, orig, &versions);
            } else {
                resolved.insert(lineno, versions.into_iter().next().unwrap());
            }
        }
        updates.insert(alias, resolved);
    }

    if had_conflict {
        anyhow::bail!("duplicate edits with conflicting content; see listing above");
    }
    Ok(updates)
}

fn print_duplicate_conflict(
    alias: FileAlias,
    lineno: usize,
    orig: &str,
    versions: &[Option<String>],
) {
    eprintln!(
        "{} {} line {} edited multiple times with different content:",
        "CONFLICT:".red().bold(),
        alias,
        lineno
    );
    eprintln!("  orig: {}", orig);
    for (i, v) in versions.iter().enumerate() {
        let text = v.as_deref().unwrap_or("<deleted>");
        eprint!("    #{}: ", i + 1);
        print_char_diff(orig, text);
    }
}

fn print_char_diff(orig: &str, updated: &str) {
    let diff = TextDiff::from_chars(orig, updated);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => print!("{}", change.value().green()),
            ChangeTag::Delete => print!("{}", change.value().red().crossed_out()),
            ChangeTag::Equal => print!("{}", change.value()),
        }
    }
    println!();
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
                eprintln!("Conflict in {}: modified externally", f.path);
                for (i, d, o, n) in conflicts {
                    print_diff(i, &d, &o, &n);
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
) -> Result<(Option<String>, usize), Vec<(usize, String, String, String)>> {
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
                conflicts.push((idx, disk.to_string(), orig.to_string(), user.to_string()));
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

fn print_diff(lineno: usize, disk: &str, original: &str, updated: &str) {
    let diff = TextDiff::from_chars(original, disk);
    let changes: Vec<_> = diff.iter_all_changes().collect();

    // Line 1: On-disk version with external modifications in yellow
    print!(" disk: {:>4} ░ ", lineno);
    for change in &changes {
        match change.tag() {
            ChangeTag::Insert => print!("{}", change.value().bold().yellow()),
            ChangeTag::Equal => print!("{}", change.value()),
            ChangeTag::Delete => {}
        }
    }
    println!();

    let diff = TextDiff::from_chars(original, updated);
    let changes: Vec<_> = diff.iter_all_changes().collect();

    // Line 1: Old version with removals in red
    print!(" orig: {:>4} ░ ", lineno);
    for change in &changes {
        match change.tag() {
            ChangeTag::Delete => print!("{}", change.value().bold().red()),
            ChangeTag::Equal => print!("{}", change.value()),
            ChangeTag::Insert => {} // Skip additions in "old" view
        }
    }
    println!();

    // Line 2: New version with additions in green
    print!("okapi: {:>4} ░ ", lineno);
    for change in &changes {
        match change.tag() {
            ChangeTag::Insert => print!("{}", change.value().bold().green()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_file(alias: FileAlias, content: &str) -> FileInfo {
        FileInfo {
            path: Utf8PathBuf::from("test.txt"),
            full_path: Utf8PathBuf::from("test.txt"),
            alias,
            original_content: content.to_string(),
            original_mtime: SystemTime::now(),
        }
    }

    #[test]
    fn duplicate_edit_with_conflicting_content_is_rejected() {
        let alias = FileAlias::from_str("A");
        let mut files = BTreeMap::new();
        files.insert(alias, make_file(alias, "line one\n"));

        // Same alias+lineno appears twice in the buffer with different
        // replacement text — e.g. from a copy-paste in the editor.
        let buffer = "A   1 ▓ first version\nA   1 ░ second version\n";

        let result = parse_changes(buffer, &files);

        assert!(
            result.is_err(),
            "expected conflicting duplicate edits to be rejected, \
             but they were silently merged (second write wins)"
        );
    }

    #[test]
    fn identical_duplicate_edits_to_same_alias_succeed() {
        let alias = FileAlias::from_str("A");
        let mut files = BTreeMap::new();
        files.insert(alias, make_file(alias, "line one\n"));

        // Same alias+lineno appears three times with identical content —
        // e.g. the user selected and re-saved the same block twice.
        let buffer = "A   1 ▓ same version\nA   1 ░ same version\nA   1 ▓ same version\n";

        let result = parse_changes(buffer, &files);

        assert!(
            result.is_ok(),
            "identical duplicates should not be treated as conflicts"
        );
        let (updates, _) = result.unwrap();
        assert_eq!(
            updates.get(&alias).and_then(|m| m.get(&1)),
            Some(&Some("same version".to_string()))
        );
    }
}
