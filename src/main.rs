mod editor;
mod file_alias;
mod file_loader;
mod search;

use anyhow::Result;
use camino::Utf8PathBuf;
use clap::{ArgGroup, Parser};
use file_alias::FileAlias;

#[derive(Parser, Debug)]
#[command(author, version, about)]
// Create a group that requires exactly one of 'pattern' or 'file'
#[command(group(
    ArgGroup::new("input")
        .required(true)
        .args(["pattern", "file"]),
))]
pub struct Args {
    /// Rust regex pattern (passed to ripgrep)
    #[arg(required_unless_present = "file")]
    pub pattern: Option<String>,

    /// Path to a file of path:line entries, one per line. Pass '-' to read from stdin
    #[arg(short, long, conflicts_with = "pattern", value_name = "FILE_PATH")]
    pub file: Option<Utf8PathBuf>,

    #[arg(value_name = "PATHS", num_args = 0..)]
    pub paths: Vec<Utf8PathBuf>,

    /// Command run to edit the virtual buffer. Search order: --editor, $EDITOR, `vim`. Ex: `subl --wait`
    #[arg(short = 'd', long, value_name = "COMMAND")]
    pub editor: Option<String>,

    /// Truncate matches to prevent an overlarge virtual buffer
    #[arg(short, long, default_value = "1000")]
    pub max_count: usize,

    /// Lines matching the initial pattern are excluded if they match this one
    #[arg(short, long, value_name = "PATTERN")]
    pub exclude: Vec<String>,

    /// Run a case-insensitive search. Passed though to ripgrep. Also applies to --exclude pattern
    #[arg(short, long)]
    pub ignore_case: bool,

    /// Prefixed to paths found in --file and paths provided as positional args
    #[arg(short, long, value_name = "PATH")]
    pub working_directory: Option<Utf8PathBuf>,

    /// Inclusive range of text columns which matches must START within.
    /// Ex: 3.. (skip first 3 chars), ..5,20.. (skip chars 6-19)
    #[arg(short, long)]
    pub columns: Option<String>,

    // Extra args are passed to ripgrep
    #[arg(last = true)]
    pub extra_args: Vec<String>,
}

#[derive(Debug)]
pub struct FileInfo {
    pub path: Utf8PathBuf,
    pub full_path: Utf8PathBuf,
    pub alias: FileAlias,
    pub original_content: String,
    pub original_mtime: std::time::SystemTime,
}

#[derive(Debug)]
pub struct MatchLine {
    pub alias: FileAlias,
    pub lineno: usize,
    pub original_content: String,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    let (match_lines, files, label) = if let Some(ref list_path) = args.file {
        if list_path == "-" {
            file_loader::load_from_stdin(&args)?
        } else {
            file_loader::load_from_list(list_path, &args)?
        }
    } else {
        search::run_ripgrep_search(&args)?
    };

    if match_lines.is_empty() {
        println!("No matches found.");
        return Ok(());
    }

    editor::run_editor_session(&args, &label, match_lines, files)?;

    Ok(())
}

pub fn alias_iter() -> impl Iterator<Item = FileAlias> {
    let alphabet = 'A'..='Z';
    let singles = alphabet.clone().map(|c| FileAlias::new(&[c]));
    let doubles = itertools::iproduct!(alphabet.clone(), alphabet.clone())
        .map(|(c1, c2)| FileAlias::new(&[c1, c2]));
    let triples = itertools::iproduct!(alphabet.clone(), alphabet.clone(), alphabet.clone())
        .map(|(c1, c2, c3)| FileAlias::new(&[c1, c2, c3]));

    singles.chain(doubles).chain(triples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alias_sequence() {
        let mut it = alias_iter();
        assert_eq!(it.next().unwrap().to_string(), "A");

        // Skip remaining 25 single letters
        for _ in 0..25 {
            it.next();
        }

        // Should start doubles
        assert_eq!(it.next().unwrap().to_string(), "AA");
        assert_eq!(it.next().unwrap().to_string(), "AB");

        // Should eventually hit triples
        let mut triples = alias_iter().skip(26 + (26 * 26));
        assert_eq!(triples.next().unwrap().to_string(), "AAA");
    }
}
