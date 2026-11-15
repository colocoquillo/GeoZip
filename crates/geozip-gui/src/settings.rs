use std::fs;
use std::path::PathBuf;

use geozip_core::Lang;

use crate::map::Source;

const RECENT_LIMIT: usize = 8;

pub struct Settings {
    pub lang: Lang,
    pub map: Source,
    pub recent: Vec<PathBuf>,
}

fn system_language() -> Lang {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|variable| std::env::var(variable).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    Lang::from_code(locale.get(..2).unwrap_or_default()).unwrap_or(Lang::En)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lang: system_language(),
            map: Source::Online,
            recent: Vec::new(),
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    let from = |variable: &str| {
        std::env::var_os(variable)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(target_os = "windows") {
        from("APPDATA")
    } else if cfg!(target_os = "macos") {
        from("HOME").map(|home| home.join("Library/Application Support"))
    } else {
        from("XDG_CONFIG_HOME").or_else(|| from("HOME").map(|home| home.join(".config")))
    };
    base.map(|dir| dir.join("geozip"))
}

fn file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("settings"))
}

impl Settings {
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "lang" => settings.lang = Lang::from_code(value).unwrap_or(settings.lang),
                "map" => {
                    settings.map = match value {
                        "online" => Source::Online,
                        "offline" => Source::OfflinePlain,
                        other => match other.strip_prefix("file:") {
                            Some(path) => Source::OfflineFile(PathBuf::from(path)),
                            None => Source::Online,
                        },
                    }
                }
                "recent" if settings.recent.len() < RECENT_LIMIT => {
                    settings.recent.push(PathBuf::from(value))
                }
                _ => {}
            }
        }
        settings
    }

    pub fn serialize(&self) -> String {
        let map = match &self.map {
            Source::Online => "online".to_string(),
            Source::OfflinePlain => "offline".to_string(),
            Source::OfflineFile(path) => format!("file:{}", path.display()),
        };
        let mut text = format!("lang={}\nmap={map}\n", self.lang.code());
        for path in &self.recent {
            text.push_str(&format!("recent={}\n", path.display()));
        }
        text
    }

    pub fn load() -> Self {
        let mut settings = file()
            .and_then(|f| fs::read_to_string(f).ok())
            .map(|t| Self::parse(&t))
            .unwrap_or_default();
        if matches!(&settings.map, Source::OfflineFile(path) if !path.is_file()) {
            settings.map = Source::OfflinePlain;
        }
        settings.recent.retain(|path| path.is_file());
        settings
    }

    pub fn save(&self) {
        let Some(file) = file() else { return };
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(file, self.serialize());
    }

    pub fn remember(&mut self, path: &std::path::Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(RECENT_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tolerance() {
        let mut settings = Settings {
            lang: Lang::En,
            map: Source::OfflineFile(PathBuf::from("/mapas/madrid.pmtiles")),
            recent: Vec::new(),
        };
        for i in 0..10 {
            settings.remember(std::path::Path::new(&format!("/tmp/{i}.zip")));
        }
        settings.remember(std::path::Path::new("/tmp/5.zip"));
        assert_eq!(settings.recent.len(), RECENT_LIMIT);
        assert_eq!(settings.recent[0], PathBuf::from("/tmp/5.zip"));
        assert_eq!(settings.recent.iter().filter(|p| p.ends_with("5.zip")).count(), 1);

        let parsed = Settings::parse(&settings.serialize());
        assert_eq!(parsed.lang, Lang::En);
        assert!(parsed.map == settings.map);
        assert_eq!(parsed.recent, settings.recent);

        let odd = Settings::parse("basura\nlang=xx\nmap=???\nrecent=/a.zip\n");
        assert_eq!(odd.lang, system_language());
        assert!(odd.map == Source::Online);
        assert_eq!(odd.recent, [PathBuf::from("/a.zip")]);
    }
}
