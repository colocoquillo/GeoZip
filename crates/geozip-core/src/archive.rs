use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use time::{OffsetDateTime, PrimitiveDateTime};
use zip::result::ZipError;
use zip::write::{FileOptions, FullFileOptions};
use zip::{AesMode, CompressionMethod, DateTime, ExtraField, ZipArchive, ZipWriter};

use crate::error::{at, Error, Result};
use crate::grid;
use crate::header::{Header, KdfParams, Level, EXTRA_FIELD_ID, SALT_LEN, WORDLIST_VERSION};
use crate::secret::{derive_password, random_bytes, Secret};

pub const INNER_NAME: &str = "contenido.zip";

const BUF_LEN: usize = 128 * 1024;
const ZIP64_THRESHOLD: u64 = 0xFFF0_0000;
const HEADER_SEARCH_LIMIT: usize = 32;

#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    DerivingKey,
    Entry(&'a str),
    Bytes { done: u64, total: u64 },
}

pub type Progress<'a> = &'a mut dyn FnMut(Event<'_>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compression {
    Store,
    Fast,
    #[default]
    Normal,
    Best,
}

impl Compression {
    fn method(self) -> CompressionMethod {
        match self {
            Compression::Store => CompressionMethod::Stored,
            _ => CompressionMethod::Deflated,
        }
    }

    fn level(self) -> Option<i64> {
        match self {
            Compression::Store | Compression::Normal => None,
            Compression::Fast => Some(1),
            Compression::Best => Some(9),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Edit {
    pub remove: Vec<String>,
    pub add: Vec<PathBuf>,
    pub add_under: String,
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub level: Level,
    pub hide_names: bool,
    pub overwrite: bool,
    pub kdf: Option<KdfParams>,
    pub compression: Compression,
}

impl CreateOptions {
    pub fn new(level: Level) -> Self {
        Self {
            level,
            hide_names: false,
            overwrite: false,
            kdf: None,
            compression: Compression::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Created {
    pub header: Header,
    pub files: usize,
    pub bytes: u64,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Packed {
    pub files: usize,
    pub bytes: u64,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PlainInfo {
    pub entries: Vec<EntryInfo>,
    pub needs_password: bool,
}

#[derive(Debug, Clone)]
pub struct Extracted {
    pub files: usize,
    pub bytes: u64,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EntryInfo {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub modified: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Info {
    pub header: Header,
    pub entries: Option<Vec<EntryInfo>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Meta {
    mtime: Option<i64>,
    mode: Option<u32>,
}

impl Meta {
    fn of(md: &fs::Metadata) -> Self {
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            Some(md.permissions().mode() & 0o777)
        };
        #[cfg(not(unix))]
        let mode = None;
        Self { mtime, mode }
    }
}

fn dos_time(mtime: Option<i64>) -> DateTime {
    mtime
        .and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok())
        .and_then(|t| DateTime::try_from(PrimitiveDateTime::new(t.date(), t.time())).ok())
        .unwrap_or_else(DateTime::default_for_write)
}

struct Sink<'a, W: Write + Seek> {
    zip: ZipWriter<W>,
    crypt: Option<(&'a str, &'a str)>,
    progress: Progress<'a>,
    compression: Compression,
    done: u64,
    total: u64,
    files: usize,
}

impl<'a, W: Write + Seek> Sink<'a, W> {
    fn new(
        zip: ZipWriter<W>,
        crypt: Option<(&'a str, &'a str)>,
        compression: Compression,
        total: u64,
        progress: Progress<'a>,
    ) -> Self {
        Self {
            zip,
            crypt,
            progress,
            compression,
            done: 0,
            total,
            files: 0,
        }
    }

    fn base_options(meta: Meta, compression: Compression) -> Result<FullFileOptions<'a, 'static>> {
        let mut o: FullFileOptions = FileOptions::default()
            .compression_method(compression.method())
            .compression_level(compression.level())
            .last_modified_time(dos_time(meta.mtime));
        if let Some(mode) = meta.mode {
            o = o.unix_permissions(mode);
        }
        if let Some(t) = meta.mtime.and_then(|t| u32::try_from(t).ok()) {
            o = o.with_extended_timestamp(Some(t), None, None)?;
        }
        Ok(o)
    }

    fn dir(&mut self, name: &str, meta: Meta) -> Result<()> {
        (self.progress)(Event::Entry(name));
        let o = Self::base_options(meta, Compression::Store)?;
        self.zip.add_directory(name, o)?;
        Ok(())
    }

    fn file(
        &mut self,
        name: &str,
        meta: Meta,
        size: u64,
        compression: Compression,
        reader: &mut dyn Read,
    ) -> Result<()> {
        (self.progress)(Event::Entry(name));
        let mut o = Self::base_options(meta, compression)?.large_file(size >= ZIP64_THRESHOLD);
        if let Some((password, header_text)) = self.crypt {
            o.add_extra_field(EXTRA_FIELD_ID, header_text.as_bytes(), false)?;
            o = o.with_aes_encryption(AesMode::Aes256, password);
        }
        self.zip.start_file(name, o)?;
        let mut buf = vec![0u8; BUF_LEN];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            self.zip.write_all(&buf[..n])?;
            self.done += n as u64;
            (self.progress)(Event::Bytes {
                done: self.done,
                total: self.total,
            });
        }
        self.files += 1;
        Ok(())
    }
}

enum FsItem {
    Dir {
        name: String,
        meta: Meta,
    },
    File {
        name: String,
        path: PathBuf,
        size: u64,
        meta: Meta,
    },
}

fn walk(path: &Path, name: String, items: &mut Vec<FsItem>, skipped: &mut Vec<String>) -> Result<()> {
    let md = at(path, fs::symlink_metadata(path))?;
    if md.file_type().is_symlink() {
        skipped.push(name);
    } else if md.is_dir() {
        items.push(FsItem::Dir {
            name: format!("{name}/"),
            meta: Meta::of(&md),
        });
        let mut children: Vec<_> = at(path, fs::read_dir(path))?
            .collect::<io::Result<Vec<_>>>()
            .map_err(|source| Error::Path {
                path: path.to_path_buf(),
                source,
            })?;
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let child_name = format!("{name}/{}", child.file_name().to_string_lossy());
            walk(&child.path(), child_name, items, skipped)?;
        }
    } else if md.is_file() {
        items.push(FsItem::File {
            name,
            path: path.to_path_buf(),
            size: md.len(),
            meta: Meta::of(&md),
        });
    } else {
        skipped.push(name);
    }
    Ok(())
}

fn collect(inputs: &[PathBuf]) -> Result<(Vec<FsItem>, Vec<String>)> {
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    for input in inputs {
        let abs = at(input, fs::canonicalize(input))?;
        let name = abs
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| Error::UnsafeEntryName(input.display().to_string()))?;
        walk(&abs, name, &mut items, &mut skipped)?;
    }
    if !items.iter().any(|i| matches!(i, FsItem::File { .. })) {
        return Err(Error::NothingToAdd);
    }
    Ok((items, skipped))
}

fn new_header(secret: &Secret, opts: &CreateOptions) -> Result<Header> {
    if secret.places.len() != opts.level.places() {
        return Err(Error::PlaceCount {
            expected: opts.level.places(),
            got: secret.places.len(),
        });
    }
    let mut salt = [0u8; SALT_LEN];
    random_bytes(&mut salt)?;
    Ok(Header {
        words_per_place: opts.level.words_per_place(),
        wordlist: WORDLIST_VERSION,
        kdf: opts.kdf.unwrap_or_else(|| opts.level.kdf()),
        salt,
        shifts: secret.places.iter().map(|&p| grid::best_shift(p)).collect(),
        hidden_names: opts.hide_names,
    })
}

fn partial_path(out: &Path) -> PathBuf {
    let mut name = out.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    out.with_file_name(name)
}

fn write_archive(
    out: &Path,
    secret: &Secret,
    opts: &CreateOptions,
    total: u64,
    progress: Progress<'_>,
    fill: &mut dyn FnMut(&mut dyn SinkDyn) -> Result<()>,
) -> Result<(Header, usize, u64)> {
    if out.exists() && !opts.overwrite {
        return Err(Error::AlreadyExists(out.to_path_buf()));
    }
    let header = new_header(secret, opts)?;
    progress(Event::DerivingKey);
    let password = derive_password(&header, secret)?;
    let header_text = header.encode();

    let tmp = partial_path(out);
    let result = (|| -> Result<(usize, u64)> {
        let file = BufWriter::new(at(&tmp, File::create(&tmp))?);
        let (file, files, bytes) = if opts.hide_names {
            let inner_zip = ZipWriter::new(Cursor::new(Vec::new()));
            let mut inner = Sink::new(inner_zip, None, opts.compression, total, &mut *progress);
            fill(&mut inner)?;
            let (files, bytes) = (inner.files, inner.done);
            let inner_bytes = inner.zip.finish()?.into_inner();

            let mut quiet = |_: Event<'_>| {};
            let crypt = Some((password.as_str(), header_text.as_str()));
            let mut outer = Sink::new(ZipWriter::new(file), crypt, Compression::Store, 0, &mut quiet);
            outer.file(
                INNER_NAME,
                Meta::default(),
                inner_bytes.len() as u64,
                Compression::Store,
                &mut inner_bytes.as_slice(),
            )?;
            outer.zip.set_comment(header_text.clone())?;
            (outer.zip.finish()?, files, bytes)
        } else {
            let crypt = Some((password.as_str(), header_text.as_str()));
            let mut sink = Sink::new(
                ZipWriter::new(file),
                crypt,
                opts.compression,
                total,
                &mut *progress,
            );
            fill(&mut sink)?;
            sink.zip.set_comment(header_text.clone())?;
            let (files, bytes) = (sink.files, sink.done);
            (sink.zip.finish()?, files, bytes)
        };
        let file = file.into_inner().map_err(|e| e.into_error())?;
        at(&tmp, file.sync_all())?;
        Ok((files, bytes))
    })();

    match result {
        Ok((files, bytes)) => {
            at(out, fs::rename(&tmp, out))?;
            Ok((header, files, bytes))
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

trait SinkDyn {
    fn dir(&mut self, name: &str, meta: Meta) -> Result<()>;
    fn file(&mut self, name: &str, meta: Meta, size: u64, reader: &mut dyn Read) -> Result<()>;
}

impl<W: Write + Seek> SinkDyn for Sink<'_, W> {
    fn dir(&mut self, name: &str, meta: Meta) -> Result<()> {
        Sink::dir(self, name, meta)
    }
    fn file(&mut self, name: &str, meta: Meta, size: u64, reader: &mut dyn Read) -> Result<()> {
        Sink::file(self, name, meta, size, self.compression, reader)
    }
}

fn total_size(items: &[FsItem]) -> u64 {
    items
        .iter()
        .map(|item| match item {
            FsItem::File { size, .. } => *size,
            FsItem::Dir { .. } => 0,
        })
        .sum()
}

fn add_items(items: &[FsItem], sink: &mut dyn SinkDyn) -> Result<()> {
    for item in items {
        match item {
            FsItem::Dir { name, meta } => sink.dir(name, *meta)?,
            FsItem::File {
                name,
                path,
                size,
                meta,
            } => {
                let mut reader = BufReader::new(at(path, File::open(path))?);
                sink.file(name, *meta, *size, &mut reader)?;
            }
        }
    }
    Ok(())
}

pub fn create_plain(
    out: &Path,
    inputs: &[PathBuf],
    overwrite: bool,
    compression: Compression,
    progress: Progress<'_>,
) -> Result<Packed> {
    if out.exists() && !overwrite {
        return Err(Error::AlreadyExists(out.to_path_buf()));
    }
    let (items, skipped) = collect(inputs)?;
    let tmp = partial_path(out);
    let written = (|| -> Result<(usize, u64)> {
        let writer = ZipWriter::new(BufWriter::new(at(&tmp, File::create(&tmp))?));
        let mut sink = Sink::new(writer, None, compression, total_size(&items), progress);
        add_items(&items, &mut sink)?;
        let (files, bytes) = (sink.files, sink.done);
        let file = sink.zip.finish()?.into_inner().map_err(|e| e.into_error())?;
        at(&tmp, file.sync_all())?;
        Ok((files, bytes))
    })();
    match written {
        Ok((files, bytes)) => {
            at(out, fs::rename(&tmp, out))?;
            Ok(Packed {
                files,
                bytes,
                skipped,
            })
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

pub fn create(
    out: &Path,
    inputs: &[PathBuf],
    secret: &Secret,
    opts: &CreateOptions,
    progress: Progress<'_>,
) -> Result<Created> {
    let (items, skipped) = collect(inputs)?;
    let (header, files, bytes) =
        write_archive(out, secret, opts, total_size(&items), progress, &mut |sink| {
            add_items(&items, sink)
        })?;
    Ok(Created {
        header,
        files,
        bytes,
        skipped,
    })
}

pub fn migrate(
    source: &Path,
    old_password: Option<&str>,
    out: &Path,
    secret: &Secret,
    opts: &CreateOptions,
    progress: Progress<'_>,
) -> Result<Created> {
    let mut archive = ZipArchive::new(BufReader::new(at(source, File::open(source))?))?;
    if find_header(&mut archive)?.is_some() {
        return Err(Error::AlreadyGeoZip);
    }

    let mut total = 0;
    let mut first_encrypted = None;
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i)?;
        total += entry.size();
        if entry.encrypted() && first_encrypted.is_none() {
            first_encrypted = Some(i);
        }
    }
    if archive.len() == 0 {
        return Err(Error::NothingToAdd);
    }
    if let Some(i) = first_encrypted {
        let password = old_password.ok_or(Error::WrongOldPassword)?;
        if let Err(ZipError::InvalidPassword) = archive.by_index_decrypt(i, password.as_bytes()) {
            return Err(Error::WrongOldPassword);
        }
    }

    let mut skipped = Vec::new();
    let (header, files, bytes) = write_archive(out, secret, opts, total, progress, &mut |sink| {
        for i in 0..archive.len() {
            let encrypted = archive.by_index_raw(i)?.encrypted();
            let mut entry = if encrypted {
                let password = old_password.ok_or(Error::WrongOldPassword)?;
                archive
                    .by_index_decrypt(i, password.as_bytes())
                    .map_err(old_password_error)?
            } else {
                archive.by_index(i)?
            };
            let raw_name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
            let name = entry
                .enclosed_name()
                .map(|p| slash_name(&p))
                .filter(|n| !n.is_empty())
                .ok_or_else(|| Error::UnsafeEntryName(raw_name.clone()))?;
            let meta = Meta {
                mtime: entry_mtime(&entry),
                mode: entry.unix_mode().map(|m| m & 0o777),
            };
            if entry.is_symlink() {
                skipped.push(name);
            } else if entry.is_dir() {
                sink.dir(&format!("{}/", name.trim_end_matches('/')), meta)?;
            } else {
                let size = entry.size();
                sink.file(&name, meta, size, &mut entry).map_err(|e| match e {
                    Error::Io(_) if encrypted => Error::WrongOldPassword,
                    other => other,
                })?;
            }
        }
        Ok(())
    })?;
    Ok(Created {
        header,
        files,
        bytes,
        skipped,
    })
}

fn old_password_error(e: ZipError) -> Error {
    match e {
        ZipError::InvalidPassword => Error::WrongOldPassword,
        other => other.into(),
    }
}

fn slash_name(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn entry_mtime<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> Option<i64> {
    let extended = entry.extra_data_fields().find_map(|f| match f {
        ExtraField::ExtendedTimestamp(ts) => ts.mod_time().map(i64::from),
        _ => None,
    });
    extended.or_else(|| {
        let dt = PrimitiveDateTime::try_from(entry.last_modified()?).ok()?;
        Some(dt.assume_utc().unix_timestamp())
    })
}

pub fn find_header<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<Option<Header>> {
    let comment = String::from_utf8_lossy(archive.comment()).into_owned();
    if let Some(header) = Header::decode(&comment)? {
        return Ok(Some(header));
    }
    for i in 0..archive.len().min(HEADER_SEARCH_LIMIT) {
        let entry = archive.by_index_raw(i)?;
        for field in entry.extra_data_fields() {
            if let ExtraField::Custom(custom) = field {
                if custom.header_id == EXTRA_FIELD_ID {
                    let text = String::from_utf8_lossy(&custom.data).into_owned();
                    if let Some(header) = Header::decode(&text)? {
                        return Ok(Some(header));
                    }
                }
            }
        }
    }
    Ok(None)
}

fn list_entries<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<Vec<EntryInfo>> {
    (0..archive.len())
        .map(|i| {
            let e = archive.by_index_raw(i)?;
            Ok(EntryInfo {
                name: e.name().map(|n| n.into_owned()).unwrap_or_default(),
                size: e.size(),
                is_dir: e.is_dir(),
                modified: entry_mtime(&e),
            })
        })
        .collect()
}

pub fn inspect(path: &Path) -> Result<Info> {
    let mut archive = ZipArchive::new(BufReader::new(at(path, File::open(path))?))?;
    let header = find_header(&mut archive)?.ok_or(Error::NotGeoZip)?;
    let entries = if header.hidden_names {
        None
    } else {
        Some(list_entries(&mut archive)?)
    };
    Ok(Info { header, entries })
}

pub fn zip_needs_password(path: &Path) -> Result<bool> {
    let mut archive = ZipArchive::new(BufReader::new(at(path, File::open(path))?))?;
    if find_header(&mut archive)?.is_some() {
        return Err(Error::AlreadyGeoZip);
    }
    for i in 0..archive.len() {
        if archive.by_index_raw(i)?.encrypted() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn wrong_secret(e: ZipError) -> Error {
    match e {
        ZipError::InvalidPassword => Error::WrongSecret,
        other => other.into(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub only: Vec<String>,
    pub relative_to: String,
    pub verify_only: bool,
}

impl Scope {
    fn includes(&self, name: &str) -> bool {
        self.only.is_empty()
            || self
                .only
                .iter()
                .any(|wanted| name == wanted || (wanted.ends_with('/') && name.starts_with(wanted.as_str())))
    }
}

struct Extractor<'a> {
    out_dir: &'a Path,
    scope: &'a Scope,
    overwrite: bool,
    progress: Progress<'a>,
    done: u64,
    total: u64,
    files: usize,
    skipped: Vec<String>,
}

impl Extractor<'_> {
    fn run<R: Read + Seek>(
        &mut self,
        archive: &mut ZipArchive<R>,
        password: Option<&str>,
        every_file_must_be_encrypted: bool,
    ) -> Result<()> {
        for i in 0..archive.len() {
            let raw = archive.by_index_raw(i)?;
            let name = raw.name().map(|n| n.into_owned()).unwrap_or_default();
            if self.scope.includes(&name) {
                self.total += raw.size();
            }
            if every_file_must_be_encrypted && !raw.encrypted() && !raw.is_dir() {
                let name = raw.name().map(|n| n.into_owned()).unwrap_or_default();
                return Err(Error::UnencryptedEntry(name));
            }
        }
        for i in 0..archive.len() {
            let raw = archive.by_index_raw(i)?;
            let encrypted = raw.encrypted();
            if !self
                .scope
                .includes(&raw.name().map(|n| n.into_owned()).unwrap_or_default())
            {
                continue;
            }
            drop(raw);
            let mut entry = match (encrypted, password) {
                (true, Some(p)) => archive.by_index_decrypt(i, p.as_bytes()).map_err(wrong_secret)?,
                (true, None) => return Err(Error::WrongSecret),
                (false, _) => archive.by_index(i)?,
            };
            let raw_name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
            let rel = entry
                .enclosed_name()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or_else(|| Error::UnsafeEntryName(raw_name.clone()))?;
            let rel = rel
                .strip_prefix(Path::new(&self.scope.relative_to))
                .map(Path::to_path_buf)
                .unwrap_or(rel);
            let dest = self.out_dir.join(&rel);
            (self.progress)(Event::Entry(&raw_name));

            if entry.is_symlink() {
                self.skipped.push(raw_name);
                continue;
            }
            if self.scope.verify_only {
                if !entry.is_dir() {
                    self.read_through(&mut entry, &raw_name)?;
                    self.files += 1;
                }
                continue;
            }
            if entry.is_dir() {
                at(&dest, fs::create_dir_all(&dest))?;
                continue;
            }
            if let Some(parent) = dest.parent() {
                at(parent, fs::create_dir_all(parent))?;
            }
            if dest.exists() && !self.overwrite {
                return Err(Error::AlreadyExists(dest));
            }
            let mtime = entry_mtime(&entry);
            let mode = entry.unix_mode();
            let copied = self.copy(&mut entry, &dest, &raw_name);
            if copied.is_err() {
                let _ = fs::remove_file(&dest);
            }
            let file = copied?;
            if let Some(t) = mtime.and_then(|t| u64::try_from(t).ok()) {
                let _ = file.set_modified(UNIX_EPOCH + Duration::from_secs(t));
            }
            drop(file);
            #[cfg(unix)]
            if let Some(mode) = mode {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(mode & 0o777));
            }
            #[cfg(not(unix))]
            let _ = mode;
            self.files += 1;
        }
        Ok(())
    }

    fn read_through(&mut self, entry: &mut dyn Read, name: &str) -> Result<()> {
        let mut buf = vec![0u8; BUF_LEN];
        loop {
            let n = entry
                .read(&mut buf)
                .map_err(|_| Error::Corrupt(name.to_string()))?;
            if n == 0 {
                return Ok(());
            }
            self.done += n as u64;
            (self.progress)(Event::Bytes {
                done: self.done,
                total: self.total,
            });
        }
    }

    fn copy(&mut self, entry: &mut dyn Read, dest: &Path, name: &str) -> Result<File> {
        let mut file = at(dest, File::create(dest))?;
        let mut buf = vec![0u8; BUF_LEN];
        loop {
            let n = entry
                .read(&mut buf)
                .map_err(|_| Error::Corrupt(name.to_string()))?;
            if n == 0 {
                break;
            }
            at(dest, file.write_all(&buf[..n]))?;
            self.done += n as u64;
            (self.progress)(Event::Bytes {
                done: self.done,
                total: self.total,
            });
        }
        Ok(file)
    }
}

pub fn extract_plain_entry(path: &Path, entry_name: &str, out_dir: &Path) -> Result<PathBuf> {
    let mut archive = ZipArchive::new(BufReader::new(at(path, File::open(path))?))?;
    let mut entry = archive.by_name(entry_name)?;
    let file_name = entry
        .enclosed_name()
        .and_then(|p| p.file_name().map(PathBuf::from))
        .filter(|_| entry.is_file())
        .ok_or_else(|| Error::UnsafeEntryName(entry_name.to_string()))?;
    at(out_dir, fs::create_dir_all(out_dir))?;
    let dest = out_dir.join(file_name);
    let mut file = at(&dest, File::create(&dest))?;
    io::copy(&mut entry, &mut file).map_err(|_| Error::Corrupt(entry_name.to_string()))?;
    Ok(dest)
}

pub fn inspect_plain(path: &Path) -> Result<PlainInfo> {
    let mut archive = ZipArchive::new(BufReader::new(at(path, File::open(path))?))?;
    if find_header(&mut archive)?.is_some() {
        return Err(Error::AlreadyGeoZip);
    }
    let mut needs_password = false;
    for i in 0..archive.len() {
        needs_password |= archive.by_index_raw(i)?.encrypted();
    }
    Ok(PlainInfo {
        entries: list_entries(&mut archive)?,
        needs_password,
    })
}

pub fn extract_plain(
    path: &Path,
    password: Option<&str>,
    out_dir: &Path,
    overwrite: bool,
    progress: Progress<'_>,
) -> Result<Extracted> {
    extract_plain_scoped(path, password, out_dir, overwrite, &Scope::default(), progress)
}

pub fn extract_plain_scoped(
    path: &Path,
    password: Option<&str>,
    out_dir: &Path,
    overwrite: bool,
    scope: &Scope,
    progress: Progress<'_>,
) -> Result<Extracted> {
    let mut archive = ZipArchive::new(BufReader::new(at(path, File::open(path))?))?;
    if find_header(&mut archive)?.is_some() {
        return Err(Error::AlreadyGeoZip);
    }
    let first_encrypted =
        (0..archive.len()).find(|&i| archive.by_index_raw(i).map(|e| e.encrypted()).unwrap_or(false));
    if let Some(index) = first_encrypted {
        let password = password.ok_or(Error::WrongOldPassword)?;
        if let Err(ZipError::InvalidPassword) = archive.by_index_decrypt(index, password.as_bytes()) {
            return Err(Error::WrongOldPassword);
        }
    }
    if !scope.verify_only {
        at(out_dir, fs::create_dir_all(out_dir))?;
    }
    let mut extractor = Extractor {
        out_dir,
        scope,
        overwrite,
        progress,
        done: 0,
        total: 0,
        files: 0,
        skipped: Vec::new(),
    };
    extractor
        .run(&mut archive, password, false)
        .map_err(|e| match e {
            Error::WrongSecret => Error::WrongOldPassword,
            other => other,
        })?;
    Ok(Extracted {
        files: extractor.files,
        bytes: extractor.done,
        skipped: extractor.skipped,
    })
}

pub fn extract(
    path: &Path,
    secret: &Secret,
    out_dir: &Path,
    overwrite: bool,
    progress: Progress<'_>,
) -> Result<Extracted> {
    extract_scoped(path, secret, out_dir, overwrite, &Scope::default(), progress)
}

pub fn extract_scoped(
    path: &Path,
    secret: &Secret,
    out_dir: &Path,
    overwrite: bool,
    scope: &Scope,
    progress: Progress<'_>,
) -> Result<Extracted> {
    let mut archive = ZipArchive::new(BufReader::new(at(path, File::open(path))?))?;
    let header = find_header(&mut archive)?.ok_or(Error::NotGeoZip)?;
    progress(Event::DerivingKey);
    let password = derive_password(&header, secret)?;

    let first = (0..archive.len())
        .find(|&i| archive.by_index_raw(i).map(|e| e.encrypted()).unwrap_or(false))
        .ok_or(Error::NotGeoZip)?;
    archive
        .by_index_decrypt(first, password.as_bytes())
        .map_err(wrong_secret)?;

    if !scope.verify_only {
        at(out_dir, fs::create_dir_all(out_dir))?;
    }
    let mut extractor = Extractor {
        out_dir,
        scope,
        overwrite,
        progress,
        done: 0,
        total: 0,
        files: 0,
        skipped: Vec::new(),
    };

    if header.hidden_names {
        let mut inner_bytes = Vec::new();
        archive
            .by_index_decrypt(first, password.as_bytes())
            .map_err(wrong_secret)?
            .read_to_end(&mut inner_bytes)
            .map_err(|_| Error::Corrupt(INNER_NAME.to_string()))?;
        let mut inner = ZipArchive::new(Cursor::new(inner_bytes))?;
        extractor.run(&mut inner, None, false)?;
    } else {
        extractor.run(&mut archive, Some(password.as_str()), true)?;
    }

    Ok(Extracted {
        files: extractor.files,
        bytes: extractor.done,
        skipped: extractor.skipped,
    })
}

pub fn list_hidden(path: &Path, secret: &Secret) -> Result<Vec<EntryInfo>> {
    let mut archive = ZipArchive::new(BufReader::new(at(path, File::open(path))?))?;
    let header = find_header(&mut archive)?.ok_or(Error::NotGeoZip)?;
    let password = derive_password(&header, secret)?;
    if !header.hidden_names {
        archive
            .by_index_decrypt(0, password.as_bytes())
            .map_err(wrong_secret)?;
        return list_entries(&mut archive);
    }
    let mut inner_bytes = Vec::new();
    archive
        .by_index_decrypt(0, password.as_bytes())
        .map_err(wrong_secret)?
        .read_to_end(&mut inner_bytes)
        .map_err(|_| Error::Corrupt(INNER_NAME.to_string()))?;
    list_entries(&mut ZipArchive::new(Cursor::new(inner_bytes))?)
}

pub fn unix_to_system_time(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn names_match(patterns: &[String], name: &str) -> bool {
    patterns
        .iter()
        .any(|p| name == p || (p.ends_with('/') && name.starts_with(p.as_str())))
}

fn additions(edit: &Edit) -> Result<(Vec<FsItem>, Vec<String>)> {
    if edit.add.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let (mut items, skipped) = collect(&edit.add)?;
    for item in &mut items {
        let (FsItem::Dir { name, .. } | FsItem::File { name, .. }) = item;
        *name = format!("{}{name}", edit.add_under);
    }
    Ok((items, skipped))
}

fn copy_kept<R: Read + Seek, W: Write + Seek>(
    source: &mut ZipArchive<R>,
    sink: &mut Sink<'_, W>,
    edit: &Edit,
    added: &[FsItem],
) -> Result<usize> {
    let replaced: Vec<&str> = added
        .iter()
        .map(|item| match item {
            FsItem::Dir { name, .. } | FsItem::File { name, .. } => name.as_str(),
        })
        .collect();
    let mut kept_files = 0;
    for i in 0..source.len() {
        let entry = source.by_index_raw(i)?;
        let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
        if names_match(&edit.remove, &name) || replaced.contains(&name.as_str()) {
            continue;
        }
        if !entry.is_dir() {
            kept_files += 1;
        }
        sink.zip.raw_copy_file(entry)?;
    }
    Ok(kept_files)
}

fn write_partial(
    path: &Path,
    write: impl FnOnce(BufWriter<File>) -> Result<BufWriter<File>>,
) -> Result<PathBuf> {
    let tmp = partial_path(path);
    let written = at(&tmp, File::create(&tmp))
        .and_then(|file| write(BufWriter::new(file)))
        .and_then(|writer| writer.into_inner().map_err(|e| Error::Io(e.into_error())))
        .and_then(|file| at(&tmp, file.sync_all()));
    match written {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn replace_with(partial: &Path, path: &Path) -> Result<()> {
    let renamed = at(path, fs::rename(partial, path));
    if renamed.is_err() {
        let _ = fs::remove_file(partial);
    }
    renamed
}

pub fn edit_plain(
    path: &Path,
    edit: &Edit,
    compression: Compression,
    progress: Progress<'_>,
) -> Result<Packed> {
    let mut source = ZipArchive::new(BufReader::new(at(path, File::open(path))?))?;
    if find_header(&mut source)?.is_some() {
        return Err(Error::AlreadyGeoZip);
    }
    let mut has_password = false;
    for i in 0..source.len() {
        has_password |= source.by_index_raw(i)?.encrypted();
    }
    if has_password && !edit.add.is_empty() {
        return Err(Error::CannotAddToPasswordZip);
    }
    let (items, skipped) = additions(edit)?;
    let comment: Box<[u8]> = source.comment().into();
    let mut counts = (0, 0);
    let partial = write_partial(path, |file| {
        let mut sink = Sink::new(
            ZipWriter::new(file),
            None,
            compression,
            total_size(&items),
            progress,
        );
        let kept = copy_kept(&mut source, &mut sink, edit, &items)?;
        add_items(&items, &mut sink)?;
        if kept + sink.files == 0 {
            return Err(Error::WouldBeEmpty);
        }
        counts = (kept + sink.files, sink.done);
        sink.zip.set_raw_comment(comment)?;
        Ok(sink.zip.finish()?)
    })?;
    drop(source);
    replace_with(&partial, path)?;
    Ok(Packed {
        files: counts.0,
        bytes: counts.1,
        skipped,
    })
}

pub fn edit(
    path: &Path,
    secret: Option<&Secret>,
    edit: &Edit,
    compression: Compression,
    progress: Progress<'_>,
) -> Result<Packed> {
    let mut source = ZipArchive::new(BufReader::new(at(path, File::open(path))?))?;
    let header = find_header(&mut source)?.ok_or(Error::NotGeoZip)?;
    let needs_secret = header.hidden_names || !edit.add.is_empty();
    let password = match secret {
        Some(secret) => {
            progress(Event::DerivingKey);
            let password = derive_password(&header, secret)?;
            let first = (0..source.len())
                .find(|&i| source.by_index_raw(i).map(|e| e.encrypted()).unwrap_or(false))
                .ok_or(Error::NotGeoZip)?;
            source
                .by_index_decrypt(first, password.as_bytes())
                .map_err(wrong_secret)?;
            Some(password)
        }
        None if needs_secret => return Err(Error::WrongSecret),
        None => None,
    };
    let (items, skipped) = additions(edit)?;
    let header_text = header.encode();
    let crypt = password.as_ref().map(|p| (p.as_str(), header_text.as_str()));
    let mut counts = (0, 0);

    if header.hidden_names {
        let password = password.as_ref().ok_or(Error::WrongSecret)?;
        let mut inner_bytes = Vec::new();
        source
            .by_index_decrypt(0, password.as_bytes())
            .map_err(wrong_secret)?
            .read_to_end(&mut inner_bytes)
            .map_err(|_| Error::Corrupt(INNER_NAME.to_string()))?;
        let mut inner_source = ZipArchive::new(Cursor::new(inner_bytes))?;
        let inner_zip = ZipWriter::new(Cursor::new(Vec::new()));
        let mut inner = Sink::new(inner_zip, None, compression, total_size(&items), progress);
        let kept = copy_kept(&mut inner_source, &mut inner, edit, &items)?;
        add_items(&items, &mut inner)?;
        if kept + inner.files == 0 {
            return Err(Error::WouldBeEmpty);
        }
        counts = (kept + inner.files, inner.done);
        let new_inner = inner.zip.finish()?.into_inner();
        drop(source);
        let partial = write_partial(path, |file| {
            let mut quiet = |_: Event<'_>| {};
            let mut outer = Sink::new(ZipWriter::new(file), crypt, Compression::Store, 0, &mut quiet);
            outer.file(
                INNER_NAME,
                Meta::default(),
                new_inner.len() as u64,
                Compression::Store,
                &mut new_inner.as_slice(),
            )?;
            outer.zip.set_comment(header_text.clone())?;
            Ok(outer.zip.finish()?)
        })?;
        replace_with(&partial, path)?;
    } else {
        let partial = write_partial(path, |file| {
            let mut sink = Sink::new(
                ZipWriter::new(file),
                crypt,
                compression,
                total_size(&items),
                progress,
            );
            let kept = copy_kept(&mut source, &mut sink, edit, &items)?;
            add_items(&items, &mut sink)?;
            if kept + sink.files == 0 {
                return Err(Error::WouldBeEmpty);
            }
            counts = (kept + sink.files, sink.done);
            sink.zip.set_comment(header_text.clone())?;
            Ok(sink.zip.finish()?)
        })?;
        drop(source);
        replace_with(&partial, path)?;
    }
    Ok(Packed {
        files: counts.0,
        bytes: counts.1,
        skipped,
    })
}
