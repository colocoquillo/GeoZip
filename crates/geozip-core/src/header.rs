use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use crate::error::{Error, Result};
use crate::grid::Shift;

pub const MAGIC: &str = "GZ1";
pub const SALT_LEN: usize = 16;
pub const EXTRA_FIELD_ID: u16 = 0x5A47;
pub const WORDLIST_VERSION: u32 = 1;

pub const MAX_PLACES: usize = 16;
pub const MAX_WORDS_PER_PLACE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    pub const MAX_M_KIB: u32 = 4 * 1024 * 1024;
    pub const MAX_T: u32 = 64;
    pub const MAX_P: u32 = 64;

    pub fn validate(&self) -> Result<()> {
        let ok = (1..=Self::MAX_P).contains(&self.p)
            && (1..=Self::MAX_T).contains(&self.t)
            && self.m_kib >= 8 * self.p
            && self.m_kib <= Self::MAX_M_KIB;
        if ok {
            Ok(())
        } else {
            Err(Error::BadKdfParams(format!(
                "m={} t={} p={}",
                self.m_kib, self.t, self.p
            )))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Alto,
    Extremo,
    ExtremoDoble,
}

impl Level {
    pub fn places(self) -> usize {
        match self {
            Level::Alto | Level::Extremo | Level::ExtremoDoble => 5,
        }
    }

    pub fn words_per_place(self) -> usize {
        match self {
            Level::Alto => 0,
            Level::Extremo => 1,
            Level::ExtremoDoble => 2,
        }
    }

    pub fn kdf(self) -> KdfParams {
        match self {
            Level::Alto => KdfParams {
                m_kib: 256 * 1024,
                t: 3,
                p: 4,
            },
            Level::Extremo | Level::ExtremoDoble => KdfParams {
                m_kib: 512 * 1024,
                t: 4,
                p: 4,
            },
        }
    }

    pub fn from_shape(places: usize, words_per_place: usize) -> Option<Self> {
        [Level::Alto, Level::Extremo, Level::ExtremoDoble]
            .into_iter()
            .find(|l| l.places() == places && l.words_per_place() == words_per_place)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub words_per_place: usize,
    pub wordlist: u32,
    pub kdf: KdfParams,
    pub salt: [u8; SALT_LEN],
    pub shifts: Vec<Shift>,
    pub hidden_names: bool,
}

impl Header {
    pub fn places(&self) -> usize {
        self.shifts.len()
    }

    pub fn level(&self) -> Option<Level> {
        Level::from_shape(self.places(), self.words_per_place)
    }

    pub fn validate(&self) -> Result<()> {
        if self.places() == 0 || self.places() > MAX_PLACES {
            return Err(Error::BadHeader(format!("lugares={}", self.places())));
        }
        if self.words_per_place > MAX_WORDS_PER_PLACE {
            return Err(Error::BadHeader(format!("palabras={}", self.words_per_place)));
        }
        if self.wordlist != WORDLIST_VERSION {
            return Err(Error::BadHeader(format!(
                "versión de lista de palabras no soportada: {}",
                self.wordlist
            )));
        }
        self.kdf.validate()
    }

    pub fn encode(&self) -> String {
        let shifts: Vec<String> = self
            .shifts
            .iter()
            .map(|s| format!("{}{}", s.lat as u8, s.lon as u8))
            .collect();
        format!(
            "{MAGIC} places={} words={} wordlist={} m={} t={} p={} salt={} shifts={} names={}",
            self.places(),
            self.words_per_place,
            self.wordlist,
            self.kdf.m_kib,
            self.kdf.t,
            self.kdf.p,
            URL_SAFE_NO_PAD.encode(self.salt),
            shifts.join("."),
            if self.hidden_names { "hidden" } else { "visible" },
        )
    }

    pub fn decode(text: &str) -> Result<Option<Self>> {
        let line = text.lines().next().unwrap_or("").trim();
        let mut tokens = line.split_ascii_whitespace();
        if tokens.next() != Some(MAGIC) {
            return Ok(None);
        }
        let bad = |what: &str| Error::BadHeader(what.to_string());

        let mut places = None;
        let mut words = None;
        let mut wordlist = None;
        let (mut m, mut t, mut p) = (None, None, None);
        let mut salt = None;
        let mut shifts = None;
        let mut hidden = None;

        for token in tokens {
            let (key, value) = token.split_once('=').ok_or_else(|| bad(token))?;
            let num = || value.parse::<u32>().map_err(|_| bad(token));
            let seen = match key {
                "places" => places.replace(num()? as usize).is_some(),
                "words" => words.replace(num()? as usize).is_some(),
                "wordlist" => wordlist.replace(num()?).is_some(),
                "m" => m.replace(num()?).is_some(),
                "t" => t.replace(num()?).is_some(),
                "p" => p.replace(num()?).is_some(),
                "salt" => {
                    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| bad(token))?;
                    let arr: [u8; SALT_LEN] = bytes.try_into().map_err(|_| bad(token))?;
                    salt.replace(arr).is_some()
                }
                "shifts" => {
                    let parsed = value
                        .split('.')
                        .map(|s| match s {
                            "00" => Ok(Shift {
                                lat: false,
                                lon: false,
                            }),
                            "01" => Ok(Shift {
                                lat: false,
                                lon: true,
                            }),
                            "10" => Ok(Shift {
                                lat: true,
                                lon: false,
                            }),
                            "11" => Ok(Shift { lat: true, lon: true }),
                            _ => Err(bad(token)),
                        })
                        .collect::<Result<Vec<_>>>()?;
                    shifts.replace(parsed).is_some()
                }
                "names" => {
                    let h = match value {
                        "hidden" => true,
                        "visible" => false,
                        _ => return Err(bad(token)),
                    };
                    hidden.replace(h).is_some()
                }
                _ => return Err(bad(token)),
            };
            if seen {
                return Err(bad(token));
            }
        }

        let missing = |k: &str| Error::BadHeader(format!("falta {k}"));
        let shifts = shifts.ok_or_else(|| missing("shifts"))?;
        if places.ok_or_else(|| missing("places"))? != shifts.len() {
            return Err(bad("places no coincide con shifts"));
        }
        let header = Header {
            words_per_place: words.ok_or_else(|| missing("words"))?,
            wordlist: wordlist.ok_or_else(|| missing("wordlist"))?,
            kdf: KdfParams {
                m_kib: m.ok_or_else(|| missing("m"))?,
                t: t.ok_or_else(|| missing("t"))?,
                p: p.ok_or_else(|| missing("p"))?,
            },
            salt: salt.ok_or_else(|| missing("salt"))?,
            shifts,
            hidden_names: hidden.ok_or_else(|| missing("names"))?,
        };
        header.validate()?;
        Ok(Some(header))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Header {
        Header {
            words_per_place: 2,
            wordlist: 1,
            kdf: Level::ExtremoDoble.kdf(),
            salt: [7; SALT_LEN],
            shifts: vec![
                Shift {
                    lat: false,
                    lon: true,
                },
                Shift {
                    lat: true,
                    lon: false,
                },
                Shift { lat: true, lon: true },
                Shift::default(),
            ],
            hidden_names: true,
        }
    }

    #[test]
    fn roundtrip() {
        let h = sample();
        let text = h.encode();
        assert_eq!(
            text,
            "GZ1 places=4 words=2 wordlist=1 m=524288 t=4 p=4 \
             salt=BwcHBwcHBwcHBwcHBwcHBw shifts=01.10.11.00 names=hidden"
        );
        assert_eq!(Header::decode(&text).unwrap(), Some(h.clone()));
        assert_eq!(h.level(), None);
        assert_eq!(Level::from_shape(5, 0), Some(Level::Alto));
        assert_eq!(Level::from_shape(5, 1), Some(Level::Extremo));
        assert_eq!(Level::from_shape(5, 2), Some(Level::ExtremoDoble));
        assert_eq!(Level::from_shape(4, 1), None);
    }

    #[test]
    fn not_ours_and_malformed() {
        assert_eq!(Header::decode("").unwrap(), None);
        assert_eq!(Header::decode("un comentario cualquiera").unwrap(), None);
        let good = sample().encode();
        assert!(Header::decode(&good.replace("places=4", "places=5")).is_err());
        assert!(Header::decode(&good.replace("m=524288", "m=4294967295")).is_err());
        assert!(Header::decode(&good.replace(" names=hidden", "")).is_err());
        assert!(Header::decode(&format!("{good} extra=1")).is_err());
        assert!(Header::decode(&format!("{good} t=1")).is_err());
        assert!(Header::decode(&good.replace("wordlist=1", "wordlist=2")).is_err());
    }
}
