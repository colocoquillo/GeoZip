use std::cmp::Ordering;
use std::collections::BTreeMap;

use geozip_core::archive::EntryInfo;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortBy {
    Name,
    Size,
    Kind,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sort {
    pub by: SortBy,
    pub ascending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            by: SortBy::Name,
            ascending: true,
        }
    }
}

impl Sort {
    pub fn toggled(self, by: SortBy) -> Self {
        Self {
            by,
            ascending: if self.by == by { !self.ascending } else { true },
        }
    }
}

pub fn parent_folder(folder: &str) -> String {
    let trimmed = folder.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(slash) => trimmed[..=slash].to_string(),
        None => String::new(),
    }
}

pub fn list_folder(entries: &[EntryInfo], folder: &str) -> Vec<Row> {
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    for entry in entries {
        let Some(rest) = entry.name.strip_prefix(folder) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        match rest.split_once('/') {
            Some((child, deeper)) => {
                let row = rows.entry(child.to_string()).or_insert_with(|| Row {
                    name: child.to_string(),
                    path: format!("{folder}{child}/"),
                    is_dir: true,
                    size: 0,
                    modified: None,
                });
                row.size += entry.size;
                if deeper.is_empty() {
                    row.modified = entry.modified;
                }
            }
            None => {
                rows.insert(
                    rest.to_string(),
                    Row {
                        name: rest.to_string(),
                        path: entry.name.clone(),
                        is_dir: entry.is_dir,
                        size: entry.size,
                        modified: entry.modified,
                    },
                );
            }
        }
    }
    rows.into_values().collect()
}

pub fn find(entries: &[EntryInfo], query: &str) -> Vec<Row> {
    let wanted = query.trim().to_lowercase();
    entries
        .iter()
        .filter(|entry| {
            let last = entry
                .name
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default();
            last.to_lowercase().contains(&wanted)
        })
        .map(|entry| Row {
            name: entry.name.trim_end_matches('/').to_string(),
            path: entry.name.clone(),
            is_dir: entry.is_dir,
            size: entry.size,
            modified: entry.modified,
        })
        .collect()
}

fn extension(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, ext)| ext)
}

pub fn sort(rows: &mut [Row], sort: Sort) {
    rows.sort_by(|a, b| {
        let folders_first = b.is_dir.cmp(&a.is_dir);
        let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
        let chosen = match sort.by {
            SortBy::Name => by_name(),
            SortBy::Size => a.size.cmp(&b.size).then_with(by_name),
            SortBy::Kind => extension(&a.name)
                .to_lowercase()
                .cmp(&extension(&b.name).to_lowercase())
                .then_with(by_name),
            SortBy::Modified => a.modified.cmp(&b.modified).then_with(by_name),
        };
        let chosen = if sort.ascending { chosen } else { chosen.reverse() };
        if folders_first == Ordering::Equal {
            chosen
        } else {
            folders_first
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, size: u64) -> EntryInfo {
        EntryInfo {
            name: name.to_string(),
            size,
            is_dir: name.ends_with('/'),
            modified: Some(size as i64),
        }
    }

    fn sample() -> Vec<EntryInfo> {
        vec![
            entry("docs/", 0),
            entry("docs/hola.txt", 12),
            entry("docs/sub/", 0),
            entry("docs/sub/datos.bin", 300),
            entry("docs/sub/b.txt", 5),
            entry("suelto.pdf", 40),
            entry("sin-entrada-de-carpeta/foto.jpg", 7),
        ]
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn lists_one_level_and_invents_missing_folders() {
        let root = list_folder(&sample(), "");
        assert_eq!(names(&root), ["docs", "sin-entrada-de-carpeta", "suelto.pdf"]);
        assert!(root[0].is_dir && root[1].is_dir && !root[2].is_dir);
        assert_eq!(root[0].path, "docs/");
        assert_eq!(root[0].size, 317);
        assert_eq!(root[1].path, "sin-entrada-de-carpeta/");

        let docs = list_folder(&sample(), "docs/");
        assert_eq!(names(&docs), ["hola.txt", "sub"]);
        assert_eq!(docs[0].path, "docs/hola.txt");
        assert_eq!(docs[1].path, "docs/sub/");

        assert_eq!(
            names(&list_folder(&sample(), "docs/sub/")),
            ["b.txt", "datos.bin"]
        );
        assert!(list_folder(&sample(), "nada/").is_empty());
    }

    #[test]
    fn finds_by_file_name_anywhere_in_the_archive() {
        let found = find(&sample(), "DATOS");
        assert_eq!(names(&found), ["docs/sub/datos.bin"]);
        assert_eq!(found[0].path, "docs/sub/datos.bin");
        assert_eq!(names(&find(&sample(), "sub")), ["docs/sub"]);
        assert_eq!(names(&find(&sample(), "docs")), ["docs"]);
        assert!(find(&sample(), "zzz").is_empty());
    }

    #[test]
    fn parents() {
        assert_eq!(parent_folder("docs/sub/"), "docs/");
        assert_eq!(parent_folder("docs/"), "");
        assert_eq!(parent_folder(""), "");
    }

    #[test]
    fn sorting_keeps_folders_first() {
        let mut rows = list_folder(&sample(), "");
        sort(
            &mut rows,
            Sort {
                by: SortBy::Name,
                ascending: false,
            },
        );
        assert_eq!(names(&rows), ["sin-entrada-de-carpeta", "docs", "suelto.pdf"]);

        let mut sub = list_folder(&sample(), "docs/sub/");
        sort(&mut sub, Sort::default().toggled(SortBy::Size));
        assert_eq!(names(&sub), ["b.txt", "datos.bin"]);
        sort(
            &mut sub,
            Sort::default().toggled(SortBy::Size).toggled(SortBy::Size),
        );
        assert_eq!(names(&sub), ["datos.bin", "b.txt"]);
        sort(&mut sub, Sort::default().toggled(SortBy::Kind));
        assert_eq!(names(&sub), ["datos.bin", "b.txt"]);
    }
}
