use std::sync::mpsc::{self, Receiver};
use std::thread;

use eframe::egui::{self, Color32, RichText};
use geozip_core::grid::LatLon;
use geozip_core::{random_words, Lang, Level, Wordlist};

use super::{
    Action, App, Doc, Pending, PlaceEntry, ACTIVE_BLUE, BUTTON_SIZE, ERR_RED, OK_GREEN, SUGGESTIONS, WARN,
};
use crate::icons;
use crate::map::{Marker, Source};
use crate::search::{self, Hit};
use crate::text::Tr;

pub const COORDINATE_DECIMALS: usize = 4;
const COORDINATE_WIDTH: f32 = 76.0;
const WORD_WIDTH: f32 = 104.0;
const OFFLINE_HITS: usize = 6;

#[derive(Default)]
pub struct Search {
    query: String,
    pending: Option<Receiver<Result<Vec<Hit>, String>>>,
    online_hits: Vec<Hit>,
    error: Option<String>,
}

impl PlaceEntry {
    fn coordinates_edited(&mut self) {
        let pasted_pair =
            self.lat.contains(',') && self.lat.contains(|c: char| c == '.' || c.is_whitespace());
        if pasted_pair {
            if let Some((lat, lon)) = self.lat.clone().split_once(',') {
                self.lat = lat.trim().to_string();
                self.lon = lon.trim().to_string();
            }
        }
        for text in [&mut self.lat, &mut self.lon] {
            *text = limited_to_needed_decimals(text);
        }
        let decimal = |text: &str| text.trim().replace(',', ".");
        self.pos = LatLon::parse(&format!("{},{}", decimal(&self.lat), decimal(&self.lon))).ok();
    }

    fn has_invalid_coordinates(&self) -> bool {
        self.pos.is_none() && !self.lat.trim().is_empty() && !self.lon.trim().is_empty()
    }
}

fn limited_to_needed_decimals(text: &str) -> String {
    let text = text.trim();
    match text.find(['.', ',']) {
        Some(separator) => text.chars().take(separator + 1 + COORDINATE_DECIMALS).collect(),
        None => text.to_string(),
    }
}

pub fn rounded(pos: LatLon) -> LatLon {
    let scale = 10f64.powi(COORDINATE_DECIMALS as i32);
    LatLon {
        lat: (pos.lat * scale).round() / scale,
        lon: (pos.lon * scale).round() / scale,
    }
}

struct Scenario {
    attacker: &'static str,
    time: &'static str,
    safe: bool,
}

fn scenarios(words_per_place: usize, tr: Tr) -> [Scenario; 3] {
    let beyond = tr.t(
        "más que la edad del universo",
        "longer than the age of the universe",
    );
    let stranger = Scenario {
        attacker: tr.t("Un desconocido", "A stranger"),
        time: beyond,
        safe: true,
    };
    let acquaintance = tr.t("Alguien que conoce tu vida", "Someone who knows your life");
    let knows_places = tr.t(
        "Alguien que sabe tus 5 lugares exactos",
        "Someone who knows your 5 exact places",
    );
    let (acquaintance_time, knows_places_time) = match words_per_place {
        0 => (
            (
                tr.t("de semanas a millones de años", "from weeks to millions of years"),
                false,
            ),
            (tr.t("al instante", "instantly"), false),
        ),
        1 => (
            (beyond, true),
            (tr.t("unos 700.000 años", "about 700,000 years"), true),
        ),
        _ => ((beyond, true), (beyond, true)),
    };
    [
        stranger,
        Scenario {
            attacker: acquaintance,
            time: acquaintance_time.0,
            safe: acquaintance_time.1,
        },
        Scenario {
            attacker: knows_places,
            time: knows_places_time.0,
            safe: knows_places_time.1,
        },
    ]
}

fn strength(ui: &mut egui::Ui, tr: Tr, words_per_place: usize) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr.t("Cuánto tardarían en abrirlo", "How long it would take to open")).strong());
            icons::info(
                ui,
                tr.t(
                    "Estimaciones para un atacante con 1.000 ordenadores probando sin parar. Cada intento cuesta alrededor de un segundo por ordenador, y eso no se puede acelerar. Con una palabra por lugar equivale a una contraseña al azar de unos 13 caracteres; con solo lugares depende de lo previsibles que sean.",
                    "Estimates for an attacker with 1,000 computers trying non-stop. Each attempt costs about one second per computer, and that cannot be sped up. With one word per place it is comparable to a random password of about 13 characters; with places only it depends on how predictable they are.",
                ),
            );
        });
        for scenario in scenarios(words_per_place, tr) {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{}:", scenario.attacker));
                ui.colored_label(if scenario.safe { OK_GREEN } else { WARN }, scenario.time);
            });
        }
    });
}

fn word_field(ui: &mut egui::Ui, tr: Tr, lang: Lang, text: &mut String, slot: usize) -> Vec<&'static str> {
    let list = Wordlist::get();
    let resolved = list.lookup_any(text, lang).is_some();
    let suggestions: Vec<&'static str> = if resolved {
        Vec::new()
    } else {
        list.complete(text, lang, SUGGESTIONS)
            .into_iter()
            .map(|s| s.text)
            .collect()
    };
    let hint = format!("{} {}", tr.t("palabra", "word"), slot + 1);
    let color = if resolved {
        OK_GREEN
    } else if text.trim().is_empty() || !suggestions.is_empty() {
        ui.visuals().text_color()
    } else {
        ERR_RED
    };
    let edit = egui::TextEdit::singleline(text)
        .hint_text(hint)
        .desired_width(WORD_WIDTH)
        .text_color(color);
    let response = ui.add(edit);
    let pressed_enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if let (true, Some(first)) = (pressed_enter, suggestions.first()) {
        *text = first.to_string();
    }
    suggestions
}

fn status_label(ui: &mut egui::Ui, online: bool, tr: Tr) {
    let (color, text) = if online {
        (OK_GREEN, tr.t("Con conexión", "Online"))
    } else {
        (ERR_RED, tr.t("Sin conexión", "Offline"))
    };
    ui.label(RichText::new(text).color(color).strong());
}

pub fn connection_status(ui: &mut egui::Ui, online: bool, tr: Tr) {
    let help = if online {
        tr.t(
            "El mapa se descarga de internet. El clic se resuelve en tu equipo, pero el servidor del mapa ve qué zonas consultas (unos 2 km). El resto del programa no usa la red. No se guarda nada en disco.",
            "The map is downloaded from the internet. Clicks are resolved on your computer, but the map server sees which areas you look at (about 2 km). The rest of the program does not use the network. Nothing is stored on disk.",
        )
    } else {
        tr.t(
            "El programa no usa la red. Puedes buscar ciudades, pegar coordenadas o cargar un mapa detallado desde un archivo .pmtiles.",
            "The program does not use the network. You can search for cities, paste coordinates or load a detailed map from a .pmtiles file.",
        )
    };
    if ui.layout().prefer_right_to_left() {
        icons::info(ui, help);
        status_label(ui, online, tr);
        icons::connection(ui, online);
    } else {
        icons::connection(ui, online);
        status_label(ui, online, tr);
        icons::info(ui, help);
    }
}

impl App {
    pub(super) fn poll_search(&mut self) {
        let Some(receiver) = &self.search.pending else {
            return;
        };
        if let Ok(result) = receiver.try_recv() {
            self.search.pending = None;
            match result {
                Ok(hits) if hits.is_empty() => {
                    self.search.error = Some(self.tr().t("Sin resultados", "No results").to_string());
                }
                Ok(hits) => self.search.online_hits = hits,
                Err(error) => self.search.error = Some(error),
            }
        }
    }

    fn search_online(&mut self, ctx: &egui::Context) {
        let (sender, receiver) = mpsc::channel();
        let (query, lang, ctx) = (self.search.query.clone(), self.lang(), ctx.clone());
        thread::spawn(move || {
            let _ = sender.send(search::search_online(&query, lang));
            ctx.request_repaint();
        });
        self.search.pending = Some(receiver);
        self.search.online_hits.clear();
        self.search.error = None;
    }

    fn words_per_place(&self) -> usize {
        match (&self.pending, &self.doc) {
            (Some(Pending::Compress(form)), _) => form.level.map_or(0, Level::words_per_place),
            (_, Doc::Archive(opened)) => opened.header.words_per_place,
            _ => 0,
        }
    }

    pub(super) fn places_panel(&mut self, ui: &mut egui::Ui) {
        let tr = self.tr();
        let lang = self.lang();

        ui.horizontal(|ui| {
            ui.label(RichText::new(tr.t("Lugares", "Places")).strong().size(15.0));
            icons::info(
                ui,
                tr.t(
                    "Señala los lugares en orden: pulsa en el mapa o escribe latitud y longitud (también puedes pegar «latitud, longitud» en la primera casilla). Bastan 4 decimales (unos 11 m): cada lugar se ajusta a una celda de 300 m, así que un error de unas decenas de metros no importa. Las palabras las elige el programa; memorízalas imaginándolas en su lugar.",
                    "Mark the places in order: click on the map or type latitude and longitude (you can also paste “latitude, longitude” into the first box). Four decimals (about 11 m) are enough: each place snaps to a 300 m cell, so being a few dozen metres off does not matter. The words are chosen by the program; memorise them by picturing them at their place.",
                ),
            );
        });
        if let Some(error) = &self.places_error {
            ui.colored_label(ERR_RED, error);
        }
        ui.add_space(2.0);

        let mut go_to = None;
        for index in 0..self.places.len() {
            let active = self.active == index;
            let entry = &mut self.places[index];
            let stroke = if active {
                egui::Stroke::new(2.0, ACTIVE_BLUE)
            } else {
                ui.style().visuals.widgets.noninteractive.bg_stroke
            };
            egui::Frame::group(ui.style()).stroke(stroke).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut suggestions: Vec<(usize, Vec<&'static str>)> = Vec::new();
                ui.horizontal(|ui| {
                    let number = RichText::new((index + 1).to_string()).strong();
                    if ui.selectable_label(active, number).clicked() {
                        self.active = index;
                        go_to = entry.pos;
                    }
                    let mut edited = false;
                    for (text, hint) in [
                        (&mut entry.lat, tr.t("latitud", "latitude")),
                        (&mut entry.lon, tr.t("longitud", "longitude")),
                    ] {
                        let edit = egui::TextEdit::singleline(text)
                            .hint_text(hint)
                            .desired_width(COORDINATE_WIDTH);
                        let response = ui.add(edit);
                        if response.gained_focus() {
                            self.active = index;
                        }
                        edited |= response.changed();
                    }
                    if edited {
                        entry.coordinates_edited();
                        go_to = entry.pos.or(go_to);
                    }

                    if !entry.generated.is_empty() {
                        let words: Vec<&str> = entry
                            .generated
                            .iter()
                            .map(|&id| Wordlist::get().word(id, lang))
                            .collect();
                        ui.label(RichText::new(words.join(" + ")).size(15.0).strong());
                        let label = format!("{} ({})", tr.t("Otra", "Other"), entry.rerolls_left);
                        if ui
                            .add_enabled(entry.rerolls_left > 0, egui::Button::new(label).small())
                            .clicked()
                        {
                            if let Ok(words) = random_words(entry.generated.len()) {
                                entry.generated = words;
                                entry.rerolls_left -= 1;
                            }
                        }
                    }
                    for (slot, typed) in entry.typed.iter_mut().enumerate() {
                        let found = word_field(ui, tr, lang, typed, slot);
                        if !found.is_empty() {
                            suggestions.push((slot, found));
                        }
                    }
                });
                if entry.has_invalid_coordinates() {
                    ui.colored_label(ERR_RED, tr.t("Coordenadas no válidas", "Invalid coordinates"));
                }
                for (slot, found) in suggestions {
                    ui.horizontal_wrapped(|ui| {
                        for word in found {
                            if ui.small_button(word).clicked() {
                                entry.typed[slot] = word.to_string();
                            }
                        }
                    });
                }
            });
        }
        if let Some(pos) = go_to {
            self.map.go_to(pos, 15.0);
        }

        ui.add_space(6.0);
        if self.places.len() == 5 {
            strength(ui, tr, self.words_per_place());
            ui.add_space(6.0);
        }

        let ready = self.secret().is_some();
        ui.horizontal(|ui| {
            let accept = match &self.pending {
                Some(Pending::Extract(form)) if form.scope.verify_only => tr.t("Comprobar", "Test"),
                Some(Pending::Extract(_)) => tr.t("Extraer", "Extract"),
                Some(Pending::Unlock) => tr.t("Ver contenido", "Show contents"),
                Some(Pending::Edit(_)) => tr.t("Aceptar", "OK"),
                Some(Pending::Compress(form)) if form.convert.is_some() => tr.t("Convertir", "Convert"),
                _ => tr.t("Comprimir", "Compress"),
            };
            if ui
                .add_enabled(ready, egui::Button::new(accept).min_size(BUTTON_SIZE))
                .clicked()
            {
                let ctx = ui.ctx().clone();
                self.run(&ctx);
            }
            if ui
                .add(egui::Button::new(tr.t("Cancelar", "Cancel")).min_size(BUTTON_SIZE))
                .clicked()
            {
                self.leave_places();
            }
        });
    }

    fn search_bar(&mut self, ui: &mut egui::Ui) {
        let tr = self.tr();
        let online = self.map.is_online();
        let mut chosen: Option<(LatLon, f64)> = None;
        let mut ask_online = false;

        ui.horizontal(|ui| {
            let edit = egui::TextEdit::singleline(&mut self.search.query)
                .hint_text(tr.t("Buscar ciudad o pueblo", "Search for a city or town"))
                .desired_width(240.0);
            let response = ui.add(edit);
            if response.changed() {
                self.search.online_hits.clear();
                self.search.error = None;
            }
            let has_query = self.search.query.trim().chars().count() >= 3;
            let searching = self.search.pending.is_some();
            let button = egui::Button::new(tr.t("Buscar calles en línea", "Search streets online"));
            if ui.add_enabled(online && has_query && !searching, button).clicked() {
                ask_online = true;
            }
            icons::info(
                ui,
                tr.t(
                    "Mientras escribes se buscan ciudades y pueblos en una lista incluida en el programa, sin usar la red. El botón envía el texto a un buscador de internet (Photon, datos de OpenStreetMap) para encontrar calles y sitios concretos: ese servidor ve lo que buscas. No está disponible sin conexión.",
                    "As you type, cities and towns are looked up in a list bundled with the program, without using the network. The button sends the text to an internet search service (Photon, OpenStreetMap data) to find streets and specific spots: that server sees what you search for. Not available offline.",
                ),
            );
            if searching {
                ui.spinner();
            }
            if let Some(error) = &self.search.error {
                ui.colored_label(WARN, error);
            }
        });

        let offline_hits = match search::gazetteer() {
            Some(gazetteer) if self.search.online_hits.is_empty() => {
                gazetteer.search(&self.search.query, OFFLINE_HITS)
            }
            _ => Vec::new(),
        };
        let hits = if self.search.online_hits.is_empty() {
            &offline_hits
        } else {
            &self.search.online_hits
        };
        if !hits.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for hit in hits {
                    let label = if hit.detail.is_empty() {
                        hit.title.clone()
                    } else {
                        format!("{} · {}", hit.title, hit.detail)
                    };
                    if ui.button(label).clicked() {
                        chosen = Some((hit.pos, hit.zoom));
                    }
                }
            });
        }

        if ask_online {
            let ctx = ui.ctx().clone();
            self.search_online(&ctx);
        }
        if let Some((pos, zoom)) = chosen {
            self.map.go_to(pos, zoom);
            self.search = Search::default();
        }
    }

    pub(super) fn places_map(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let tr = self.tr();
        let mut action = None;
        ui.horizontal(|ui| {
            connection_status(ui, self.map.is_online(), tr);
            match self.map.source() {
                Source::Online => {
                    if ui.button(tr.t("Trabajar sin conexión", "Work offline")).clicked() {
                        action = Some(Action::MapOfflinePlain);
                    }
                }
                Source::OfflinePlain | Source::OfflineFile(_) => {
                    if let Some(file) = self.map.offline_file() {
                        ui.label(file.file_name().unwrap_or_default().to_string_lossy());
                    }
                    if ui
                        .button(tr.t("Cargar mapa .pmtiles…", "Load .pmtiles map…"))
                        .clicked()
                    {
                        action = Some(Action::MapOfflineFile);
                    }
                    if ui.button(tr.t("Usar mapa en línea", "Use online map")).clicked() {
                        action = Some(Action::MapOnline);
                    }
                    if ui
                        .link(tr.t("¿Dónde consigo un mapa?", "Where do I get a map?"))
                        .clicked()
                    {
                        action = Some(Action::OfflineMapsHelp);
                    }
                }
            }
            if !self.map.can_pick() {
                ui.colored_label(WARN, tr.t("Acerca el mapa para elegir", "Zoom in to pick"));
            }
        });
        self.search_bar(ui);

        let markers: Vec<Marker> = self
            .places
            .iter()
            .enumerate()
            .filter_map(|(index, place)| {
                place.pos.map(|pos| Marker {
                    pos,
                    shift: self.shift_for(index, pos),
                    label: (index + 1).to_string(),
                    active: index == self.active,
                })
            })
            .collect();
        let fixed_shift = match &self.doc {
            Doc::Archive(opened) if self.asking_for_an_existing_archive() => {
                opened.header.shifts.get(self.active).copied()
            }
            _ => None,
        };
        let attribution = self.map.attribution();
        let map_rect = ui.available_rect_before_wrap();
        if let Some(pos) = self.map.show(ui, &markers, fixed_shift) {
            let active = self.active;
            self.set_place(active, pos);
        }
        ui.painter().text(
            map_rect.right_bottom() - egui::vec2(6.0, 4.0),
            egui::Align2::RIGHT_BOTTOM,
            attribution,
            egui::FontId::proportional(10.0),
            Color32::from_gray(60),
        );
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_keep_only_the_decimals_that_matter() {
        assert_eq!(limited_to_needed_decimals("40.416775"), "40.4167");
        assert_eq!(limited_to_needed_decimals(" -3,703790 "), "-3,7037");
        assert_eq!(limited_to_needed_decimals("40.41"), "40.41");
        assert_eq!(limited_to_needed_decimals("40"), "40");
    }

    #[test]
    fn pasted_pair_is_split_and_trimmed() {
        let mut entry = PlaceEntry {
            lat: "40.416775, -3.703790".to_string(),
            ..Default::default()
        };
        entry.coordinates_edited();
        assert_eq!((entry.lat.as_str(), entry.lon.as_str()), ("40.4167", "-3.7037"));
        assert_eq!(
            entry.pos,
            Some(LatLon {
                lat: 40.4167,
                lon: -3.7037
            })
        );
    }

    #[test]
    fn clicked_positions_are_rounded_to_what_is_shown() {
        let pos = rounded(LatLon {
            lat: 41.900_949,
            lon: 12.483_351,
        });
        assert_eq!(
            pos,
            LatLon {
                lat: 41.9009,
                lon: 12.4834
            }
        );
    }
}
