use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::grid::{self, LatLon};
use crate::header::{Header, MAGIC};
use crate::wordlist::WORD_COUNT;

pub const KEY_LEN: usize = 32;

#[derive(Clone)]
pub struct Secret {
    pub places: Vec<LatLon>,
    pub words: Vec<Vec<u16>>,
}

impl Secret {
    pub fn places_only(places: Vec<LatLon>) -> Self {
        let words = vec![Vec::new(); places.len()];
        Self { places, words }
    }

    pub fn new(places: Vec<LatLon>, words: Vec<Vec<u16>>) -> Self {
        Self { places, words }
    }

    pub(crate) fn canonical(&self, header: &Header) -> Result<Zeroizing<String>> {
        if self.places.len() != header.places() {
            return Err(Error::PlaceCount {
                expected: header.places(),
                got: self.places.len(),
            });
        }
        let mut out = Zeroizing::new(format!(
            "{MAGIC}\nplaces={};words={};wordlist={}\n",
            header.places(),
            header.words_per_place,
            header.wordlist
        ));
        for (i, (&place, &shift)) in self.places.iter().zip(&header.shifts).enumerate() {
            let cell = grid::quantize(place, shift);
            out.push_str(&format!(
                "{},{},{}{}",
                cell.lat, cell.lon, shift.lat as u8, shift.lon as u8
            ));

            let mut ids = self.words.get(i).cloned().unwrap_or_default();
            ids.sort_unstable();
            ids.dedup();
            if ids.len() != header.words_per_place {
                return Err(Error::WordCount {
                    place: i + 1,
                    expected: header.words_per_place,
                    got: ids.len(),
                });
            }
            if let Some(&id) = ids.iter().find(|&&id| id as usize >= WORD_COUNT) {
                return Err(Error::UnknownWord(id.to_string()));
            }
            for (j, id) in ids.iter().enumerate() {
                out.push(if j == 0 { ':' } else { ',' });
                out.push_str(&id.to_string());
            }
            out.push('\n');
        }
        Ok(out)
    }
}

pub fn derive_password(header: &Header, secret: &Secret) -> Result<Zeroizing<String>> {
    header.validate()?;
    let canonical = secret.canonical(header)?;
    let bad = |e: argon2::Error| Error::BadKdfParams(e.to_string());
    let params = Params::new(header.kdf.m_kib, header.kdf.t, header.kdf.p, Some(KEY_LEN)).map_err(bad)?;
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(canonical.as_bytes(), &header.salt, &mut key[..])
        .map_err(bad)?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(&key[..])))
}

pub(crate) fn random_bytes(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|e| Error::Random(e.to_string()))
}

pub fn random_words(count: usize) -> Result<Vec<u16>> {
    assert!(count <= WORD_COUNT);
    let mask = WORD_COUNT.next_power_of_two() as u16 - 1;
    let mut ids: Vec<u16> = Vec::with_capacity(count);
    while ids.len() < count {
        let mut raw = [0u8; 2];
        random_bytes(&mut raw)?;
        let id = u16::from_le_bytes(raw) & mask;
        if (id as usize) < WORD_COUNT && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Shift;
    use crate::header::{KdfParams, SALT_LEN, WORDLIST_VERSION};

    pub(crate) fn fast_header(places: usize, words: usize) -> Header {
        Header {
            words_per_place: words,
            wordlist: WORDLIST_VERSION,
            kdf: KdfParams {
                m_kib: 64,
                t: 1,
                p: 1,
            },
            salt: [1; SALT_LEN],
            shifts: vec![Shift::default(); places],
            hidden_names: false,
        }
    }

    fn places(n: usize) -> Vec<LatLon> {
        (0..n)
            .map(|i| LatLon {
                lat: 40.0 + i as f64,
                lon: -3.0 + i as f64,
            })
            .collect()
    }

    #[test]
    fn canonical_format() {
        let h = fast_header(2, 2);
        let s = Secret::new(places(2), vec![vec![900, 5], vec![7, 1849]]);
        assert_eq!(
            s.canonical(&h).unwrap().as_str(),
            "GZ1\nplaces=2;words=2;wordlist=1\n14842,-853,00:5,900\n15213,-561,00:7,1849\n"
        );
    }

    #[test]
    fn word_order_inside_a_place_is_irrelevant_but_place_order_matters() {
        let h = fast_header(2, 2);
        let a = Secret::new(places(2), vec![vec![5, 900], vec![7, 1849]]);
        let b = Secret::new(places(2), vec![vec![900, 5], vec![1849, 7]]);
        assert_eq!(
            *derive_password(&h, &a).unwrap(),
            *derive_password(&h, &b).unwrap()
        );

        let mut swapped = places(2);
        swapped.reverse();
        let c = Secret::new(swapped, vec![vec![7, 1849], vec![5, 900]]);
        assert_ne!(
            *derive_password(&h, &a).unwrap(),
            *derive_password(&h, &c).unwrap()
        );
    }

    #[test]
    fn public_fields_and_salt_change_the_key() {
        let h = fast_header(2, 0);
        let s = Secret::places_only(places(2));
        let base = derive_password(&h, &s).unwrap();
        assert_eq!(base.len(), 43);

        let mut salted = h.clone();
        salted.salt[0] ^= 1;
        assert_ne!(*base, *derive_password(&salted, &s).unwrap());

        let mut shifted = h.clone();
        shifted.shifts[1].lon = true;
        assert_ne!(*base, *derive_password(&shifted, &s).unwrap());
    }

    #[test]
    fn rejects_wrong_shapes() {
        let h = fast_header(2, 2);
        assert!(matches!(
            derive_password(&h, &Secret::places_only(places(3))),
            Err(Error::PlaceCount { .. })
        ));
        assert!(matches!(
            derive_password(&h, &Secret::new(places(2), vec![vec![1, 1], vec![2, 3]])),
            Err(Error::WordCount { place: 1, .. })
        ));
        assert!(matches!(
            derive_password(&h, &Secret::new(places(2), vec![vec![1, 2], vec![2, 5000]])),
            Err(Error::UnknownWord(_))
        ));
    }

    #[test]
    fn random_words_are_distinct_and_in_range() {
        for _ in 0..200 {
            let w = random_words(2).unwrap();
            assert_eq!(w.len(), 2);
            assert!(w[0] < w[1] && (w[1] as usize) < WORD_COUNT);
        }
    }
}
