mod dbg_hook;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use geozip_core::archive::{
    self, Compression, CreateOptions, Created, Edit, EntryInfo, Event, Extracted, Packed, Scope,
};
use geozip_core::grid::{self, LatLon, Shift};
use geozip_core::{random_words, Error, Header, Lang, Level, Secret, Wordlist};
use time::UtcOffset;

use crate::browser::{self, Row, Sort, SortBy};
use crate::dialogs;
use crate::icons::{self, Icon};
use crate::map::{MapView, Source};
use crate::search;
use crate::settings::Settings;
use crate::system;
use crate::text::{format_time, human_size, kind_of, level_summary, Tr};

const WORD_REROLLS: u8 = 3;
const SUGGESTIONS: usize = 6;
const ROW_HEIGHT: f32 = 20.0;
const BUTTON_SIZE: egui::Vec2 = egui::vec2(96.0, 26.0);

pub const OK_GREEN: Color32 = Color32::from_rgb(0, 148, 50);
pub const WARN: Color32 = Color32::from_rgb(180, 83, 9);
pub const ERR_RED: Color32 = Color32::from_rgb(200, 40, 40);
const ACTIVE_BLUE: Color32 = Color32::from_rgb(9, 132, 227);

pub struct Opened {
    pub path: PathBuf,
    pub header: Header,
    entries: Option<Vec<EntryInfo>>,
    remembered_secret: Option<Secret>,
}

struct PlainZip {
    path: PathBuf,
    entries: Vec<EntryInfo>,
    needs_password: bool,
}

enum Doc {
    New(Vec<PathBuf>),
    Archive(Opened),
    Plain(PlainZip),
}

#[derive(Clone)]
pub struct OldZip {
    pub needs_password: bool,
    pub password: String,
}

#[derive(Clone)]
pub struct CompressForm {
    pub output: PathBuf,
    pub output_chosen_by_user: bool,
    pub named_after: PathBuf,
    pub level: Option<Level>,
    pub hide_names: bool,
    pub compression: Compression,
    pub convert: Option<OldZip>,
    pub error: Option<String>,
}

impl CompressForm {
    pub fn use_default_output(&mut self) {
        if !self.output_chosen_by_user {
            let stem = self.named_after.file_stem().unwrap_or_default().to_string_lossy();
            let extension = if self.level.is_some() { "geo.zip" } else { "zip" };
            let mut output = self.named_after.with_file_name(format!("{stem}.{extension}"));
            if output == self.named_after {
                output = self.named_after.with_file_name(format!("{stem}-2.{extension}"));
            }
            self.output = output;
        }
    }
}

#[derive(Clone)]
pub struct ExtractForm {
    pub out_dir: PathBuf,
    pub overwrite: bool,
    pub password: Option<String>,
    pub scope: Scope,
    pub error: Option<String>,
}

enum Pending {
    Compress(CompressForm),
    Extract(ExtractForm),
    Unlock,
    Edit(Edit),
}

pub enum Outcome {
    Created {
        output: PathBuf,
        files: usize,
        bytes: u64,
        skipped: Vec<String>,
        words: Vec<Vec<u16>>,
        protected: bool,
    },
    Extracted {
        out_dir: PathBuf,
        extracted: Extracted,
    },
    Verified {
        extracted: Extracted,
    },
}

enum Dialog {
    None,
    Compress(CompressForm),
    Extract(ExtractForm),
    ConfirmRemoval(Edit),
    Working,
    Finished(Outcome),
    Info,
    OfflineMaps,
    About,
    Message(String),
}

#[derive(PartialEq)]
enum View {
    Browser,
    Places,
}

#[derive(Clone, Copy)]
enum Action {
    New,
    OpenArchive,
    OpenRecent(usize),
    AddFiles,
    AddFolder,
    Remove,
    Compress,
    Convert,
    Extract,
    Test,
    Unlock,
    Info,
    Up,
    OpenRow(usize),
    SortBy(SortBy),
    FindChanged,
    OfflineMapsHelp,
    About,
    Quit,
    Language(Lang),
    MapOnline,
    MapOfflinePlain,
    MapOfflineFile,
    #[cfg(target_os = "linux")]
    ToggleDesktopEntry,
}

#[derive(Default)]
struct PlaceEntry {
    lat: String,
    lon: String,
    pos: Option<LatLon>,
    generated: Vec<u16>,
    rerolls_left: u8,
    typed: Vec<String>,
}

enum Msg {
    Entry(String),
    Bytes(u64, u64),
    Created(Result<Created, Error>),
    Packed(Result<Packed, Error>),
    Extracted(Result<Extracted, Error>),
    Listed(Result<Vec<EntryInfo>, Error>),
    Edited(Result<Option<Vec<EntryInfo>>, Error>),
}

#[derive(Default)]
pub struct Progress {
    pub current: String,
    pub done: u64,
    pub total: u64,
}

pub struct App {
    settings: Settings,
    local_offset: UtcOffset,
    logo: egui::TextureHandle,
    view: View,
    doc: Doc,
    folder: String,
    find: String,
    rows: Vec<Row>,
    sort: Sort,
    selected: BTreeSet<usize>,
    dialog: Dialog,
    pending: Option<Pending>,
    job_secret: Option<Secret>,
    places: Vec<PlaceEntry>,
    active: usize,
    places_error: Option<String>,
    map: MapView,
    worker: Option<Receiver<Msg>>,
    progress: Progress,
    search: places::Search,
    preview_dir: Option<PathBuf>,
}

fn input_row(path: &Path) -> Row {
    let metadata = std::fs::metadata(path).ok();
    let modified = metadata
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    let is_dir = metadata.as_ref().is_some_and(|m| m.is_dir());
    Row {
        name: path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        path: path.to_string_lossy().into_owned(),
        is_dir,
        size: if is_dir {
            0
        } else {
            metadata.as_ref().map_or(0, |m| m.len())
        },
        modified,
    }
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        paths: Vec<PathBuf>,
        local_offset: UtcOffset,
        logo: egui::ColorImage,
    ) -> Self {
        let settings = Settings::load();
        let mut map = MapView::new(&cc.egui_ctx, settings.lang);
        if settings.map != Source::Online {
            map.set_source(settings.map.clone(), &cc.egui_ctx);
        }
        let mut app = Self {
            settings,
            local_offset,
            logo: cc
                .egui_ctx
                .load_texture("logo", logo, egui::TextureOptions::LINEAR),
            view: View::Browser,
            doc: Doc::New(Vec::new()),
            folder: String::new(),
            find: String::new(),
            rows: Vec::new(),
            sort: Sort::default(),
            selected: BTreeSet::new(),
            dialog: Dialog::None,
            pending: None,
            job_secret: None,
            places: Vec::new(),
            active: 0,
            places_error: None,
            map,
            worker: None,
            progress: Progress::default(),
            search: places::Search::default(),
            preview_dir: None,
        };
        search::load_in_background();
        if !paths.is_empty() {
            app.take_paths(paths, &cc.egui_ctx);
        }
        app
    }

    fn lang(&self) -> Lang {
        self.settings.lang
    }

    fn tr(&self) -> Tr {
        Tr(self.settings.lang)
    }

    fn entries(&self) -> Option<&[EntryInfo]> {
        match &self.doc {
            Doc::Archive(opened) => opened.entries.as_deref(),
            Doc::Plain(zip) => Some(&zip.entries),
            Doc::New(_) => None,
        }
    }

    fn contents_are_hidden(&self) -> bool {
        matches!(&self.doc, Doc::Archive(Opened { entries: None, .. }))
    }

    fn finding(&self) -> bool {
        !self.find.trim().is_empty() && self.entries().is_some()
    }

    fn refresh_rows(&mut self) {
        let mut rows = match (&self.doc, self.entries()) {
            (Doc::New(inputs), _) => inputs.iter().map(|path| input_row(path)).collect(),
            (_, Some(entries)) if self.finding() => browser::find(entries, &self.find),
            (_, Some(entries)) => browser::list_folder(entries, &self.folder),
            (_, None) => Vec::new(),
        };
        browser::sort(&mut rows, self.sort);
        self.rows = rows;
        self.selected.clear();
    }

    fn show_doc(&mut self, doc: Doc) {
        self.doc = doc;
        self.folder.clear();
        self.find.clear();
        self.refresh_rows();
    }

    fn take_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        if self.worker.is_some() || self.view != View::Browser || !matches!(self.dialog, Dialog::None) {
            return;
        }
        let building_a_list = matches!(&self.doc, Doc::New(inputs) if !inputs.is_empty());
        match paths.as_slice() {
            [single] if is_zip(single) && !building_a_list => self.open_archive(single.clone()),
            _ => self.add_paths(paths, ctx),
        }
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        if paths.is_empty() {
            return;
        }
        if let Doc::New(inputs) = &mut self.doc {
            for path in paths {
                if !inputs.contains(&path) {
                    inputs.push(path);
                }
            }
            self.refresh_rows();
        } else {
            let edit = Edit {
                add: paths,
                add_under: self.folder.clone(),
                ..Edit::default()
            };
            self.start(Pending::Edit(edit), ctx);
        }
    }

    fn inspect(path: &Path) -> Result<Doc, Error> {
        match archive::inspect(path) {
            Ok(info) => Ok(Doc::Archive(Opened {
                path: path.to_path_buf(),
                header: info.header,
                entries: info.entries,
                remembered_secret: None,
            })),
            Err(Error::NotGeoZip) => archive::inspect_plain(path).map(|info| {
                Doc::Plain(PlainZip {
                    path: path.to_path_buf(),
                    entries: info.entries,
                    needs_password: info.needs_password,
                })
            }),
            Err(e) => Err(e),
        }
    }

    fn open_archive(&mut self, path: PathBuf) {
        match Self::inspect(&path) {
            Ok(doc) => {
                self.show_doc(doc);
                self.settings.remember(&path);
                self.settings.save();
            }
            Err(e) => self.dialog = Dialog::Message(e.to_string()),
        }
    }

    fn reload_after_edit(&mut self, hidden_entries: Option<Vec<EntryInfo>>) {
        let Some(path) = self.archive_path().map(Path::to_path_buf) else {
            return;
        };
        let remembered = match &mut self.doc {
            Doc::Archive(opened) => opened.remembered_secret.take(),
            _ => None,
        };
        match Self::inspect(&path) {
            Ok(mut doc) => {
                if let Doc::Archive(opened) = &mut doc {
                    opened.remembered_secret = remembered;
                    if opened.entries.is_none() {
                        opened.entries = hidden_entries;
                    }
                }
                self.doc = doc;
                self.refresh_rows();
            }
            Err(e) => self.dialog = Dialog::Message(e.to_string()),
        }
    }

    fn compress_form(named_after: &Path, convert: Option<OldZip>) -> CompressForm {
        let mut form = CompressForm {
            output: PathBuf::new(),
            output_chosen_by_user: false,
            named_after: named_after.to_path_buf(),
            level: Some(Level::Extremo),
            hide_names: true,
            compression: Compression::Normal,
            convert,
            error: None,
        };
        form.use_default_output();
        form
    }

    fn archive_path(&self) -> Option<&Path> {
        match &self.doc {
            Doc::Archive(opened) => Some(&opened.path),
            Doc::Plain(zip) => Some(&zip.path),
            Doc::New(_) => None,
        }
    }

    fn zip_password_prompt(&self) -> Option<String> {
        matches!(&self.doc, Doc::Plain(zip) if zip.needs_password).then(String::new)
    }

    fn selected_paths(&self) -> Vec<String> {
        self.selected
            .iter()
            .filter_map(|&i| self.rows.get(i))
            .map(|row| row.path.clone())
            .collect()
    }

    fn selection_scope(&self) -> Scope {
        let relative_to = if self.finding() {
            ""
        } else {
            self.folder.trim_end_matches('/')
        };
        Scope {
            only: self.selected_paths(),
            relative_to: relative_to.to_string(),
            verify_only: false,
        }
    }

    fn open_row(&mut self, index: usize, ctx: &egui::Context) {
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        let opened = match &self.doc {
            Doc::New(_) => system::open_with_default_program(Path::new(&row.path)).map_err(Error::from),
            _ if row.is_dir => {
                self.folder = row.path;
                self.find.clear();
                self.refresh_rows();
                return;
            }
            Doc::Plain(zip) if !zip.needs_password => {
                let dir = self.preview_dir.get_or_insert_with(|| {
                    std::env::temp_dir().join(format!("geozip-{}", std::process::id()))
                });
                archive::extract_plain_entry(&zip.path, &row.path, dir)
                    .and_then(|file| system::open_with_default_program(&file).map_err(Error::from))
            }
            _ => {
                self.selected = BTreeSet::from([index]);
                self.perform(Action::Extract, ctx);
                return;
            }
        };
        if let Err(e) = opened {
            self.dialog = Dialog::Message(e.to_string());
        }
    }

    fn use_map(&mut self, source: Source, ctx: &egui::Context) {
        self.map.set_source(source.clone(), ctx);
        self.settings.map = source;
        self.settings.save();
    }

    fn perform(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::New => self.show_doc(Doc::New(Vec::new())),
            Action::OpenArchive => {
                if let Some(file) = rfd::FileDialog::new().add_filter("ZIP", &["zip"]).pick_file() {
                    self.open_archive(file);
                }
            }
            Action::OpenRecent(index) => {
                if let Some(path) = self.settings.recent.get(index).cloned() {
                    self.open_archive(path);
                }
            }
            Action::AddFiles => {
                if let Some(files) = rfd::FileDialog::new().pick_files() {
                    self.add_paths(files, ctx);
                }
            }
            Action::AddFolder => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.add_paths(vec![dir], ctx);
                }
            }
            Action::Remove => {
                let chosen = self.selected_paths();
                if chosen.is_empty() {
                    return;
                }
                if let Doc::New(inputs) = &mut self.doc {
                    inputs.retain(|input| !chosen.contains(&input.to_string_lossy().into_owned()));
                    self.refresh_rows();
                } else {
                    self.dialog = Dialog::ConfirmRemoval(Edit {
                        remove: chosen,
                        ..Edit::default()
                    });
                }
            }
            Action::Compress => {
                if let Doc::New(inputs) = &self.doc {
                    if let Some(first) = inputs.first() {
                        self.dialog = Dialog::Compress(Self::compress_form(first, None));
                    }
                }
            }
            Action::Convert => {
                if let Doc::Plain(zip) = &self.doc {
                    let old = OldZip {
                        needs_password: zip.needs_password,
                        password: String::new(),
                    };
                    self.dialog = Dialog::Compress(Self::compress_form(&zip.path, Some(old)));
                }
            }
            Action::Extract => {
                let Some(path) = self.archive_path() else { return };
                let stem = path.file_stem().unwrap_or_default().to_os_string();
                self.dialog = Dialog::Extract(ExtractForm {
                    out_dir: path.with_file_name(stem),
                    overwrite: false,
                    password: self.zip_password_prompt(),
                    scope: self.selection_scope(),
                    error: None,
                });
            }
            Action::Test => {
                if self.archive_path().is_none() {
                    return;
                }
                let form = ExtractForm {
                    out_dir: PathBuf::new(),
                    overwrite: false,
                    password: self.zip_password_prompt(),
                    scope: Scope {
                        verify_only: true,
                        ..Scope::default()
                    },
                    error: None,
                };
                if form.password.is_some() {
                    self.dialog = Dialog::Extract(form);
                } else {
                    self.start(Pending::Extract(form), ctx);
                }
            }
            Action::Unlock => {
                if self.contents_are_hidden() {
                    self.start(Pending::Unlock, ctx);
                }
            }
            Action::Info => self.dialog = Dialog::Info,
            Action::Up => {
                self.folder = browser::parent_folder(&self.folder);
                self.refresh_rows();
            }
            Action::OpenRow(index) => self.open_row(index, ctx),
            Action::SortBy(by) => {
                self.sort = self.sort.toggled(by);
                self.refresh_rows();
            }
            Action::FindChanged => self.refresh_rows(),
            Action::OfflineMapsHelp => self.dialog = Dialog::OfflineMaps,
            Action::About => self.dialog = Dialog::About,
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Action::Language(lang) => {
                self.settings.lang = lang;
                self.settings.save();
                self.map.set_language(lang, ctx);
            }
            Action::MapOnline => self.use_map(Source::Online, ctx),
            Action::MapOfflinePlain => self.use_map(Source::OfflinePlain, ctx),
            Action::MapOfflineFile => {
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("PMTiles", &["pmtiles"])
                    .pick_file()
                {
                    self.use_map(Source::OfflineFile(file), ctx);
                }
            }
            #[cfg(target_os = "linux")]
            Action::ToggleDesktopEntry => {
                let result = if crate::desktop::is_registered() {
                    crate::desktop::unregister()
                } else {
                    crate::desktop::register(crate::ICON_PNG)
                };
                if let Err(e) = result {
                    self.dialog = Dialog::Message(e.to_string());
                }
            }
        }
    }

    fn uses_archive_secret(&self, pending: &Pending) -> bool {
        match (pending, &self.doc) {
            (Pending::Compress(_), _) => false,
            (Pending::Edit(edit), Doc::Archive(opened)) => opened.header.hidden_names || !edit.add.is_empty(),
            (_, Doc::Archive(_)) => true,
            _ => false,
        }
    }

    fn remembered_secret(&self) -> Option<&Secret> {
        match &self.doc {
            Doc::Archive(opened) => opened.remembered_secret.as_ref(),
            _ => None,
        }
    }

    fn needs_places(&self, pending: &Pending) -> bool {
        match pending {
            Pending::Compress(form) => form.level.is_some(),
            other => self.uses_archive_secret(other) && self.remembered_secret().is_none(),
        }
    }

    fn asking_for_an_existing_archive(&self) -> bool {
        matches!(
            self.pending,
            Some(Pending::Extract(_) | Pending::Unlock | Pending::Edit(_))
        )
    }

    fn start(&mut self, pending: Pending, ctx: &egui::Context) {
        if self.needs_places(&pending) {
            self.ask_places(pending);
        } else {
            self.pending = Some(pending);
            self.run(ctx);
        }
    }

    fn ask_places(&mut self, pending: Pending) {
        let entries = match (&pending, &self.doc) {
            (Pending::Compress(form), _) => {
                let level = form.level.unwrap_or(Level::Alto);
                (0..level.places())
                    .map(|_| {
                        Ok(PlaceEntry {
                            generated: random_words(level.words_per_place())?,
                            rerolls_left: WORD_REROLLS,
                            ..Default::default()
                        })
                    })
                    .collect::<Result<Vec<_>, Error>>()
            }
            (_, Doc::Archive(opened)) => Ok((0..opened.header.places())
                .map(|_| PlaceEntry {
                    typed: vec![String::new(); opened.header.words_per_place],
                    ..Default::default()
                })
                .collect()),
            _ => return,
        };
        match entries {
            Ok(entries) => {
                self.places = entries;
                self.active = 0;
                self.places_error = None;
                self.pending = Some(pending);
                self.view = View::Places;
            }
            Err(e) => self.dialog = Dialog::Message(e.to_string()),
        }
    }

    fn leave_places(&mut self) {
        self.view = View::Browser;
        self.pending = None;
        self.places.clear();
        self.places_error = None;
    }

    fn shift_for(&self, index: usize, pos: LatLon) -> Shift {
        match &self.doc {
            Doc::Archive(opened) if self.asking_for_an_existing_archive() => opened.header.shifts[index],
            _ => grid::best_shift(pos),
        }
    }

    fn set_place(&mut self, index: usize, pos: LatLon) {
        let Some(entry) = self.places.get_mut(index) else {
            return;
        };
        let pos = places::rounded(pos);
        entry.pos = Some(pos);
        entry.lat = format!("{:.*}", places::COORDINATE_DECIMALS, pos.lat);
        entry.lon = format!("{:.*}", places::COORDINATE_DECIMALS, pos.lon);
        if let Some(next) = (0..self.places.len()).find(|&j| self.places[j].pos.is_none()) {
            self.active = next;
        }
    }

    fn secret(&self) -> Option<Secret> {
        let places: Vec<LatLon> = self.places.iter().map(|p| p.pos).collect::<Option<_>>()?;
        let words = if self.asking_for_an_existing_archive() {
            self.places
                .iter()
                .map(|p| {
                    let mut ids: Vec<u16> = p
                        .typed
                        .iter()
                        .map(|w| Wordlist::get().lookup_any(w, self.lang()))
                        .collect::<Option<_>>()?;
                    ids.sort_unstable();
                    ids.dedup();
                    (ids.len() == p.typed.len()).then_some(ids)
                })
                .collect::<Option<_>>()?
        } else {
            self.places.iter().map(|p| p.generated.clone()).collect()
        };
        Some(Secret::new(places, words))
    }

    fn run(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending else { return };
        let secret = if self.view == View::Places {
            match self.secret() {
                Some(secret) => Some(secret),
                None => return,
            }
        } else if self.uses_archive_secret(pending) {
            self.remembered_secret().cloned()
        } else {
            None
        };
        let (tx, rx) = mpsc::channel();
        let report = {
            let tx = tx.clone();
            let ctx = ctx.clone();
            move |event: Event<'_>| {
                let msg = match event {
                    Event::DerivingKey => return,
                    Event::Entry(name) => Msg::Entry(name.to_string()),
                    Event::Bytes { done, total } => Msg::Bytes(done, total),
                };
                let _ = tx.send(msg);
                ctx.request_repaint();
            }
        };
        let ctx = ctx.clone();
        let job_secret = secret.clone();
        let job: Box<dyn FnOnce() -> Msg + Send> = match (pending, &self.doc, secret) {
            (Pending::Compress(form), Doc::New(inputs), secret) => {
                let (output, sources, compression) = (form.output.clone(), inputs.clone(), form.compression);
                match (form.level, secret) {
                    (Some(level), Some(secret)) => {
                        let options = create_options(level, form);
                        Box::new(move || {
                            let mut report = report;
                            Msg::Created(archive::create(&output, &sources, &secret, &options, &mut report))
                        })
                    }
                    _ => Box::new(move || {
                        let mut report = report;
                        Msg::Packed(archive::create_plain(
                            &output,
                            &sources,
                            true,
                            compression,
                            &mut report,
                        ))
                    }),
                }
            }
            (Pending::Compress(form), Doc::Plain(zip), Some(secret)) => {
                let (source, output) = (zip.path.clone(), form.output.clone());
                let options = create_options(form.level.unwrap_or(Level::Alto), form);
                let old_password = form
                    .convert
                    .as_ref()
                    .filter(|old| old.needs_password)
                    .map(|old| old.password.clone());
                Box::new(move || {
                    let mut report = report;
                    let password = old_password.as_deref();
                    Msg::Created(archive::migrate(
                        &source,
                        password,
                        &output,
                        &secret,
                        &options,
                        &mut report,
                    ))
                })
            }
            (Pending::Extract(form), Doc::Archive(opened), Some(secret)) => {
                let (path, form) = (opened.path.clone(), form.clone());
                Box::new(move || {
                    let mut report = report;
                    Msg::Extracted(archive::extract_scoped(
                        &path,
                        &secret,
                        &form.out_dir,
                        form.overwrite,
                        &form.scope,
                        &mut report,
                    ))
                })
            }
            (Pending::Extract(form), Doc::Plain(zip), _) => {
                let (path, form) = (zip.path.clone(), form.clone());
                Box::new(move || {
                    let mut report = report;
                    Msg::Extracted(archive::extract_plain_scoped(
                        &path,
                        form.password.as_deref(),
                        &form.out_dir,
                        form.overwrite,
                        &form.scope,
                        &mut report,
                    ))
                })
            }
            (Pending::Unlock, Doc::Archive(opened), Some(secret)) => {
                let path = opened.path.clone();
                Box::new(move || Msg::Listed(archive::list_hidden(&path, &secret)))
            }
            (Pending::Edit(edit), Doc::Archive(opened), secret) => {
                let (path, edit, hidden) = (opened.path.clone(), edit.clone(), opened.header.hidden_names);
                Box::new(move || {
                    let mut report = report;
                    let edited =
                        archive::edit(&path, secret.as_ref(), &edit, Compression::Normal, &mut report);
                    Msg::Edited(edited.and_then(|_| match (&secret, hidden) {
                        (Some(secret), true) => archive::list_hidden(&path, secret).map(Some),
                        _ => Ok(None),
                    }))
                })
            }
            (Pending::Edit(edit), Doc::Plain(zip), _) => {
                let (path, edit) = (zip.path.clone(), edit.clone());
                Box::new(move || {
                    let mut report = report;
                    Msg::Edited(
                        archive::edit_plain(&path, &edit, Compression::Normal, &mut report).map(|_| None),
                    )
                })
            }
            _ => return,
        };
        thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
        self.job_secret = job_secret;
        self.worker = Some(rx);
        self.progress = Progress::default();
        self.places_error = None;
        self.dialog = Dialog::Working;
    }

    fn remember_job_secret(&mut self) {
        let secret = self.job_secret.take();
        if let (Doc::Archive(opened), Some(secret)) = (&mut self.doc, secret) {
            opened.remembered_secret = Some(secret);
        }
    }

    fn poll_worker(&mut self) {
        let Some(rx) = &self.worker else { return };
        let mut finished = None;
        for msg in rx.try_iter() {
            match msg {
                Msg::Entry(name) => self.progress.current = name,
                Msg::Bytes(done, total) => {
                    self.progress.done = done;
                    self.progress.total = total;
                }
                other => finished = Some(other),
            }
        }
        let Some(msg) = finished else { return };
        self.worker = None;
        self.dialog = Dialog::None;
        let words: Vec<Vec<u16>> = self.places.iter().map(|p| p.generated.clone()).collect();
        let result = match (msg, self.pending.take()) {
            (Msg::Created(Ok(created)), Some(Pending::Compress(form))) => Ok(Some(Outcome::Created {
                output: form.output,
                files: created.files,
                bytes: created.bytes,
                skipped: created.skipped,
                words,
                protected: true,
            })),
            (Msg::Packed(Ok(packed)), Some(Pending::Compress(form))) => Ok(Some(Outcome::Created {
                output: form.output,
                files: packed.files,
                bytes: packed.bytes,
                skipped: packed.skipped,
                words: Vec::new(),
                protected: false,
            })),
            (Msg::Extracted(Ok(extracted)), Some(Pending::Extract(form))) => {
                self.remember_job_secret();
                Ok(Some(if form.scope.verify_only {
                    Outcome::Verified { extracted }
                } else {
                    Outcome::Extracted {
                        out_dir: form.out_dir,
                        extracted,
                    }
                }))
            }
            (Msg::Listed(Ok(entries)), _) => {
                self.remember_job_secret();
                if let Doc::Archive(opened) = &mut self.doc {
                    opened.entries = Some(entries);
                }
                self.refresh_rows();
                Ok(None)
            }
            (Msg::Edited(Ok(hidden_entries)), _) => {
                self.remember_job_secret();
                self.reload_after_edit(hidden_entries);
                Ok(None)
            }
            (Msg::Created(Err(e)), pending)
            | (Msg::Packed(Err(e)), pending)
            | (Msg::Extracted(Err(e)), pending)
            | (Msg::Listed(Err(e)), pending)
            | (Msg::Edited(Err(e)), pending) => Err((e, pending)),
            _ => return,
        };
        self.job_secret = None;
        match result {
            Ok(outcome) => {
                self.leave_places();
                if let Some(Outcome::Created { output, .. }) = &outcome {
                    self.open_archive(output.clone());
                }
                if let Some(outcome) = outcome {
                    self.dialog = Dialog::Finished(outcome);
                }
            }
            Err((Error::WrongOldPassword, Some(Pending::Compress(mut form)))) => {
                self.leave_places();
                form.error = Some(Error::WrongOldPassword.to_string());
                self.dialog = Dialog::Compress(form);
            }
            Err((Error::WrongOldPassword, Some(Pending::Extract(mut form)))) => {
                self.leave_places();
                form.error = Some(Error::WrongOldPassword.to_string());
                self.dialog = Dialog::Extract(form);
            }
            Err((e, pending)) if self.view == View::Places => {
                self.pending = pending;
                self.places_error = Some(e.to_string());
            }
            Err((e, _)) => self.dialog = Dialog::Message(e.to_string()),
        }
    }

    fn menu_bar(&self, ui: &mut egui::Ui) -> Option<Action> {
        let tr = self.tr();
        let has_inputs = matches!(&self.doc, Doc::New(inputs) if !inputs.is_empty());
        let is_archive = !matches!(self.doc, Doc::New(_));
        let is_plain = matches!(self.doc, Doc::Plain(_));
        let is_protected = matches!(self.doc, Doc::Archive(_));
        let mut action = None;
        let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, value: Action| {
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                action = Some(value);
                ui.close();
            }
        };
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(tr.t("Archivo", "File"), |ui| {
                item(ui, true, tr.t("Nuevo", "New"), Action::New);
                item(
                    ui,
                    true,
                    tr.t("Abrir archivo…", "Open archive…"),
                    Action::OpenArchive,
                );
                ui.menu_button(tr.t("Recientes", "Recent"), |ui| {
                    if self.settings.recent.is_empty() {
                        ui.weak(tr.t("Ninguno todavía", "None yet"));
                    }
                    for (index, path) in self.settings.recent.iter().enumerate() {
                        item(ui, true, &path.display().to_string(), Action::OpenRecent(index));
                    }
                });
                ui.separator();
                item(ui, true, tr.t("Salir", "Quit"), Action::Quit);
            });
            ui.menu_button(tr.t("Órdenes", "Commands"), |ui| {
                item(ui, true, tr.t("Añadir archivos…", "Add files…"), Action::AddFiles);
                item(
                    ui,
                    true,
                    tr.t("Añadir carpeta…", "Add folder…"),
                    Action::AddFolder,
                );
                item(
                    ui,
                    !self.selected.is_empty(),
                    tr.t("Quitar", "Remove"),
                    Action::Remove,
                );
                item(ui, has_inputs, tr.t("Comprimir…", "Compress…"), Action::Compress);
                ui.separator();
                item(
                    ui,
                    is_archive,
                    tr.t("Extraer en…", "Extract to…"),
                    Action::Extract,
                );
                item(ui, is_archive, tr.t("Comprobar", "Test"), Action::Test);
                item(
                    ui,
                    self.contents_are_hidden(),
                    tr.t("Ver el contenido…", "Show the contents…"),
                    Action::Unlock,
                );
                item(
                    ui,
                    is_plain,
                    tr.t(
                        "Convertir a archivo protegido…",
                        "Convert to a protected archive…",
                    ),
                    Action::Convert,
                );
                item(ui, is_protected, tr.t("Información", "Information"), Action::Info);
            });
            ui.menu_button(tr.t("Opciones", "Options"), |ui| {
                ui.menu_button(tr.t("Idioma", "Language"), |ui| {
                    item(ui, true, "Español", Action::Language(Lang::Es));
                    item(ui, true, "English", Action::Language(Lang::En));
                });
                ui.menu_button(tr.t("Mapa", "Map"), |ui| {
                    item(ui, true, tr.t("En línea", "Online"), Action::MapOnline);
                    item(ui, true, tr.t("Sin conexión", "Offline"), Action::MapOfflinePlain);
                    item(
                        ui,
                        true,
                        tr.t("Sin conexión con mapa .pmtiles…", "Offline with a .pmtiles map…"),
                        Action::MapOfflineFile,
                    );
                });
                #[cfg(target_os = "linux")]
                {
                    let label = if crate::desktop::is_registered() {
                        tr.t(
                            "Quitar del menú de aplicaciones",
                            "Remove from the applications menu",
                        )
                    } else {
                        tr.t("Añadir al menú de aplicaciones", "Add to the applications menu")
                    };
                    item(ui, true, label, Action::ToggleDesktopEntry);
                }
            });
            ui.menu_button(tr.t("Ayuda", "Help"), |ui| {
                item(
                    ui,
                    true,
                    tr.t("Mapas sin conexión…", "Offline maps…"),
                    Action::OfflineMapsHelp,
                );
                item(ui, true, tr.t("Acerca de GeoZip", "About GeoZip"), Action::About);
            });
        });
        action
    }

    fn toolbar(&self, ui: &mut egui::Ui) -> Option<Action> {
        let tr = self.tr();
        let is_new = matches!(self.doc, Doc::New(_));
        let has_inputs = is_new && !self.rows.is_empty();
        let is_plain = matches!(self.doc, Doc::Plain(_));
        let is_protected = matches!(self.doc, Doc::Archive(_));
        let mut action = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let mut tool = |ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool, value: Action| {
                if icons::tool_button(ui, icon, label, enabled) {
                    action = Some(value);
                }
            };
            tool(ui, Icon::AddFiles, tr.t("Añadir", "Add"), true, Action::AddFiles);
            tool(
                ui,
                Icon::AddFolder,
                tr.t("Carpeta", "Folder"),
                true,
                Action::AddFolder,
            );
            tool(
                ui,
                Icon::Remove,
                tr.t("Quitar", "Remove"),
                !self.selected.is_empty(),
                Action::Remove,
            );
            tool(
                ui,
                Icon::Compress,
                tr.t("Comprimir", "Compress"),
                has_inputs,
                Action::Compress,
            );
            ui.separator();
            tool(ui, Icon::Open, tr.t("Abrir", "Open"), true, Action::OpenArchive);
            tool(
                ui,
                Icon::Extract,
                tr.t("Extraer en", "Extract to"),
                !is_new,
                Action::Extract,
            );
            tool(ui, Icon::Test, tr.t("Comprobar", "Test"), !is_new, Action::Test);
            tool(
                ui,
                Icon::Protect,
                tr.t("Convertir", "Convert"),
                is_plain,
                Action::Convert,
            );
            tool(
                ui,
                Icon::Info,
                tr.t("Información", "Info"),
                is_protected,
                Action::Info,
            );
        });
        action
    }

    fn address_bar(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let tr = self.tr();
        let mut action = None;
        ui.horizontal(|ui| {
            if icons::small_button(ui, Icon::Up, !self.folder.is_empty() && !self.finding()) {
                action = Some(Action::Up);
            }
            let (icon, text) = match &self.doc {
                Doc::New(_) => (Icon::Folder, tr.t("Archivo nuevo", "New archive").to_string()),
                Doc::Archive(opened) => (Icon::Locked, opened.path.display().to_string()),
                Doc::Plain(zip) => (Icon::Compress, zip.path.display().to_string()),
            };
            icons::small(ui, icon);
            let can_find = self.entries().is_some();
            let find_width = if can_find { 190.0 } else { 0.0 };
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.set_width((ui.available_width() - find_width).max(120.0));
                    if self.folder.is_empty() {
                        ui.label(text);
                    } else {
                        ui.label(format!("{text} / {}", self.folder.trim_end_matches('/')));
                    }
                });
            if can_find {
                let edit = egui::TextEdit::singleline(&mut self.find)
                    .hint_text(tr.t("Buscar en el archivo", "Find in the archive"))
                    .desired_width(f32::INFINITY);
                if ui.add(edit).changed() {
                    action = Some(Action::FindChanged);
                }
            }
        });
        action
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let tr = self.tr();
        ui.horizontal(|ui| {
            if self.contents_are_hidden() {
                ui.label(tr.t("Nombres ocultos", "Names hidden"));
            } else if self.selected.is_empty() {
                let total: u64 = self.rows.iter().map(|row| row.size).sum();
                ui.label(format!(
                    "{} {} · {}",
                    self.rows.len(),
                    tr.t("elementos", "items"),
                    human_size(total)
                ));
            } else {
                let chosen: u64 = self
                    .selected
                    .iter()
                    .filter_map(|&i| self.rows.get(i))
                    .map(|row| row.size)
                    .sum();
                ui.label(format!(
                    "{} {} · {}",
                    self.selected.len(),
                    tr.t("seleccionados", "selected"),
                    human_size(chosen)
                ));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                places::connection_status(ui, self.map.is_online(), tr);
                match &self.doc {
                    Doc::Archive(opened) => {
                        ui.label(level_summary(&opened.header, tr));
                    }
                    Doc::Plain(zip) if zip.needs_password => {
                        ui.label(tr.t(
                            "ZIP convencional con contraseña",
                            "Conventional ZIP with a password",
                        ));
                    }
                    Doc::Plain(_) => {
                        ui.label(tr.t("ZIP convencional sin protección", "Conventional ZIP, unprotected"));
                    }
                    Doc::New(_) => {}
                }
            });
        });
    }

    fn row_menu(&self, ui: &mut egui::Ui, index: usize) -> Option<Action> {
        let tr = self.tr();
        let mut action = None;
        let mut item = |ui: &mut egui::Ui, label: &str, value: Action| {
            if ui.button(label).clicked() {
                action = Some(value);
                ui.close();
            }
        };
        item(ui, tr.t("Abrir", "Open"), Action::OpenRow(index));
        match &self.doc {
            Doc::New(_) => item(
                ui,
                tr.t("Quitar de la lista", "Remove from the list"),
                Action::Remove,
            ),
            _ => {
                item(ui, tr.t("Extraer en…", "Extract to…"), Action::Extract);
                item(
                    ui,
                    tr.t("Borrar del archivo…", "Delete from the archive…"),
                    Action::Remove,
                );
                item(ui, tr.t("Comprobar el archivo", "Test the archive"), Action::Test);
            }
        }
        action
    }

    fn file_list(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let tr = self.tr();
        let hidden = self.contents_are_hidden();
        if self.rows.is_empty() && self.folder.is_empty() && !hidden && !self.finding() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(tr.t("Arrastra aquí archivos o carpetas", "Drop files or folders here"))
                        .weak(),
                );
            });
            return None;
        }
        let has_parent_row = !self.folder.is_empty() && !self.finding();
        let leading_rows = usize::from(has_parent_row || hidden);
        let mut action = None;
        let mut clicked = None;
        let mut context_target = None;
        TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(240.0).clip(true))
            .column(Column::initial(90.0))
            .column(Column::initial(120.0))
            .column(Column::initial(140.0))
            .header(22.0, |mut header| {
                for (title, by) in [
                    (tr.t("Nombre", "Name"), SortBy::Name),
                    (tr.t("Tamaño", "Size"), SortBy::Size),
                    (tr.t("Tipo", "Type"), SortBy::Kind),
                    (tr.t("Modificado", "Modified"), SortBy::Modified),
                ] {
                    header.col(|ui| {
                        let label =
                            egui::Label::new(RichText::new(title).strong()).sense(egui::Sense::click());
                        if ui.add(label).clicked() {
                            action = Some(Action::SortBy(by));
                        }
                        if self.sort.by == by {
                            icons::sort_arrow(ui, self.sort.ascending);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.rows.len() + leading_rows, |mut table_row| {
                    let Some(index) = table_row.index().checked_sub(leading_rows) else {
                        table_row.col(|ui| {
                            if hidden {
                                icons::small(ui, Icon::Locked);
                                ui.label(tr.t(
                                    "Contenido cifrado (doble clic para verlo)",
                                    "Encrypted contents (double-click to show)",
                                ));
                            } else {
                                icons::small(ui, Icon::Folder);
                                ui.label("..");
                            }
                        });
                        for _ in 0..3 {
                            table_row.col(|_| {});
                        }
                        if table_row.response().double_clicked() {
                            action = Some(if hidden { Action::Unlock } else { Action::Up });
                        }
                        return;
                    };
                    let row = &self.rows[index];
                    table_row.set_selected(self.selected.contains(&index));
                    table_row.col(|ui| {
                        icons::small(ui, if row.is_dir { Icon::Folder } else { Icon::File });
                        ui.label(&row.name);
                    });
                    table_row.col(|ui| {
                        ui.label(human_size(row.size));
                    });
                    table_row.col(|ui| {
                        ui.label(kind_of(&row.name, row.is_dir, tr));
                    });
                    table_row.col(|ui| {
                        ui.label(
                            row.modified
                                .map(|t| format_time(t, self.local_offset))
                                .unwrap_or_default(),
                        );
                    });
                    let response = table_row.response();
                    if response.double_clicked() {
                        action = Some(Action::OpenRow(index));
                    } else if response.clicked() {
                        clicked = Some(index);
                    }
                    if response.secondary_clicked() {
                        context_target = Some(index);
                    }
                    response.context_menu(|ui| {
                        if let Some(chosen) = self.row_menu(ui, index) {
                            action = Some(chosen);
                        }
                    });
                });
            });
        if let Some(index) = context_target {
            if !self.selected.contains(&index) {
                self.selected = BTreeSet::from([index]);
            }
        }
        if let Some(index) = clicked {
            let additive = ui.input(|i| i.modifiers.command || i.modifiers.shift);
            if additive {
                if !self.selected.remove(&index) {
                    self.selected.insert(index);
                }
            } else {
                let was_only_selection = self.selected.len() == 1 && self.selected.contains(&index);
                self.selected.clear();
                if !was_only_selection {
                    self.selected.insert(index);
                }
            }
        }
        action
    }

    fn show_dialog(&mut self, ctx: &egui::Context) {
        let tr = self.tr();
        match std::mem::replace(&mut self.dialog, Dialog::None) {
            Dialog::None => {}
            Dialog::Compress(mut form) => match dialogs::compress(ctx, tr, &mut form) {
                Some(true) => {
                    form.error = None;
                    self.start(Pending::Compress(form), ctx);
                }
                Some(false) => {}
                None => self.dialog = Dialog::Compress(form),
            },
            Dialog::Extract(mut form) => match dialogs::extract(ctx, tr, &mut form) {
                Some(true) => {
                    form.error = None;
                    self.start(Pending::Extract(form), ctx);
                }
                Some(false) => {}
                None => self.dialog = Dialog::Extract(form),
            },
            Dialog::ConfirmRemoval(edit) => match dialogs::confirm_removal(ctx, tr, edit.remove.len()) {
                Some(true) => self.start(Pending::Edit(edit), ctx),
                Some(false) => {}
                None => self.dialog = Dialog::ConfirmRemoval(edit),
            },
            Dialog::Working => {
                dialogs::working(ctx, tr, &self.progress, self.job_secret.is_some());
                self.dialog = Dialog::Working;
            }
            Dialog::Finished(outcome) => {
                if !dialogs::finished(ctx, tr, self.lang(), &outcome) {
                    self.dialog = Dialog::Finished(outcome);
                }
            }
            Dialog::Info => {
                if let Doc::Archive(opened) = &self.doc {
                    if !dialogs::archive_info(ctx, tr, opened) {
                        self.dialog = Dialog::Info;
                    }
                }
            }
            Dialog::OfflineMaps => {
                if !dialogs::offline_maps(ctx, tr) {
                    self.dialog = Dialog::OfflineMaps;
                }
            }
            Dialog::About => {
                if !dialogs::about(ctx, tr, &self.logo) {
                    self.dialog = Dialog::About;
                }
            }
            Dialog::Message(text) => {
                if !dialogs::message(ctx, tr, &text) {
                    self.dialog = Dialog::Message(text);
                }
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(dir) = &self.preview_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn create_options(level: Level, form: &CompressForm) -> CreateOptions {
    CreateOptions {
        level,
        hide_names: form.hide_names,
        overwrite: true,
        kdf: None,
        compression: form.compression,
    }
}

fn is_zip(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_worker();
        self.poll_search();
        let ctx = ui.ctx().clone();
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.take_paths(dropped, &ctx);
        }

        let mut action = None;
        let browsing = self.view == View::Browser;
        egui::Panel::top("menu").show(ui, |ui| {
            ui.add_enabled_ui(browsing, |ui| action = self.menu_bar(ui));
        });
        if browsing {
            egui::Panel::top("toolbar").show(ui, |ui| {
                ui.add_space(2.0);
                action = action.or(self.toolbar(ui));
                ui.add_space(2.0);
            });
            egui::Panel::top("address").show(ui, |ui| {
                ui.add_space(3.0);
                action = action.or(self.address_bar(ui));
                ui.add_space(3.0);
            });
            egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
            egui::CentralPanel::default().show(ui, |ui| action = action.or(self.file_list(ui)));
        } else {
            egui::Panel::left("places").exact_size(460.0).show(ui, |ui| {
                ui.add_space(6.0);
                self.places_panel(ui);
            });
            egui::CentralPanel::default().show(ui, |ui| action = action.or(self.places_map(ui)));
        }

        let typing = ctx.egui_wants_keyboard_input();
        if browsing && !typing && matches!(self.dialog, Dialog::None) {
            let (delete, back) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Delete),
                    i.key_pressed(egui::Key::Backspace),
                )
            });
            if delete {
                action = Some(Action::Remove);
            } else if back && !self.folder.is_empty() {
                action = Some(Action::Up);
            }
        }
        if let Some(action) = action {
            self.perform(action, &ctx);
        }
        self.show_dialog(&ctx);
    }
}
