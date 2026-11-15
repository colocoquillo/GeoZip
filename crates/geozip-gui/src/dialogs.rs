use std::path::Path;

use eframe::egui::{self, RichText};
use geozip_core::archive::Compression;
use geozip_core::{Lang, Level, Wordlist};

use crate::app::{CompressForm, ExtractForm, Opened, Outcome, Progress, ERR_RED, WARN};
use crate::icons;
use crate::text::{human_size, level_summary, Tr};

const BUTTON_SIZE: egui::Vec2 = egui::vec2(96.0, 26.0);
const PMTILES_TOOL_URL: &str = "https://github.com/protomaps/go-pmtiles/releases";
const PMTILES_BUILDS_URL: &str = "https://maps.protomaps.com/builds/";
const PMTILES_GUIDE_URL: &str = "https://docs.protomaps.com/guide/getting-started";
const BOUNDING_BOX_URL: &str = "http://bboxfinder.com/";
const PMTILES_EXAMPLE: &str =
    "pmtiles extract https://build.protomaps.com/AAAAMMDD.pmtiles madrid.pmtiles --bbox=-3.89,40.31,-3.52,40.56";

fn modal(ctx: &egui::Context, id: &str, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    egui::Modal::new(egui::Id::unique(id)).show(ctx, |ui| {
        ui.label(RichText::new(title).strong().size(15.0));
        ui.separator();
        content(ui);
    });
}

fn accept_cancel(ui: &mut egui::Ui, tr: Tr, can_accept: bool) -> Option<bool> {
    let mut decision = None;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let accept = egui::Button::new(tr.t("Aceptar", "OK")).min_size(BUTTON_SIZE);
        if ui.add_enabled(can_accept, accept).clicked() {
            decision = Some(true);
        }
        if ui
            .add(egui::Button::new(tr.t("Cancelar", "Cancel")).min_size(BUTTON_SIZE))
            .clicked()
        {
            decision = Some(false);
        }
    });
    decision
}

fn close_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add_space(8.0);
    ui.add(egui::Button::new(label).min_size(BUTTON_SIZE)).clicked()
}

fn path_box(ui: &mut egui::Ui, path: &Path) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .inner_margin(egui::Margin::symmetric(6, 3))
        .show(ui, |ui| {
            ui.set_width(440.0);
            ui.label(path.display().to_string());
        });
}

fn option_with_info(ui: &mut egui::Ui, option: impl FnOnce(&mut egui::Ui), help: &str) {
    ui.horizontal(|ui| {
        option(ui);
        icons::info(ui, help);
    });
}

fn password_field(ui: &mut egui::Ui, tr: Tr, password: &mut String) {
    ui.horizontal(|ui| {
        ui.label(tr.t("Contraseña del ZIP", "ZIP password"));
        ui.add(
            egui::TextEdit::singleline(password)
                .password(true)
                .desired_width(220.0),
        );
    });
}

fn protection_choice(ui: &mut egui::Ui, tr: Tr, form: &mut CompressForm) {
    let before = form.level;
    if form.convert.is_none() {
        option_with_info(
            ui,
            |ui| {
                ui.radio_value(
                    &mut form.level,
                    None,
                    tr.t("Sin protección (ZIP normal)", "No protection (ordinary ZIP)"),
                );
            },
            tr.t(
                "Solo comprime. Cualquiera puede abrir el archivo con cualquier programa.",
                "Compression only. Anyone can open the archive with any program.",
            ),
        );
    }
    option_with_info(
        ui,
        |ui| {
            ui.radio_value(
                &mut form.level,
                Some(Level::Alto),
                tr.t("Alto: 5 lugares", "High: 5 places"),
            );
        },
        tr.t(
            "Solo lugares. Fuerte frente a desconocidos, más débil frente a quien te conoce bien.",
            "Places only. Strong against strangers, weaker against someone who knows you well.",
        ),
    );
    option_with_info(
        ui,
        |ui| {
            let with_words = matches!(form.level, Some(Level::Extremo | Level::ExtremoDoble));
            let label = tr.t(
                "Extremo: 5 lugares y 1 palabra por lugar",
                "Extreme: 5 places and 1 word per place",
            );
            if ui.radio(with_words, label).clicked() && !with_words {
                form.level = Some(Level::Extremo);
            }
        },
        tr.t(
            "El programa elige las palabras al azar. Resiste aunque alguien conozca tus lugares.",
            "The program picks the words at random. Holds even if someone knows your places.",
        ),
    );
    option_with_info(
        ui,
        |ui| {
            ui.add_space(18.0);
            let with_words = matches!(form.level, Some(Level::Extremo | Level::ExtremoDoble));
            let mut two_words = form.level == Some(Level::ExtremoDoble);
            let checkbox = egui::Checkbox::new(&mut two_words, tr.t("Dos palabras por lugar", "Two words per place"));
            if ui.add_enabled(with_words, checkbox).changed() {
                form.level = Some(if two_words { Level::ExtremoDoble } else { Level::Extremo });
            }
        },
        tr.t(
            "Opcional. Con una palabra por lugar ya es muy difícil de romper; con dos resiste incluso a un atacante con muchos recursos que conozca tus lugares exactos. A cambio hay que memorizar el doble.",
            "Optional. One word per place is already very hard to break; two holds even against a well-resourced attacker who knows your exact places. In exchange there is twice as much to memorise.",
        ),
    );
    if form.level != before {
        form.use_default_output();
    }
}

pub fn compress(ctx: &egui::Context, tr: Tr, form: &mut CompressForm) -> Option<bool> {
    let title = if form.convert.is_some() {
        tr.t("Convertir a archivo protegido", "Convert to a protected archive")
    } else {
        tr.t("Comprimir", "Compress")
    };
    let mut decision = None;
    modal(ctx, "compress", title, |ui| {
        if let Some(error) = &form.error {
            ui.colored_label(ERR_RED, error);
        }
        ui.horizontal(|ui| {
            ui.label(tr.t("Archivo de destino", "Destination archive"));
            if ui.button(tr.t("Examinar…", "Browse…")).clicked() {
                let mut dialog = rfd::FileDialog::new().add_filter("ZIP", &["zip"]);
                if let Some(dir) = form.output.parent() {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(name) = form.output.file_name() {
                    dialog = dialog.set_file_name(name.to_string_lossy());
                }
                if let Some(path) = dialog.save_file() {
                    form.output = path;
                    form.output_chosen_by_user = true;
                }
            }
        });
        path_box(ui, &form.output);
        if form.output.exists() {
            ui.colored_label(
                WARN,
                tr.t(
                    "Ya existe y se sustituirá",
                    "It already exists and will be replaced",
                ),
            );
        }

        ui.add_space(8.0);
        ui.label(RichText::new(tr.t("Protección", "Protection")).strong());
        protection_choice(ui, tr, form);

        ui.add_space(8.0);
        option_with_info(
            ui,
            |ui| {
                let checkbox = egui::Checkbox::new(
                    &mut form.hide_names,
                    tr.t("Ocultar los nombres de los archivos", "Hide file names"),
                );
                ui.add_enabled(form.level.is_some(), checkbox);
            },
            tr.t(
                "El contenido se guarda como un ZIP dentro de otro, así que desde fuera no se ven los nombres. Necesita tanta memoria libre como ocupe el contenido.",
                "Contents are stored as a ZIP inside another, so names are not visible from outside. It needs as much free memory as the contents take.",
            ),
        );

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(tr.t("Compresión", "Compression"));
            let name = |compression: Compression| match compression {
                Compression::Store => tr.t("Sin comprimir", "Store"),
                Compression::Fast => tr.t("Rápida", "Fast"),
                Compression::Normal => tr.t("Normal", "Normal"),
                Compression::Best => tr.t("Máxima", "Best"),
            };
            egui::ComboBox::from_id_salt("compression")
                .selected_text(name(form.compression))
                .show_ui(ui, |ui| {
                    for choice in [
                        Compression::Store,
                        Compression::Fast,
                        Compression::Normal,
                        Compression::Best,
                    ] {
                        ui.selectable_value(&mut form.compression, choice, name(choice));
                    }
                });
        });

        let mut password_missing = false;
        if let Some(old) = form.convert.as_mut().filter(|old| old.needs_password) {
            ui.add_space(8.0);
            password_field(ui, tr, &mut old.password);
            password_missing = old.password.is_empty();
        }
        decision = accept_cancel(ui, tr, !password_missing);
    });
    decision
}

pub fn extract(ctx: &egui::Context, tr: Tr, form: &mut ExtractForm) -> Option<bool> {
    let verifying = form.scope.verify_only;
    let title = if verifying {
        tr.t("Comprobar", "Test")
    } else {
        tr.t("Extraer en", "Extract to")
    };
    let mut decision = None;
    modal(ctx, "extract", title, |ui| {
        if let Some(error) = &form.error {
            ui.colored_label(ERR_RED, error);
        }
        if !verifying {
            ui.horizontal(|ui| {
                ui.label(tr.t("Carpeta de destino", "Destination folder"));
                if ui.button(tr.t("Examinar…", "Browse…")).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        form.out_dir = dir;
                    }
                }
            });
            path_box(ui, &form.out_dir);
            ui.add_space(4.0);
            let what = match form.scope.only.len() {
                0 => tr
                    .t("Se extrae todo el contenido", "Everything will be extracted")
                    .to_string(),
                1 => tr
                    .t(
                        "Se extrae el elemento seleccionado",
                        "The selected item will be extracted",
                    )
                    .to_string(),
                n => format!(
                    "{} {n} {}",
                    tr.t("Se extraen los", "The"),
                    tr.t("elementos seleccionados", "selected items will be extracted")
                ),
            };
            ui.label(what);
            ui.checkbox(
                &mut form.overwrite,
                tr.t(
                    "Sustituir los archivos que ya existan",
                    "Replace files that already exist",
                ),
            );
        }
        let mut password_missing = false;
        if let Some(password) = &mut form.password {
            ui.add_space(4.0);
            password_field(ui, tr, password);
            password_missing = password.is_empty();
        }
        decision = accept_cancel(ui, tr, !password_missing);
    });
    decision
}

pub fn confirm_removal(ctx: &egui::Context, tr: Tr, count: usize) -> Option<bool> {
    let mut decision = None;
    modal(
        ctx,
        "confirm-removal",
        tr.t("Borrar del archivo", "Delete from the archive"),
        |ui| {
            ui.set_max_width(420.0);
            ui.label(match count {
                1 => tr
                    .t(
                        "Se borrará el elemento seleccionado.",
                        "The selected item will be deleted.",
                    )
                    .to_string(),
                n => format!(
                    "{} {n} {}",
                    tr.t("Se borrarán los", "The"),
                    tr.t("elementos seleccionados.", "selected items will be deleted.")
                ),
            });
            ui.label(tr.t(
                "El archivo se reescribe sin ellos y no se puede deshacer.",
                "The archive is rewritten without them and this cannot be undone.",
            ));
            decision = accept_cancel(ui, tr, true);
        },
    );
    decision
}

pub fn working(ctx: &egui::Context, tr: Tr, progress: &Progress, deriving_key: bool) {
    modal(ctx, "working", tr.t("Trabajando", "Working"), |ui| {
        ui.set_width(380.0);
        if progress.total > 0 {
            let fraction = progress.done as f32 / progress.total as f32;
            ui.add(egui::ProgressBar::new(fraction).show_percentage());
            ui.label(RichText::new(&progress.current).weak());
        } else if deriving_key {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr.t("Derivando la clave…", "Deriving the key…"));
                icons::info(
                    ui,
                    tr.t(
                        "Este paso es lento a propósito: es lo que frena a quien intente adivinar tus lugares.",
                        "This step is slow on purpose: it is what slows down anyone trying to guess your places.",
                    ),
                );
            });
        } else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr.t("Preparando…", "Preparing…"));
            });
        }
    });
}

pub fn finished(ctx: &egui::Context, tr: Tr, lang: Lang, outcome: &Outcome) -> bool {
    let title = match outcome {
        Outcome::Created { protected: true, .. } => tr.t("Archivo protegido", "Archive protected"),
        Outcome::Created { protected: false, .. } => tr.t("Archivo comprimido", "Archive compressed"),
        Outcome::Extracted { .. } => tr.t("Extracción terminada", "Extraction finished"),
        Outcome::Verified { .. } => tr.t("Comprobación correcta", "Test passed"),
    };
    let mut close = false;
    modal(ctx, "finished", title, |ui| {
        ui.set_max_width(460.0);
        match outcome {
            Outcome::Created {
                output,
                files,
                bytes,
                skipped,
                words,
                protected,
            } => {
                ui.label(output.display().to_string());
                ui.label(format!(
                    "{files} {} · {}",
                    tr.t("archivos", "files"),
                    human_size(*bytes)
                ));
                for name in skipped {
                    ui.colored_label(WARN, format!("{} {name}", tr.t("No incluido:", "Not included:")));
                }
                if words.iter().any(|w| !w.is_empty()) {
                    ui.add_space(6.0);
                    egui::Grid::new("words").num_columns(2).show(ui, |ui| {
                        for (index, ids) in words.iter().enumerate() {
                            let text: Vec<&str> =
                                ids.iter().map(|&id| Wordlist::get().word(id, lang)).collect();
                            ui.label(format!("{} {}", tr.t("Lugar", "Place"), index + 1));
                            ui.label(RichText::new(text.join("  +  ")).strong());
                            ui.end_row();
                        }
                    });
                }
                if *protected {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            WARN,
                            tr.t("Si lo olvidas no se puede recuperar", "If you forget it, it cannot be recovered"),
                        );
                        icons::info(
                            ui,
                            tr.t(
                                "No se guarda nada fuera del archivo. Si olvidas los lugares, su orden o las palabras, el contenido se pierde. Comprueba que sabes abrirlo antes de borrar los originales.",
                                "Nothing is stored outside the archive. If you forget the places, their order or the words, the contents are lost. Check that you can open it before deleting the originals.",
                            ),
                        );
                    });
                }
            }
            Outcome::Verified { extracted } => {
                ui.label(format!(
                    "{} {} · {}",
                    extracted.files,
                    tr.t("archivos", "files"),
                    human_size(extracted.bytes)
                ));
                ui.label(tr.t(
                    "Todo el contenido se ha podido leer y descifrar. No se ha escrito nada en disco.",
                    "All the contents could be read and decrypted. Nothing was written to disk.",
                ));
            }
            Outcome::Extracted { out_dir, extracted } => {
                ui.label(format!(
                    "{} {} · {}",
                    extracted.files,
                    tr.t("archivos", "files"),
                    human_size(extracted.bytes)
                ));
                ui.label(out_dir.display().to_string());
                for name in &extracted.skipped {
                    ui.colored_label(WARN, format!("{} {name}", tr.t("No extraído:", "Not extracted:")));
                }
            }
        }
        close = close_button(ui, tr.t("Aceptar", "OK"));
    });
    close
}

pub fn archive_info(ctx: &egui::Context, tr: Tr, opened: &Opened) -> bool {
    let header = &opened.header;
    let mut close = false;
    modal(ctx, "info", tr.t("Información", "Information"), |ui| {
        egui::Grid::new("info")
            .num_columns(2)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                let mut line = |label: &str, value: String| {
                    ui.label(label);
                    ui.label(value);
                    ui.end_row();
                };
                line(tr.t("Archivo", "Archive"), opened.path.display().to_string());
                line(tr.t("Nivel", "Level"), level_summary(header, tr));
                line(tr.t("Cifrado", "Encryption"), "ZIP AES-256 (AE-2)".to_string());
                line(
                    tr.t("Derivación de clave", "Key derivation"),
                    format!(
                        "Argon2id · {} MiB · {} {} · {} {}",
                        header.kdf.m_kib / 1024,
                        header.kdf.t,
                        tr.t("pasadas", "passes"),
                        header.kdf.p,
                        tr.t("hilos", "threads")
                    ),
                );
                let names = if header.hidden_names {
                    tr.t("ocultos", "hidden")
                } else {
                    tr.t("visibles", "visible")
                };
                line(tr.t("Nombres de archivo", "File names"), names.to_string());
            });
        close = close_button(ui, tr.t("Cerrar", "Close"));
    });
    close
}

pub fn offline_maps(ctx: &egui::Context, tr: Tr) -> bool {
    let mut close = false;
    modal(
        ctx,
        "offline-maps",
        tr.t("Mapas sin conexión", "Offline maps"),
        |ui| {
            ui.set_max_width(520.0);
            ui.label(tr.t(
            "GeoZip puede usar un mapa guardado en tu equipo, en formato .pmtiles, para no consultar ningún servidor al elegir lugares. El mapa se recorta de la copia mundial gratuita de Protomaps con su herramienta «pmtiles».",
            "GeoZip can use a map stored on your computer, in .pmtiles format, so that no server is contacted while you pick places. The map is cut out of Protomaps' free world copy with their “pmtiles” tool.",
        ));

            ui.add_space(8.0);
            ui.label(RichText::new(tr.t("Tamaño aproximado", "Approximate size")).strong());
            egui::Grid::new("map-sizes")
                .num_columns(2)
                .spacing([16.0, 3.0])
                .show(ui, |ui| {
                    for (area, size) in [
                        (
                            tr.t(
                                "El mundo, sin detalle de calles",
                                "The world, without street detail",
                            ),
                            "60 MB",
                        ),
                        (tr.t("Una ciudad grande", "A large city"), "20 – 100 MB"),
                        (tr.t("Un país como España", "A country like Spain"), "1 – 2 GB"),
                        (tr.t("El planeta entero", "The whole planet"), "> 100 GB"),
                    ] {
                        ui.label(area);
                        ui.label(size);
                        ui.end_row();
                    }
                });

            ui.add_space(8.0);
            ui.label(RichText::new(tr.t("Cómo conseguirlo", "How to get one")).strong());
            ui.horizontal_wrapped(|ui| {
                ui.label(tr.t("1. Descarga la herramienta:", "1. Download the tool:"));
                ui.hyperlink_to("go-pmtiles", PMTILES_TOOL_URL);
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(tr.t(
                    "2. Mira la fecha de la última copia mundial:",
                    "2. Look up the date of the latest world copy:",
                ));
                ui.hyperlink_to("maps.protomaps.com/builds", PMTILES_BUILDS_URL);
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(tr.t(
                    "3. Obtén las coordenadas de tu zona:",
                    "3. Get the coordinates of your area:",
                ));
                ui.hyperlink_to("bboxfinder.com", BOUNDING_BOX_URL);
            });
            ui.label(tr.t(
                "4. Recorta tu zona, por ejemplo Madrid:",
                "4. Cut out your area, for example Madrid:",
            ));
            let mut command = PMTILES_EXAMPLE.to_string();
            ui.add(
                egui::TextEdit::multiline(&mut command)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .code_editor(),
            );
            ui.label(tr.t(
                "5. Cárgalo en GeoZip desde Opciones > Mapa > Sin conexión con mapa .pmtiles.",
                "5. Load it in GeoZip from Options > Map > Offline with a .pmtiles map.",
            ));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(tr.t("Guía completa:", "Full guide:"));
                ui.hyperlink_to("docs.protomaps.com", PMTILES_GUIDE_URL);
            });
            close = close_button(ui, tr.t("Cerrar", "Close"));
        },
    );
    close
}

pub fn about(ctx: &egui::Context, tr: Tr, logo: &egui::TextureHandle) -> bool {
    let mut close = false;
    modal(ctx, "about", tr.t("Acerca de GeoZip", "About GeoZip"), |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(64.0, 64.0)));
            ui.vertical(|ui| {
                ui.label(RichText::new("GeoZip").strong().size(18.0));
                ui.label(format!(
                    "{} {}",
                    tr.t("Versión", "Version"),
                    env!("CARGO_PKG_VERSION")
                ));
                ui.label(tr.t(
                    "ZIP con AES-256 cuya clave son lugares",
                    "AES-256 ZIP whose key is a set of places",
                ));
                ui.label(tr.t("Licencia MIT · experimental", "MIT licence · experimental"));
            });
        });
        close = close_button(ui, tr.t("Cerrar", "Close"));
    });
    close
}

pub fn message(ctx: &egui::Context, tr: Tr, text: &str) -> bool {
    let mut close = false;
    modal(ctx, "message", "GeoZip", |ui| {
        ui.set_max_width(420.0);
        ui.label(text);
        close = close_button(ui, tr.t("Aceptar", "OK"));
    });
    close
}
