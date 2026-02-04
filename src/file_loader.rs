use crate::{alias_iter, Args, FileAlias, FileInfo, MatchLine};
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashSet};
use std::{env, fs};

pub fn load_from_list(
    list_path: &Utf8PathBuf,
    args: &Args,
) -> Result<(Vec<MatchLine>, BTreeMap<FileAlias, FileInfo>, String)> {
    let content = fs::read_to_string(list_path).context("reading list file")?;
    let mut requests = Vec::new();

    // Get the absolute base path (either -w or current process CWD)
    let absolute_base = get_absolute_base(args)?;

    for (idx, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (path_str, line_str) = line
            .rsplit_once(':')
            .with_context(|| format!("missing colon separator on line {}", idx + 1))?;

        let lineno = line_str.parse::<usize>().context("parsing line number")?;

        // Resolve path: absolute paths stay as-is, relative paths joined to absolute_base
        let path = Utf8PathBuf::from(path_str);
        let full_path = if path.is_absolute() {
            path
        } else {
            absolute_base.join(path)
        };

        requests.push((full_path, lineno));
    }

    let unique_paths: Vec<Utf8PathBuf> = requests
        .iter()
        .map(|(p, _)| p.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let file_infos = load_files_parallel(unique_paths)?;
    let (files, path_to_alias) = assign_aliases(file_infos);
    let match_lines = build_match_lines(requests, &files, &path_to_alias);

    Ok((match_lines, files, format!("File: {}", list_path)))
}

/// Determines the absolute base directory.
/// If --working-directory is provided, it's resolved against CWD. If not, CWD is used.
fn get_absolute_base(args: &Args) -> Result<Utf8PathBuf> {
    let cwd = Utf8PathBuf::try_from(env::current_dir()?)?;

    if let Some(ref wd) = args.working_directory {
        if wd.is_absolute() {
            Ok(wd.clone())
        } else {
            Ok(cwd.join(wd))
        }
    } else {
        Ok(cwd)
    }
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
) -> Vec<MatchLine> {
    reqs.into_iter()
        .filter_map(|(path, lineno)| {
            let alias = path_map.get(&path)?;
            let file = files.get(alias)?;
            let line_content = file.original_content.lines().nth(lineno - 1)?;
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
    use camino_tempfile::tempdir;
    use clap::Parser;

    #[test]
    fn test_relative_path_resolution() {
        let dir = tempdir().unwrap();
        let wd = Utf8PathBuf::try_from(dir.path().to_path_buf()).unwrap();

        let target = wd.join("target.txt");
        fs::write(&target, "content").unwrap();

        let list_path = wd.join("list.txt");
        fs::write(&list_path, "target.txt:1").unwrap();

        let args = Args::parse_from(&["okapi", "-w", wd.as_str(), "--file", list_path.as_str()]);
        let (matches, files, _) = load_from_list(&list_path, &args).unwrap();

        let alias = matches[0].alias;
        let info = files.get(&alias).unwrap();

        // Ensure the path in the alias section is absolute
        assert!(info.full_path.is_absolute());
        assert!(info.full_path.ends_with("target.txt"));
    }
}
