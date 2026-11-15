use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("coordenada fuera de rango: {0}")]
    BadCoordinate(String),

    #[error("se esperaban {expected} lugares y se han dado {got}")]
    PlaceCount { expected: usize, got: usize },

    #[error("el lugar {place} necesita {expected} palabras distintas y tiene {got}")]
    WordCount {
        place: usize,
        expected: usize,
        got: usize,
    },

    #[error("palabra desconocida: «{0}»")]
    UnknownWord(String),

    #[error("este ZIP no es un archivo GeoZip (no tiene cabecera GZ1)")]
    NotGeoZip,

    #[error("este ZIP ya es un archivo GeoZip")]
    AlreadyGeoZip,

    #[error("cabecera GZ1 no válida: {0}")]
    BadHeader(String),

    #[error("lugares o palabras incorrectos (o el archivo está dañado)")]
    WrongSecret,

    #[error("la contraseña del ZIP original es incorrecta o falta")]
    WrongOldPassword,

    #[error("no hay nada que comprimir")]
    NothingToAdd,

    #[error("el archivo no puede quedarse sin ningún fichero")]
    WouldBeEmpty,

    #[error("no se pueden añadir ficheros a un ZIP convencional con contraseña; conviértelo primero")]
    CannotAddToPasswordZip,

    #[error("ya existe: {0}")]
    AlreadyExists(PathBuf),

    #[error("nombre de entrada no seguro dentro del ZIP: {0}")]
    UnsafeEntryName(String),

    #[error("la entrada «{0}» está dañada o ha sido manipulada")]
    Corrupt(String),

    #[error("la entrada «{0}» no está cifrada; puede haber sido añadida por un tercero")]
    UnencryptedEntry(String),

    #[error("parámetros de derivación no válidos: {0}")]
    BadKdfParams(String),

    #[error("no se pudo obtener aleatoriedad del sistema: {0}")]
    Random(String),

    #[error("{path}: {source}")]
    Path {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("error de ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn at<T>(path: &std::path::Path, r: std::io::Result<T>) -> Result<T> {
    r.map_err(|source| Error::Path {
        path: path.to_path_buf(),
        source,
    })
}
