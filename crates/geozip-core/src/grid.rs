use crate::error::{Error, Result};

pub const GRID_METERS: f64 = 300.0;
pub const SHIFT_METERS: f64 = 150.0;
pub const METERS_PER_DEG: f64 = 111_320.0;
const DEG_TO_RAD: f64 = 0.017453292519943295;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shift {
    pub lat: bool,
    pub lon: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub lat: i64,
    pub lon: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Located {
    pub cell: Cell,
    pub margin_m: f64,
}

impl LatLon {
    pub fn new(lat: f64, lon: f64) -> Result<Self> {
        if !(lat.is_finite() && lon.is_finite())
            || !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
        {
            return Err(Error::BadCoordinate(format!("{lat},{lon}")));
        }
        Ok(Self { lat, lon })
    }

    pub fn parse(text: &str) -> Result<Self> {
        let bad = || Error::BadCoordinate(text.trim().to_string());
        let cleaned = text.trim().trim_matches(|c| c == '(' || c == ')');
        let (a, b) = cleaned.split_once(',').ok_or_else(bad)?;
        let lat: f64 = a.trim().parse().map_err(|_| bad())?;
        let lon: f64 = b.trim().parse().map_err(|_| bad())?;
        Self::new(lat, lon)
    }
}

pub fn parse_places(text: &str) -> Result<Vec<LatLon>> {
    text.split(|c| c == ';' || c == '\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(LatLon::parse)
        .collect()
}

fn offset(shifted: bool) -> f64 {
    if shifted {
        SHIFT_METERS
    } else {
        0.0
    }
}

fn axis(meters: f64, shifted: bool) -> (i64, f64) {
    let t = (meters - offset(shifted)) / GRID_METERS;
    let cell = t.floor();
    let frac = (t - cell) * GRID_METERS;
    (cell as i64, frac.min(GRID_METERS - frac))
}

fn lon_scale(lat_cell: i64, shift_lat: bool) -> f64 {
    let center_m = (lat_cell as f64 + 0.5) * GRID_METERS + offset(shift_lat);
    let center_deg = (center_m / METERS_PER_DEG).clamp(-90.0, 90.0);
    METERS_PER_DEG * libm::cos(center_deg * DEG_TO_RAD)
}

fn wrap_lon(lon: f64) -> f64 {
    if lon >= 180.0 {
        lon - 360.0
    } else {
        lon
    }
}

pub fn locate(p: LatLon, shift: Shift) -> Located {
    let (lat_cell, m_lat) = axis(p.lat * METERS_PER_DEG, shift.lat);
    let (lon_cell, m_lon) = axis(wrap_lon(p.lon) * lon_scale(lat_cell, shift.lat), shift.lon);
    Located {
        cell: Cell {
            lat: lat_cell,
            lon: lon_cell,
        },
        margin_m: m_lat.min(m_lon),
    }
}

pub fn quantize(p: LatLon, shift: Shift) -> Cell {
    locate(p, shift).cell
}

pub fn best_shift(p: LatLon) -> Shift {
    let y = p.lat * METERS_PER_DEG;
    let shift_lat = axis(y, true).1 > axis(y, false).1;
    let (lat_cell, _) = axis(y, shift_lat);
    let x = wrap_lon(p.lon) * lon_scale(lat_cell, shift_lat);
    let shift_lon = axis(x, true).1 > axis(x, false).1;
    Shift {
        lat: shift_lat,
        lon: shift_lon,
    }
}

pub fn cell_bounds(cell: Cell, shift: Shift) -> (LatLon, LatLon) {
    let south = (cell.lat as f64 * GRID_METERS + offset(shift.lat)) / METERS_PER_DEG;
    let north = south + GRID_METERS / METERS_PER_DEG;
    let scale = lon_scale(cell.lat, shift.lat);
    let west = (cell.lon as f64 * GRID_METERS + offset(shift.lon)) / scale;
    let east = west + GRID_METERS / scale;
    (
        LatLon {
            lat: south,
            lon: west,
        },
        LatLon {
            lat: north,
            lon: east,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const CITIES: [(f64, f64); 6] = [
        (40.4168, -3.7038),
        (40.7128, -74.0060),
        (35.6762, 139.6503),
        (-33.8688, 151.2093),
        (64.1466, -21.9426),
        (-54.8019, -68.3030),
    ];

    #[test]
    fn best_shift_guarantees_75_m() {
        for (lat, lon) in CITIES {
            for i in 0..400 {
                let p = LatLon {
                    lat: lat + i as f64 * 0.000_137,
                    lon: lon + i as f64 * 0.000_211,
                };
                let l = locate(p, best_shift(p));
                assert!(l.margin_m >= 75.0 - 1e-6, "{p:?} margin {}", l.margin_m);
            }
        }
    }

    #[test]
    fn cells_are_rectangular_everywhere() {
        for (lat, lon) in CITIES {
            let p = LatLon { lat, lon };
            let shift = best_shift(p);
            let cell = quantize(p, shift);
            let (sw, ne) = cell_bounds(cell, shift);
            for fy in [0.02, 0.5, 0.98] {
                for fx in [0.02, 0.5, 0.98] {
                    let q = LatLon {
                        lat: sw.lat + (ne.lat - sw.lat) * fy,
                        lon: sw.lon + (ne.lon - sw.lon) * fx,
                    };
                    assert_eq!(quantize(q, shift), cell, "{q:?}");
                }
            }
            let outside = LatLon {
                lat: ne.lat + (ne.lat - sw.lat) * 0.02,
                lon: sw.lon,
            };
            assert_ne!(quantize(outside, shift), cell);
        }
    }

    #[test]
    fn tolerates_75_m_error_after_best_shift() {
        for (lat, lon) in CITIES {
            let p = LatLon { lat, lon };
            let shift = best_shift(p);
            let cell = quantize(p, shift);
            let dlat = 74.0 / METERS_PER_DEG;
            let dlon = 74.0 / (METERS_PER_DEG * libm::cos(lat * DEG_TO_RAD));
            for (sy, sx) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (0.7, 0.7)] {
                let q = LatLon {
                    lat: lat + sy * dlat,
                    lon: lon + sx * dlon,
                };
                assert_eq!(quantize(q, shift), cell, "{q:?}");
            }
        }
    }

    #[test]
    fn known_cells() {
        let madrid = LatLon {
            lat: 40.4168,
            lon: -3.7038,
        };
        assert_eq!(
            quantize(madrid, Shift::default()),
            Cell {
                lat: 14997,
                lon: -1047
            }
        );
        let tokyo = LatLon {
            lat: 35.6762,
            lon: 139.6503,
        };
        assert_eq!(
            quantize(tokyo, Shift { lat: true, lon: true }),
            Cell {
                lat: 13237,
                lon: 42094
            }
        );
    }

    #[test]
    fn parsing() {
        assert!(LatLon::parse("40.4168, -3.7038").is_ok());
        assert!(LatLon::parse("(40.4168,-3.7038)").is_ok());
        assert!(LatLon::parse("91,0").is_err());
        assert!(LatLon::parse("40.4").is_err());
        assert_eq!(parse_places("1,2; 3,4\n5,6").unwrap().len(), 3);
    }

    #[test]
    fn antimeridian_is_one_meridian() {
        let a = LatLon {
            lat: 10.0,
            lon: 180.0,
        };
        let b = LatLon {
            lat: 10.0,
            lon: -180.0,
        };
        assert_eq!(quantize(a, Shift::default()), quantize(b, Shift::default()));
    }
}
