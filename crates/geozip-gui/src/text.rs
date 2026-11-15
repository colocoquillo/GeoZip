use std::path::Path;

use geozip_core::{Header, Lang, Level};
use time::{OffsetDateTime, UtcOffset};

#[derive(Clone, Copy)]
pub struct Tr(pub Lang);

impl Tr {
    pub fn t(self, es: &'static str, en: &'static str) -> &'static str {
        match self.0 {
            Lang::Es => es,
            Lang::En => en,
        }
    }
}

pub fn level_summary(header: &Header, tr: Tr) -> String {
    let name = match header.level() {
        Some(Level::Alto) => tr.t("Alto", "High"),
        Some(Level::Extremo | Level::ExtremoDoble) => tr.t("Extremo", "Extreme"),
        None => tr.t("Personalizado", "Custom"),
    };
    let places = format!("{} {}", header.places(), tr.t("lugares", "places"));
    match header.words_per_place {
        0 => format!("{name} · {places}"),
        1 => format!(
            "{name} · {places} · 1 {}",
            tr.t("palabra por lugar", "word per place")
        ),
        n => format!(
            "{name} · {places} · {n} {}",
            tr.t("palabras por lugar", "words per place")
        ),
    }
}

pub fn kind_of(name: &str, is_dir: bool, tr: Tr) -> String {
    if is_dir {
        return tr.t("Carpeta", "Folder").to_string();
    }
    match Path::new(name).extension() {
        Some(ext) => format!(
            "{} {}",
            tr.t("Archivo", "File"),
            ext.to_string_lossy().to_uppercase()
        ),
        None => tr.t("Archivo", "File").to_string(),
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub fn format_time(unix_seconds: i64, offset: UtcOffset) -> String {
    match OffsetDateTime::from_unix_timestamp(unix_seconds) {
        Ok(utc) => {
            let t = utc.to_offset(offset);
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute()
            )
        }
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn times() {
        assert_eq!(format_time(1_500_000_000, UtcOffset::UTC), "2017-07-14 02:40");
        let madrid_summer = UtcOffset::from_hms(2, 0, 0).unwrap();
        assert_eq!(format_time(1_500_000_000, madrid_summer), "2017-07-14 04:40");
    }

    #[test]
    fn kinds() {
        let tr = Tr(Lang::Es);
        assert_eq!(kind_of("fotos", true, tr), "Carpeta");
        assert_eq!(kind_of("a/b/nota.txt", false, tr), "Archivo TXT");
        assert_eq!(kind_of("LEEME", false, tr), "Archivo");
    }
}
