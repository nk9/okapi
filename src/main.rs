use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use clap::Parser;
use regex::Regex;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::process::Command;

/// Edit all regex matches from many files in one buffer.
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// PCRE-compatible regex pattern (passed to ripgrep -P)
    pattern: String,

    /// Files or directories to search (passed to rg)
    #[arg(value_name = "PATHS", num_args = 0..)]
    paths: Vec<Utf8PathBuf>,

    /// Editor command (default: "subl --wait")
    #[arg(short, long, default_value = "subl --wait")]
    editor: String,

    #[arg(short, long)]
    max_count: Option<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Run ripgrep to get matches
    let mut cmd = Command::new("rg");
    cmd.arg("-nP").arg(&args.pattern).args(&args.paths).arg("--no-heading");

    if let Some(max_count) = args.max_count {
        cmd.arg("--max-count").arg(max_count.to_string());
    }

    let output = cmd
        .output()
        .context("failed to run ripgrep (is rg installed?)")?;

    if !output.status.success() {
        eprintln!("ripgrep exited with status {:?}", output.status.code());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        println!("No matches found.");
        return Ok(());
    }

    // Parse ripgrep output: "path:line:content"
    let mut matches: Vec<(Utf8PathBuf, usize, String)> = Vec::new();
    for line in stdout.lines() {
        if let Some((path, rest)) = line.split_once(':') {
            if let Some((lineno, content)) = rest.split_once(':') {
                let path = Utf8PathBuf::from(path);
                if let Ok(line_no) = lineno.parse::<usize>() {
                    matches.push((path, line_no, content.to_string()));
                }
            }
        }
    }

    // Sort matches by filename then line number for stability
    matches.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    // Build alternating-length file aliases
    let mut file_aliases = BTreeMap::<Utf8PathBuf, String>::new();
    let mut alias_iter = generate_alias();
    for (path, _, _) in &matches {
        if !file_aliases.contains_key(path) {
            file_aliases.insert(path.clone(), alias_iter.next().unwrap().to_string());
        }
    }

    // Prepare the virtual editing buffer
    let tmp_dir = tempdir().context("creating temporary directory")?;
    let tmp: Utf8PathBuf = tmp_dir
        .path()
        .join("fixall-edit.fixall.txt")
        .try_into()?;

    write_virtual_buffer(&tmp, &args.pattern, &matches, &file_aliases)?;

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

    apply_changes(&new_text, &file_aliases)?;

    println!("Applied edits successfully.");
    Ok(())
}

/// Generate alternating-length aliases (A, AA, B, AB, C, AC, …)
fn generate_alias() -> impl Iterator<Item = String> {
    let letters: Vec<char> = (b'A'..=b'Z').map(|c| c as char).collect();

    // Collect into a Vec<String> and return its into_iter() — simple and compiles.
    let mut v = Vec::new();

    // First 52: A, AA, B, AB, C, AC, ...
    for &c in &letters {
        v.push(c.to_string());
        v.push(format!("A{}", c));
    }

    // Beyond first 52: interleave BA, CA, BB, CB, BC, CC, ...
    for &first in &letters {
        for &second in &letters {
            // Skip any already yielded in first 52 (A..Z and A*)
            if first != 'A' && second != 'A' {
                v.push(format!("{}{}", first, second));
            }
        }
    }

    v.into_iter()
}

fn write_virtual_buffer(
    tmp: &Utf8Path,
    regex: &str,
    matches: &[(Utf8PathBuf, usize, String)],
    file_aliases: &BTreeMap<Utf8PathBuf, String>,
) -> Result<()> {
    let mut file = fs::File::create(tmp)?;

    writeln!(file, "# fixall — bulk regex editing buffer")?;
    writeln!(file, "# Regex: {regex}")?;
    writeln!(file, "# Save and close to apply changes.")?;
    writeln!(file, "# Lines starting with '#' are ignored.")?;
    writeln!(file, "#")?;
    writeln!(file, "# --- Begin editable lines ---")?;
    writeln!(file)?;

    let max_line_len = matches
        .iter()
        .map(|(_, l, _)| l.to_string().len())
        .max()
        .unwrap_or(1);

    for (path, lineno, content) in matches {
        let alias = file_aliases.get(path).unwrap();
        writeln!(file, "{alias:>2} {lineno:>width$} | {content}", width = max_line_len)?;
    }

    writeln!(file)?;
    writeln!(file, "# --- File Aliases ---")?;
    for (path, alias) in file_aliases {
        writeln!(file, "# {alias:>2} = {path}")?;
    }

    Ok(())
}

fn apply_changes(new_text: &str, file_aliases: &BTreeMap<Utf8PathBuf, String>) -> Result<()> {
    let line_re = Regex::new(r"^([A-Z]+)\s+(\d+)\s+\|\s(.*)$")?;

    // Build reverse map for alias -> path
    let alias_to_path: BTreeMap<String, &Utf8PathBuf> =
        file_aliases.iter().map(|(p, a)| (a.clone(), p)).collect();

    let mut file_cache: BTreeMap<&Utf8PathBuf, Vec<String>> = BTreeMap::new();

    for line in new_text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }

        if let Some(cap) = line_re.captures(line) {
            let alias = cap.get(1).unwrap().as_str();
            let lineno: usize = cap.get(2).unwrap().as_str().parse()?;
            let content = cap.get(3).unwrap().as_str();

            if let Some(path) = alias_to_path.get(alias) {
                let lines = file_cache.entry(path).or_insert_with(|| {
                    fs::read_to_string(path)
                        .unwrap_or_default()
                        .lines()
                        .map(|s| s.to_string())
                        .collect()
                });

                if let Some(line_slot) = lines.get_mut(lineno - 1) {
                    *line_slot = content.to_string();
                } else {
                    eprintln!("Warning: line {lineno} out of range for {path}");
                }
            }
        }
    }

    for (path, lines) in file_cache {
        let joined = lines.join("\n") + "\n";
        fs::write(path, joined)
            .with_context(|| format!("writing changes back to {}", path.as_str()))?;
    }

    Ok(())
}
