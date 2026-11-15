use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind};
use geozip_core::grid::{self, LatLon, Shift};
use geozip_core::Lang;
use walkers::sources::{Attribution, TileSource};
use walkers::{lat_lon, HttpOptions, HttpTiles, Map, MapMemory, PmTiles, Projector, Style, TileId, Tiles};

use crate::search;

pub const MIN_PICK_ZOOM: f64 = 12.0;

const PLACED: Color32 = Color32::from_rgb(214, 48, 49);
const ACTIVE: Color32 = Color32::from_rgb(9, 132, 227);
const PLAIN_BACKGROUND: Color32 = Color32::from_rgb(236, 240, 244);
const PLAIN_LABEL: Color32 = Color32::from_rgb(60, 70, 85);
const PLAIN_LABEL_LIMIT: usize = 70;
const DEFAULT_TILES_URL: &str = "https://tiles.openfreemap.org/planet/20261101_080013_pt/{z}/{x}/{y}.pbf";
const ONLINE_STYLE: &str = include_str!("../../../assets/map-style-online.json");
const OFFLINE_STYLE: &str = include_str!("../../../assets/map-style-offline.json");

struct OnlineVectorTiles {
    url_template: String,
}

impl OnlineVectorTiles {
    fn from_env() -> Self {
        Self {
            url_template: std::env::var("GEOZIP_TILES_URL").unwrap_or_else(|_| DEFAULT_TILES_URL.to_string()),
        }
    }
}

impl TileSource for OnlineVectorTiles {
    fn tile_url(&self, tile: TileId) -> String {
        self.url_template
            .replace("{z}", &tile.zoom.to_string())
            .replace("{x}", &tile.x.to_string())
            .replace("{y}", &tile.y.to_string())
    }

    fn attribution(&self) -> Attribution {
        Attribution {
            text: "OpenFreeMap © OpenMapTiles · OpenStreetMap",
            url: "https://openfreemap.org",
            logo_light: None,
            logo_dark: None,
        }
    }

    fn tile_size(&self) -> u32 {
        512
    }

    fn max_zoom(&self) -> u8 {
        14
    }
}

fn with_labels_in(style_json: &str, name_fields: &[String]) -> serde_json::Value {
    let mut style: serde_json::Value =
        serde_json::from_str(style_json).expect("bundled map style is valid JSON");
    let mut label = vec![serde_json::json!("coalesce")];
    label.extend(name_fields.iter().map(|field| serde_json::json!(["get", field])));
    let label = serde_json::Value::Array(label);
    let layers = style.get_mut("layers").and_then(|l| l.as_array_mut());
    for layer in layers.into_iter().flatten() {
        let Some(text_field) = layer.pointer_mut("/layout/text-field") else {
            continue;
        };
        let chooses_a_name = matches!(
            text_field.get(0).and_then(|op| op.as_str()),
            Some("case" | "step" | "format")
        );
        if chooses_a_name {
            *text_field = label.clone();
        }
    }
    style
}

fn style_with_labels_in(style_json: &str, name_fields: &[String]) -> Style {
    serde_json::from_value(with_labels_in(style_json, name_fields))
        .expect("bundled map style matches the renderer")
}

fn online_style(lang: Lang) -> Style {
    let fields = [
        format!("name:{}", lang.code()),
        "name:latin".into(),
        "name:en".into(),
        "name_en".into(),
    ];
    style_with_labels_in(ONLINE_STYLE, &fields)
}

fn offline_style(lang: Lang) -> Style {
    let fields = [format!("name:{}", lang.code()), "name:en".into(), "name".into()];
    style_with_labels_in(OFFLINE_STYLE, &fields)
}

pub struct Marker {
    pub pos: LatLon,
    pub shift: Shift,
    pub label: String,
    pub active: bool,
}

#[derive(Clone, PartialEq)]
pub enum Source {
    Online,
    OfflinePlain,
    OfflineFile(PathBuf),
}

pub struct MapView {
    tiles: Option<Box<dyn Tiles>>,
    source: Source,
    lang: Lang,
    memory: MapMemory,
    home: walkers::Position,
}

impl MapView {
    pub fn new(ctx: &egui::Context, lang: Lang) -> Self {
        let mut memory = MapMemory::default();
        let _ = memory.set_zoom(2.5);
        let mut view = Self {
            tiles: None,
            source: Source::Online,
            lang,
            memory,
            home: lat_lon(25.0, 5.0),
        };
        view.set_source(Source::Online, ctx);
        view
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    pub fn is_online(&self) -> bool {
        self.source == Source::Online
    }

    pub fn offline_file(&self) -> Option<&Path> {
        match &self.source {
            Source::OfflineFile(path) => Some(path),
            _ => None,
        }
    }

    pub fn set_language(&mut self, lang: Lang, ctx: &egui::Context) {
        if self.lang != lang {
            self.lang = lang;
            self.set_source(self.source.clone(), ctx);
        }
    }

    pub fn set_source(&mut self, source: Source, ctx: &egui::Context) {
        self.tiles = match &source {
            Source::Online => {
                let without_disk_cache = HttpOptions::default();
                Some(Box::new(HttpTiles::with_options_and_style(
                    OnlineVectorTiles::from_env(),
                    without_disk_cache,
                    online_style(self.lang),
                    ctx.clone(),
                )))
            }
            Source::OfflinePlain => None,
            Source::OfflineFile(path) => Some(Box::new(PmTiles::with_style(
                path,
                offline_style(self.lang),
                ctx.clone(),
            ))),
        };
        self.source = source;
    }

    pub fn go_to(&mut self, pos: LatLon, min_zoom: f64) {
        self.memory.center_at(lat_lon(pos.lat, pos.lon));
        if self.memory.zoom() < min_zoom {
            let _ = self.memory.set_zoom(min_zoom);
        }
    }

    pub fn can_pick(&self) -> bool {
        self.memory.zoom() >= MIN_PICK_ZOOM
    }

    pub fn attribution(&self) -> &'static str {
        match &self.tiles {
            Some(tiles) => tiles.attribution().text,
            None => "GeoNames",
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        markers: &[Marker],
        fixed_shift: Option<Shift>,
    ) -> Option<LatLon> {
        let can_pick = self.can_pick();
        let zoom = self.memory.zoom();
        let plain = self.tiles.is_none();
        if plain {
            ui.painter()
                .rect_filled(ui.available_rect_before_wrap(), 0.0, PLAIN_BACKGROUND);
        }
        let tiles = self.tiles.as_mut().map(|t| t.as_mut() as &mut dyn Tiles);
        let map = Map::new(tiles, &mut self.memory, self.home)
            .zoom_with_ctrl(false)
            .double_click_to_zoom(false);

        map.show(ui, |ui, response, projector, _memory| {
            let painter = ui.painter().with_clip_rect(response.rect);
            let to_latlon = |pos: Pos2| {
                let p = projector.unproject(pos.to_vec2());
                LatLon::new(p.y(), p.x()).ok()
            };

            if plain {
                paint_plain_places(&painter, response.rect, projector, zoom);
            }

            let hovered = response.hover_pos().and_then(to_latlon);
            if let (true, Some(pos)) = (can_pick, hovered) {
                let shift = fixed_shift.unwrap_or_else(|| grid::best_shift(pos));
                let rect = cell_rect(projector, pos, shift);
                painter.rect_filled(rect, 0.0, ACTIVE.gamma_multiply(0.15));
                painter.rect_stroke(rect, 0.0, Stroke::new(1.5, ACTIVE), StrokeKind::Middle);
            }

            for marker in markers {
                let color = if marker.active { ACTIVE } else { PLACED };
                let rect = cell_rect(projector, marker.pos, marker.shift);
                painter.rect_filled(rect, 0.0, color.gamma_multiply(0.25));
                painter.rect_stroke(rect, 0.0, Stroke::new(2.0, color), StrokeKind::Middle);
                let center = rect.center();
                let pin = center - egui::vec2(0.0, rect.height().max(8.0) / 2.0 + 14.0);
                painter.line_segment([center, pin], Stroke::new(2.0, color));
                painter.circle_filled(pin, 11.0, color);
                painter.circle_stroke(pin, 11.0, Stroke::new(1.5, Color32::WHITE));
                painter.text(
                    pin,
                    Align2::CENTER_CENTER,
                    &marker.label,
                    FontId::proportional(13.0),
                    Color32::WHITE,
                );
            }

            if response.clicked() && can_pick {
                response.interact_pointer_pos().and_then(to_latlon)
            } else {
                None
            }
        })
        .inner
    }
}

fn minimum_population_shown(zoom: f64) -> u32 {
    match zoom {
        z if z < 4.0 => 2_000_000,
        z if z < 6.0 => 500_000,
        z if z < 8.0 => 100_000,
        z if z < 10.0 => 20_000,
        z if z < 12.0 => 3_000,
        _ => 0,
    }
}

fn paint_plain_places(painter: &egui::Painter, view: Rect, projector: &Projector, zoom: f64) {
    let Some(gazetteer) = search::gazetteer() else {
        return;
    };
    let minimum = minimum_population_shown(zoom);
    let font = FontId::proportional(12.0);
    let mut occupied: Vec<Rect> = Vec::new();
    for place in gazetteer.by_population() {
        if place.population < minimum || occupied.len() >= PLAIN_LABEL_LIMIT {
            break;
        }
        let dot = projector.project(lat_lon(place.pos.lat, place.pos.lon)).to_pos2();
        if !view.contains(dot) {
            continue;
        }
        let galley = painter.layout_no_wrap(place.name.clone(), font.clone(), PLAIN_LABEL);
        let label = Rect::from_min_size(dot + egui::vec2(6.0, -galley.size().y / 2.0), galley.size());
        let footprint = label
            .union(Rect::from_center_size(dot, egui::vec2(8.0, 8.0)))
            .expand(3.0);
        if occupied.iter().any(|taken| taken.intersects(footprint)) {
            continue;
        }
        occupied.push(footprint);
        painter.circle_filled(dot, 3.0, PLAIN_LABEL);
        painter.galley(label.min, galley, PLAIN_LABEL);
    }
}

fn cell_rect(projector: &Projector, pos: LatLon, shift: Shift) -> Rect {
    let (south_west, north_east) = grid::cell_bounds(grid::quantize(pos, shift), shift);
    let a = projector
        .project(lat_lon(south_west.lat, south_west.lon))
        .to_pos2();
    let b = projector
        .project(lat_lon(north_east.lat, north_east.lon))
        .to_pos2();
    Rect::from_two_pos(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_styles_load_in_every_language() {
        for lang in Lang::ALL {
            online_style(lang);
            offline_style(lang);
        }
    }

    #[test]
    fn labels_use_one_name_instead_of_mixing_scripts() {
        assert!(ONLINE_STYLE.contains("name:nonlatin"));
        let fields = ["name:es".to_string(), "name:latin".to_string()];
        let rewritten = with_labels_in(ONLINE_STYLE, &fields).to_string();
        assert!(!rewritten.contains("name:nonlatin"));
        assert!(rewritten.contains(r#"["coalesce",["get","name:es"],["get","name:latin"]]"#));
    }
}
