//! 发送端：把用户选中的文件和文件夹展开成清单。目录先于其中的内容，符号链接跳过。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use baton_core::files::{Entry, EntryKind, numbered};
use baton_core::{Error, Result};

/// 清单中的一项与它在本机的来源；目录没有来源。
pub struct Source {
    pub entry: Entry,
    pub path: Option<PathBuf>,
}

/// 选中的顶层条目重名时（来自不同文件夹）依次编号，接收方据首段区分它们。
pub fn collect(paths: &[PathBuf]) -> Result<Vec<Source>> {
    let mut out = Vec::new();
    let mut tops = HashSet::new();
    for path in paths {
        let Ok(meta) = fs::symlink_metadata(path) else {
            log::info!("跳过读不到的 {}", path.display());
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let kind = if meta.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        let top = (0..)
            .map(|n| numbered(&name, n, kind))
            .find(|candidate| tops.insert(candidate.clone()))
            .unwrap_or(name);
        if meta.is_dir() {
            out.push(Source {
                entry: entry(vec![top.clone()], EntryKind::Dir, 0),
                path: None,
            });
            walk(path, &[top], &mut out);
        } else if meta.is_file() {
            out.push(Source {
                entry: entry(vec![top], EntryKind::File, meta.len()),
                path: Some(path.clone()),
            });
        }
    }
    if out.is_empty() {
        return Err(Error::File("选中的内容里没有可发送的文件".into()));
    }
    Ok(out)
}

fn walk(dir: &Path, prefix: &[String], out: &mut Vec<Source>) {
    let mut children: Vec<(String, PathBuf)> = match fs::read_dir(dir) {
        Ok(read) => read
            .filter_map(|e| e.ok())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect(),
        Err(e) => {
            log::info!("跳过读不到的文件夹 {}：{e}", dir.display());
            return;
        }
    };
    children.sort();
    for (name, path) in children {
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let mut segments = prefix.to_vec();
        segments.push(name);
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            out.push(Source {
                entry: entry(segments.clone(), EntryKind::Dir, 0),
                path: None,
            });
            walk(&path, &segments, out);
        } else if meta.is_file() {
            out.push(Source {
                entry: entry(segments, EntryKind::File, meta.len()),
                path: Some(path),
            });
        }
    }
}

fn entry(path: Vec<String>, kind: EntryKind, size: u64) -> Entry {
    Entry { path, kind, size }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(sources: &[Source]) -> Vec<String> {
        sources.iter().map(|s| s.entry.path.join("/")).collect()
    }

    #[test]
    fn walks_folders_depth_first_with_empty_dirs() {
        let root = tempfile::tempdir().unwrap();
        let photos = root.path().join("照片");
        fs::create_dir_all(photos.join("空")).unwrap();
        fs::create_dir_all(photos.join("b")).unwrap();
        fs::write(photos.join("b").join("2.jpg"), b"22").unwrap();
        fs::write(photos.join("1.jpg"), b"1").unwrap();
        let note = root.path().join("note.txt");
        fs::write(&note, b"hello").unwrap();

        let sources = collect(&[photos, note]).unwrap();
        assert_eq!(
            paths(&sources),
            [
                "照片",
                "照片/1.jpg",
                "照片/b",
                "照片/b/2.jpg",
                "照片/空",
                "note.txt"
            ]
        );
        assert_eq!(sources[3].entry.size, 2);
        assert!(sources[0].path.is_none());
        assert!(sources[5].path.is_some());
    }

    #[test]
    fn same_named_selections_are_numbered() {
        let root = tempfile::tempdir().unwrap();
        for d in ["x", "y"] {
            fs::create_dir(root.path().join(d)).unwrap();
            fs::write(root.path().join(d).join("a.txt"), b"a").unwrap();
        }
        let sources = collect(&[root.path().join("x/a.txt"), root.path().join("y/a.txt")]).unwrap();
        assert_eq!(paths(&sources), ["a.txt", "a (1).txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("d");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("real.txt"), b"r").unwrap();
        std::os::unix::fs::symlink(dir.join("real.txt"), dir.join("link.txt")).unwrap();
        let sources = collect(&[dir]).unwrap();
        assert_eq!(paths(&sources), ["d", "d/real.txt"]);
    }

    #[test]
    fn nothing_readable_is_an_error() {
        assert!(collect(&[PathBuf::from("/definitely/not/here")]).is_err());
    }
}
