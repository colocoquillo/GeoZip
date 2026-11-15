use std::collections::HashMap;
use std::sync::OnceLock;

use unicode_normalization::UnicodeNormalization;

pub const WORD_COUNT: usize = 1850;

const TSV: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../wordlist/gz1-concepts.tsv"
));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Es,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Es, Lang::En];

    pub fn code(self) -> &'static str {
        match self {
            Lang::Es => "es",
            Lang::En => "en",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.code() == code)
    }

    fn slot(self) -> usize {
        match self {
            Lang::Es => 0,
            Lang::En => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub id: u16,
    pub text: &'static str,
    pub main: &'static str,
}

struct PerLang {
    main: Vec<&'static str>,
    index: HashMap<String, u16>,
    sorted: Vec<(String, &'static str, u16)>,
}

pub struct Wordlist {
    langs: [PerLang; 2],
}

pub fn normalize(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .nfd()
        .filter(|c| c.is_ascii_lowercase())
        .collect()
}

impl Wordlist {
    pub fn get() -> &'static Wordlist {
        static LIST: OnceLock<Wordlist> = OnceLock::new();
        LIST.get_or_init(|| Wordlist::parse(TSV).expect("lista de palabras embebida no válida"))
    }

    fn parse(tsv: &'static str) -> Result<Wordlist, String> {
        let mut langs = [Lang::Es, Lang::En].map(|_| PerLang {
            main: Vec::with_capacity(WORD_COUNT),
            index: HashMap::new(),
            sorted: Vec::new(),
        });
        for (row, line) in tsv.lines().skip(1).filter(|l| !l.is_empty()).enumerate() {
            let cols: Vec<&'static str> = line.split('\t').collect();
            if cols.len() != 5 || cols[0].parse::<usize>() != Ok(row) {
                return Err(format!("fila {row} mal formada: {line:?}"));
            }
            let id = row as u16;
            for (lang, main, aliases) in [(Lang::En, cols[1], cols[3]), (Lang::Es, cols[2], cols[4])] {
                let per = &mut langs[lang.slot()];
                per.main.push(main);
                let words = std::iter::once(main).chain(aliases.split('|').filter(|a| !a.is_empty()));
                for word in words {
                    let norm = normalize(word);
                    if norm.is_empty() {
                        return Err(format!("palabra vacía en la fila {row}"));
                    }
                    if per.index.insert(norm.clone(), id).is_some() {
                        return Err(format!("palabra repetida en {}: {word}", lang.code()));
                    }
                    per.sorted.push((norm, word, id));
                }
            }
        }
        if langs[0].main.len() != WORD_COUNT {
            return Err(format!(
                "hay {} conceptos, se esperaban {WORD_COUNT}",
                langs[0].main.len()
            ));
        }
        for per in &mut langs {
            per.sorted.sort();
        }
        Ok(Wordlist { langs })
    }

    pub fn word(&self, id: u16, lang: Lang) -> &'static str {
        self.langs[lang.slot()].main[id as usize]
    }

    pub fn lookup(&self, text: &str, lang: Lang) -> Option<u16> {
        self.langs[lang.slot()].index.get(&normalize(text)).copied()
    }

    pub fn lookup_any(&self, text: &str, prefer: Lang) -> Option<u16> {
        self.lookup(text, prefer)
            .or_else(|| Lang::ALL.into_iter().find_map(|l| self.lookup(text, l)))
    }

    pub fn complete(&self, prefix: &str, lang: Lang, limit: usize) -> Vec<Suggestion> {
        let prefix = normalize(prefix);
        if prefix.is_empty() {
            return Vec::new();
        }
        let per = &self.langs[lang.slot()];
        let start = per
            .sorted
            .partition_point(|(norm, _, _)| norm.as_str() < prefix.as_str());
        per.sorted[start..]
            .iter()
            .take_while(|(norm, _, _)| norm.starts_with(&prefix))
            .take(limit)
            .map(|&(_, text, id)| Suggestion {
                id,
                text,
                main: per.main[id as usize],
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        assert_eq!(normalize("  Melocotón "), "melocoton");
        assert_eq!(normalize("PINGÜINO"), "pinguino");
        assert_eq!(normalize("piña"), "pina");
    }

    #[test]
    fn list_is_well_formed() {
        let list = Wordlist::get();
        for lang in Lang::ALL {
            for id in 0..WORD_COUNT as u16 {
                let word = list.word(id, lang);
                assert_eq!(list.lookup(word, lang), Some(id), "{word}");
                let norm = normalize(word);
                assert!((3..=14).contains(&norm.len()), "{word}");
            }
        }
    }

    #[test]
    fn no_cross_language_conflicts() {
        let list = Wordlist::get();
        let [es, en] = &list.langs;
        for (norm, id) in &es.index {
            if let Some(other) = en.index.get(norm) {
                assert_eq!(id, other, "«{norm}» significa cosas distintas en es y en");
            }
        }
    }

    #[test]
    fn completion_is_prefix_based_and_bounded() {
        let list = Wordlist::get();
        let word = list.word(1000, Lang::Es);
        let prefix: String = normalize(word).chars().take(3).collect();
        let found = list.complete(&prefix, Lang::Es, 500);
        assert!(found.iter().any(|s| s.id == 1000));
        assert!(found.iter().all(|s| normalize(s.text).starts_with(&prefix)));
        assert!(list.complete(&prefix, Lang::Es, 1).len() == 1);
        assert!(list.complete("", Lang::Es, 10).is_empty());
    }
}
