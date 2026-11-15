use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use geozip_core::archive::{self, Compression, CreateOptions, Created, Event};
use geozip_core::grid::{self, LatLon};
use geozip_core::{derive_password, random_words, Error, Header, Lang, Level, Secret, Wordlist};

#[derive(Parser)]
#[command(
    name = "geozip",
    version,
    about = "ZIP con AES-256 cuya clave son lugares del mapa (y palabras) en vez de una contraseña"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Crea un ZIP protegido a partir de archivos y carpetas")]
    Create {
        #[arg(help = "ZIP de salida")]
        output: PathBuf,
        #[arg(required = true, help = "Archivos y carpetas que se incluyen")]
        inputs: Vec<PathBuf>,
        #[arg(long, help = "Solo comprime: ZIP normal sin protección")]
        plain: bool,
        #[command(flatten)]
        new: NewArchive,
    },
    #[command(about = "Abre un ZIP protegido y extrae su contenido")]
    Open {
        archive: PathBuf,
        #[arg(short, long, default_value = ".", help = "Carpeta de destino")]
        output: PathBuf,
        #[arg(long, help = "Sustituye los archivos que ya existan")]
        overwrite: bool,
        #[command(flatten)]
        secret: SecretArgs,
    },
    #[command(about = "Muestra la parte pública de un ZIP protegido, sin pedir el secreto")]
    Info { archive: PathBuf },
    #[command(about = "Convierte un ZIP corriente, con o sin contraseña, al sistema nuevo")]
    Migrate {
        #[arg(help = "ZIP original")]
        source: PathBuf,
        #[arg(help = "ZIP de salida")]
        output: PathBuf,
        #[arg(
            long,
            help = "Contraseña del ZIP original; si falta y hace falta, se pregunta"
        )]
        old_password: Option<String>,
        #[command(flatten)]
        new: NewArchive,
    },
    #[command(about = "Muestra la contraseña AES derivada, para abrir el ZIP con 7-Zip u otra herramienta")]
    Key {
        archive: PathBuf,
        #[command(flatten)]
        secret: SecretArgs,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum CompressionArg {
    Store,
    Fast,
    Normal,
    Best,
}

impl From<CompressionArg> for Compression {
    fn from(arg: CompressionArg) -> Self {
        match arg {
            CompressionArg::Store => Compression::Store,
            CompressionArg::Fast => Compression::Fast,
            CompressionArg::Normal => Compression::Normal,
            CompressionArg::Best => Compression::Best,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum LevelArg {
    #[value(help = "5 lugares")]
    Alto,
    #[value(help = "5 lugares y 1 palabra por lugar")]
    Extremo,
}

#[derive(Args)]
struct NewArchive {
    #[arg(short, long, value_enum, default_value = "alto", help = "Nivel de seguridad")]
    level: LevelArg,
    #[arg(long, help = "Con el nivel extremo, 2 palabras por lugar en vez de 1")]
    two_words: bool,
    #[arg(long, help = "Oculta también los nombres de los archivos (ZIP dentro de ZIP)")]
    hide_names: bool,
    #[arg(long, help = "Sustituye el ZIP de salida si ya existe")]
    overwrite: bool,
    #[arg(long, value_enum, default_value = "normal", help = "Nivel de compresión")]
    compression: CompressionArg,
    #[command(flatten)]
    secret: SecretArgs,
}

#[derive(Args)]
struct SecretArgs {
    #[arg(
        long,
        help = "Lugares en orden: \"lat,lon;lat,lon;…\". Si se omite, se preguntan uno a uno"
    )]
    places: Option<String>,
    #[arg(
        long,
        help = "Palabras por lugar: \"a,b;c,d;…\". Al crear, omítelas para que se generen al azar"
    )]
    words: Option<String>,
    #[arg(long, default_value = "es", value_parser = parse_lang, help = "Idioma de las palabras (es, en)")]
    lang: Lang,
}

fn parse_lang(code: &str) -> Result<Lang, String> {
    Lang::from_code(code).ok_or_else(|| format!("idioma no disponible: {code} (es, en)"))
}

type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

fn prompt(question: &str) -> AnyResult<String> {
    eprint!("{question}");
    io::stderr().flush()?;
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line)? == 0 {
        return Err("entrada terminada antes de tiempo".into());
    }
    Ok(line.trim().to_string())
}

fn read_places(args: &SecretArgs, count: usize) -> AnyResult<Vec<LatLon>> {
    let places = match &args.places {
        Some(text) => grid::parse_places(text)?,
        None => {
            eprintln!("Introduce {count} lugares en orden, como «latitud,longitud» (4 decimales bastan).");
            (1..=count)
                .map(|i| Ok(LatLon::parse(&prompt(&format!("Lugar {i}: "))?)?))
                .collect::<AnyResult<Vec<_>>>()?
        }
    };
    if places.len() != count {
        return Err(Error::PlaceCount {
            expected: count,
            got: places.len(),
        }
        .into());
    }
    Ok(places)
}

fn parse_word_group(text: &str, lang: Lang) -> AnyResult<Vec<u16>> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            Wordlist::get()
                .lookup_any(w, lang)
                .ok_or_else(|| Error::UnknownWord(w.to_string()).into())
        })
        .collect()
}

fn read_words(args: &SecretArgs, places: usize, per_place: usize) -> AnyResult<Vec<Vec<u16>>> {
    if per_place == 0 {
        return Ok(vec![Vec::new(); places]);
    }
    let groups: Vec<Vec<u16>> = match &args.words {
        Some(text) => text
            .split(';')
            .filter(|g| !g.trim().is_empty())
            .map(|g| parse_word_group(g, args.lang))
            .collect::<AnyResult<_>>()?,
        None => (1..=places)
            .map(|i| {
                let q = format!("Palabras del lugar {i} ({per_place}, separadas por coma): ");
                parse_word_group(&prompt(&q)?, args.lang)
            })
            .collect::<AnyResult<_>>()?,
    };
    if groups.len() != places {
        return Err(format!(
            "se esperaban palabras para {places} lugares y hay {}",
            groups.len()
        )
        .into());
    }
    Ok(groups)
}

fn secret_for(header: &Header, args: &SecretArgs) -> AnyResult<Secret> {
    let places = read_places(args, header.places())?;
    let words = read_words(args, places.len(), header.words_per_place)?;
    Ok(Secret::new(places, words))
}

fn secret_for_new(level: Level, args: &SecretArgs) -> AnyResult<(Secret, bool)> {
    let places = read_places(args, level.places())?;
    let per_place = level.words_per_place();
    let generated = per_place > 0 && args.words.is_none();
    let words = if generated {
        places
            .iter()
            .map(|_| random_words(per_place))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        read_words(args, places.len(), per_place)?
    };
    Ok((Secret::new(places, words), generated))
}

fn progress() -> impl FnMut(Event<'_>) {
    let tty = io::stderr().is_terminal();
    let mut last_percent = u64::MAX;
    move |event| match event {
        Event::DerivingKey => eprintln!("Derivando la clave (Argon2id)…"),
        Event::Entry(_) => {}
        Event::Bytes { done, total } => {
            let percent = if total == 0 { 100 } else { done * 100 / total };
            if tty && percent != last_percent {
                last_percent = percent;
                eprint!("\r{percent:3} %");
                if percent == 100 {
                    eprintln!();
                }
            }
        }
    }
}

fn describe(header: &Header) -> String {
    let level = match header.level() {
        Some(Level::Alto) => "Alto",
        Some(Level::Extremo | Level::ExtremoDoble) => "Extremo",
        None => "personalizado",
    };
    let words = match header.words_per_place {
        0 => String::new(),
        1 => " + 1 palabra por lugar".to_string(),
        n => format!(" + {n} palabras por lugar"),
    };
    let names = if header.hidden_names {
        "ocultos"
    } else {
        "visibles"
    };
    format!(
        "Nivel {level}: {} lugares{words}. Nombres de archivo {names}.",
        header.places()
    )
}

fn report_created(created: &Created, secret: &Secret, show_words: bool, lang: Lang) {
    println!("{}", describe(&created.header));
    println!("{} archivos, {} bytes.", created.files, created.bytes);
    for name in &created.skipped {
        println!("Omitido (no es archivo ni carpeta): {name}");
    }
    if show_words {
        println!();
        println!(
            "PALABRAS GENERADAS. Memorízalas junto a cada lugar: sin ellas el archivo no se puede abrir."
        );
        for (i, ids) in secret.words.iter().enumerate() {
            let words: Vec<&str> = ids.iter().map(|&id| Wordlist::get().word(id, lang)).collect();
            println!("  Lugar {}: {}", i + 1, words.join(" + "));
        }
    }
}

fn options(new: &NewArchive) -> CreateOptions {
    CreateOptions {
        level: match new.level {
            LevelArg::Alto => Level::Alto,
            LevelArg::Extremo if new.two_words => Level::ExtremoDoble,
            LevelArg::Extremo => Level::Extremo,
        },
        hide_names: new.hide_names,
        overwrite: new.overwrite,
        kdf: None,
        compression: new.compression.into(),
    }
}

fn run(cli: Cli) -> AnyResult<()> {
    if let Command::Create { new, .. } | Command::Migrate { new, .. } = &cli.command {
        if new.two_words && matches!(new.level, LevelArg::Alto) {
            return Err("--two-words solo se puede usar con --level extremo".into());
        }
    }
    match cli.command {
        Command::Create {
            output,
            inputs,
            plain: true,
            new,
        } => {
            let packed = archive::create_plain(
                &output,
                &inputs,
                new.overwrite,
                new.compression.into(),
                &mut progress(),
            )?;
            println!(
                "ZIP normal sin protección: {} archivos, {} bytes.",
                packed.files, packed.bytes
            );
        }
        Command::Create {
            output,
            inputs,
            plain: false,
            new,
        } => {
            let opts = options(&new);
            if output.exists() && !opts.overwrite {
                return Err(Error::AlreadyExists(output).into());
            }
            let (secret, generated) = secret_for_new(opts.level, &new.secret)?;
            let created = archive::create(&output, &inputs, &secret, &opts, &mut progress())?;
            report_created(&created, &secret, generated, new.secret.lang);
        }
        Command::Migrate {
            source,
            output,
            old_password,
            new,
        } => {
            let opts = options(&new);
            if output.exists() && !opts.overwrite {
                return Err(Error::AlreadyExists(output).into());
            }
            let old_password = match old_password {
                Some(p) => Some(p),
                None if archive::zip_needs_password(&source)? => {
                    Some(prompt("Contraseña del ZIP original: ")?)
                }
                None => None,
            };
            let (secret, generated) = secret_for_new(opts.level, &new.secret)?;
            let created = archive::migrate(
                &source,
                old_password.as_deref(),
                &output,
                &secret,
                &opts,
                &mut progress(),
            )?;
            report_created(&created, &secret, generated, new.secret.lang);
        }
        Command::Open {
            archive: path,
            output,
            overwrite,
            secret,
        } => {
            let info = archive::inspect(&path)?;
            eprintln!("{}", describe(&info.header));
            let secret = secret_for(&info.header, &secret)?;
            let done = archive::extract(&path, &secret, &output, overwrite, &mut progress())?;
            println!("{} archivos extraídos en {}", done.files, output.display());
            for name in &done.skipped {
                println!("Omitido (enlace simbólico): {name}");
            }
        }
        Command::Info { archive: path } => {
            let info = archive::inspect(&path)?;
            println!("{}", describe(&info.header));
            println!(
                "Argon2id: {} MiB, {} pasadas, {} hilos.",
                info.header.kdf.m_kib / 1024,
                info.header.kdf.t,
                info.header.kdf.p
            );
            match info.entries {
                None => println!("El contenido solo se puede listar con el secreto."),
                Some(entries) => {
                    for e in entries {
                        if e.is_dir {
                            println!("{:>12}  {}", "", e.name);
                        } else {
                            println!("{:>12}  {}", e.size, e.name);
                        }
                    }
                }
            }
        }
        Command::Key {
            archive: path,
            secret,
        } => {
            let info = archive::inspect(&path)?;
            eprintln!("{}", describe(&info.header));
            let secret = secret_for(&info.header, &secret)?;
            eprintln!("Derivando la clave (Argon2id)…");
            let password = derive_password(&info.header, &secret)?;
            println!("{}", password.as_str());
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
