use crate::{alias_iter, Args, FileAlias, FileInfo, MatchLine};
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashSet};
use std::fs;

pub fn load_from_list(
    list_path: &Utf8PathBuf,
    args: &Args,
) -> Result<(Vec<MatchLine>, BTreeMap<FileAlias, FileInfo>, String)> {
    let content = fs::read_to_string(list_path).context("reading list file")?;
    let mut requests = Vec::new();

    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        if let Some((path_str, line_str)) = line.rsplit_once(':') {
            let lineno = line_str.parse::<usize>().context("parsing line number")?;
            let mut path = Utf8PathBuf::from(path_str);
            if let Some(ref wd) = args.working_directory {
                path = wd.join(path);
            }
            requests.push((path, lineno));
        }
    }

    let unique_paths: Vec<Utf8PathBuf> = requests.iter().map(|(p, _)| p.clone())
        .collect::<HashSet<_>>().into_iter().collect();

    // Parallel processing of file contents
    let file_infos: Vec<FileInfo> = unique_paths.into_par_iter().map(|full_path| {
        let content = fs::read_to_string(&full_path)?;
        let metadata = fs::metadata(&full_path)?;
        let display_path = match &args.working_directory {
            Some(wd) => full_path.strip_prefix(wd).unwrap_or(&full_path).to_path_buf(),
            None => full_path.clone(),
        };
        Ok(FileInfo {
            path: display_path, full_path, alias: FileAlias::new(&['A']),
            original_content: content, original_mtime: metadata.modified()?,
        })
    }).collect::<Result<Vec<_>>>()?;

    let (files, path_to_alias) = assign_aliases(file_infos);
    let match_lines = build_match_lines(requests, &files, &path_to_alias);

    Ok((match_lines, files, format!("File: {}", list_path)))
}

fn assign_aliases(mut infos: Vec<FileInfo>) -> (BTreeMap<FileAlias, FileInfo>, BTreeMap<Utf8PathBuf, FileAlias>) {
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

fn build_match_lines(reqs: Vec<(Utf8PathBuf, usize)>, files: &BTreeMap<FileAlias, FileInfo>, path_map: &BTreeMap<Utf8PathBuf, FileAlias>) -> Vec<MatchLine> {
    reqs.into_iter().filter_map(|(path, lineno)| {
        let alias = path_map.get(&path)?;
        let file = files.get(alias)?;
        let line_content = file.original_content.lines().nth(lineno - 1)?;
        Some(MatchLine { alias: *alias, lineno, original_content: line_content.to_string() })
    }).collect()
}
