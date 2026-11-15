use std::io::Read;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use flate2::read::GzDecoder;
use geozip_core::grid::LatLon;
use geozip_core::wordlist::normalize;
use geozip_core::Lang;

const PLACES_GZ: &[u8] = include_bytes!("../../../assets/places.tsv.gz");
const ONLINE_ENDPOINT: &str = "https://photon.komoot.io/api/";
const ONLINE_LIMIT: usize = 8;
const MIN_QUERY_LEN: usize = 2;

pub struct Place {
    pub name: String,
    pub country: String,
    pub pos: LatLon,
    pub population: u32,
    name_key: String,
    alternate_keys: Vec<String>,
}

pub struct Hit {
    pub title: String,
    pub detail: String,
    pub pos: LatLon,
    pub zoom: f64,
}

pub struct Gazetteer {
    places: Vec<Place>,
}

static GAZETTEER: OnceLock<Gazetteer> = OnceLock::new();

pub fn load_in_background() {
    thread::spawn(|| {
        GAZETTEER.get_or_init(|| Gazetteer::parse(PLACES_GZ));
    });
}

pub fn gazetteer() -> Option<&'static Gazetteer> {
    GAZETTEER.get()
}

fn parse_place(line: &str) -> Option<Place> {
    let mut fields = line.split('\t');
    let name = fields.next()?;
    let country = fields.next()?;
    let lat = fields.next()?.parse().ok()?;
    let lon = fields.next()?.parse().ok()?;
    let population = fields.next()?.parse().ok()?;
    let alternates = fields.next().unwrap_or("");
    Some(Place {
        name: name.to_string(),
        country: country.to_string(),
        pos: LatLon { lat, lon },
        population,
        name_key: normalize(name),
        alternate_keys: alternates
            .split('|')
            .filter(|a| !a.is_empty())
            .map(normalize)
            .collect(),
    })
}

impl Gazetteer {
    fn parse(gzipped: &[u8]) -> Self {
        let mut text = String::new();
        let _ = GzDecoder::new(gzipped).read_to_string(&mut text);
        Self {
            places: text.lines().filter_map(parse_place).collect(),
        }
    }

    pub fn by_population(&self) -> &[Place] {
        &self.places
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let key = normalize(query);
        if key.len() < MIN_QUERY_LEN {
            return Vec::new();
        }
        let by_relevance: [&dyn Fn(&Place) -> bool; 3] = [
            &|p| p.name_key == key || p.alternate_keys.iter().any(|a| *a == key),
            &|p| p.name_key.starts_with(&key),
            &|p| p.alternate_keys.iter().any(|a| a.starts_with(&key)),
        ];
        let mut found: Vec<&Place> = Vec::new();
        for matches in by_relevance {
            for place in self.places.iter().filter(|p| matches(p)) {
                if found.len() >= limit {
                    break;
                }
                if !found.iter().any(|f| std::ptr::eq(*f, place)) {
                    found.push(place);
                }
            }
        }
        found.into_iter().map(Place::to_hit).collect()
    }
}

impl Place {
    fn to_hit(&self) -> Hit {
        Hit {
            title: self.name.clone(),
            detail: self.country.clone(),
            pos: self.pos,
            zoom: if self.population >= 500_000 { 11.0 } else { 13.0 },
        }
    }
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn online_hit(feature: &serde_json::Value) -> Option<Hit> {
    let coordinates = feature.pointer("/geometry/coordinates")?.as_array()?;
    let pos = LatLon::new(coordinates.get(1)?.as_f64()?, coordinates.first()?.as_f64()?).ok()?;
    let property = |key: &str| {
        feature
            .pointer(&format!("/properties/{key}"))
            .and_then(|v| v.as_str())
    };
    let title = property("name")
        .or(property("street"))
        .or(property("city"))?
        .to_string();
    let mut detail: Vec<&str> = Vec::new();
    for part in ["street", "city", "state", "country"]
        .into_iter()
        .filter_map(property)
    {
        if part != title && !detail.contains(&part) {
            detail.push(part);
        }
    }
    let zoom = match property("type") {
        Some("country") => 5.0,
        Some("state") | Some("county") => 8.0,
        Some("city") => 12.0,
        Some("district") | Some("locality") => 14.0,
        _ => 16.0,
    };
    Some(Hit {
        title,
        detail: detail.join(", "),
        pos,
        zoom,
    })
}

pub fn search_online(query: &str, lang: Lang) -> Result<Vec<Hit>, String> {
    let mut url = format!(
        "{ONLINE_ENDPOINT}?q={}&limit={ONLINE_LIMIT}",
        percent_encode(query.trim())
    );
    if lang == Lang::En {
        url.push_str("&lang=en");
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(12)))
        .user_agent(concat!("geozip/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let body = agent
        .get(&url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let features = json
        .get("features")
        .and_then(|f| f.as_array())
        .ok_or("unexpected response")?;
    Ok(features.iter().filter_map(online_hit).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Gazetteer {
        Gazetteer {
            places: [
                "London\tGB\t51.5085\t-0.1257\t8961989\tLondres|Londra",
                "Rome\tIT\t41.8919\t12.5113\t2318895\tRoma",
                "Sevilla\tES\t37.3828\t-5.9732\t686741\tSeville",
                "Romford\tGB\t51.5750\t0.1833\t122000\t",
                "Londrina\tBR\t-23.3103\t-51.1628\t471832\t",
            ]
            .into_iter()
            .filter_map(parse_place)
            .collect(),
        }
    }

    #[test]
    fn finds_by_local_name_alternate_name_and_prefix() {
        let g = sample();
        let titles = |q: &str| g.search(q, 5).into_iter().map(|h| h.title).collect::<Vec<_>>();
        assert_eq!(titles("londres"), ["London"]);
        assert_eq!(titles("Roma"), ["Rome"]);
        assert_eq!(titles("rom"), ["Rome", "Romford"]);
        assert_eq!(titles("lond"), ["London", "Londrina"]);
        assert_eq!(titles("SEVILLA"), ["Sevilla"]);
        assert!(titles("x").is_empty());
        assert!(titles("zzzz").is_empty());
    }

    #[test]
    fn embedded_places_load() {
        let g = Gazetteer::parse(PLACES_GZ);
        assert!(g.by_population().len() > 200_000);
        let madrid = g.search("madrid", 3);
        assert_eq!(madrid[0].detail, "ES");
        assert!((madrid[0].pos.lat - 40.4).abs() < 0.2);
    }

    #[test]
    fn parses_online_results() {
        let feature = serde_json::json!({
            "geometry": {"coordinates": [12.4833, 41.9009]},
            "properties": {"name": "Fontana di Trevi", "street": "Piazza di Trevi", "city": "Roma", "country": "Italia", "type": "house"}
        });
        let hit = online_hit(&feature).unwrap();
        assert_eq!(hit.title, "Fontana di Trevi");
        assert_eq!(hit.detail, "Piazza di Trevi, Roma, Italia");
        assert!((hit.pos.lat - 41.9009).abs() < 1e-9);
        assert_eq!(percent_encode("calle alcalá"), "calle%20alcal%C3%A1");
    }
}
