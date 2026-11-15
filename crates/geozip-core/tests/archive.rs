use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use geozip_core::archive::{self, Compression, CreateOptions, Edit, Scope, INNER_NAME};
use geozip_core::grid::METERS_PER_DEG;
use geozip_core::{derive_password, Error, KdfParams, LatLon, Level, Secret};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::{AesMode, ZipArchive, ZipWriter};

const FAST: KdfParams = KdfParams {
    m_kib: 64,
    t: 1,
    p: 1,
};

fn opts(level: Level, hide_names: bool) -> CreateOptions {
    CreateOptions {
        level,
        hide_names,
        overwrite: false,
        kdf: Some(FAST),
        compression: Compression::default(),
    }
}

fn places(n: usize) -> Vec<LatLon> {
    let all = [
        (40.4168, -3.7038),
        (41.9009, 12.4833),
        (35.6762, 139.6503),
        (-33.8688, 151.2093),
        (40.7128, -74.0060),
    ];
    all[..n].iter().map(|&(lat, lon)| LatLon { lat, lon }).collect()
}

fn alto() -> Secret {
    Secret::places_only(places(5))
}

fn extremo() -> Secret {
    Secret::new(
        places(5),
        vec![
            vec![10, 1800],
            vec![3, 4],
            vec![77, 1024],
            vec![0, 1849],
            vec![500, 501],
        ],
    )
}

fn sample_tree(base: &Path) -> PathBuf {
    let root = base.join("docs");
    fs::create_dir_all(root.join("sub/vacía")).unwrap();
    fs::write(root.join("hola.txt"), "hola, mundo\n").unwrap();
    fs::write(root.join("vacío.bin"), b"").unwrap();
    let big: Vec<u8> = (0..300_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    fs::write(root.join("sub/datos.bin"), big).unwrap();
    fs::write(root.join("sub/ñandú.txt"), "áéíóú ñ\n".repeat(500)).unwrap();
    let old = archive::unix_to_system_time(1_500_000_000);
    File::options()
        .write(true)
        .open(root.join("hola.txt"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.join("hola.txt"), fs::Permissions::from_mode(0o640)).unwrap();
    }
    root
}

fn files_under(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                out.push((format!("{rel}/"), Vec::new()));
                walk(&p, root, out);
            } else {
                out.push((rel, fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

fn quiet() -> impl FnMut(archive::Event<'_>) {
    |_| {}
}

#[test]
fn roundtrip_visible_names() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let zip_path = tmp.path().join("a.zip");

    let mut events = 0;
    let created = archive::create(
        &zip_path,
        &[root.clone()],
        &alto(),
        &opts(Level::Alto, false),
        &mut |_| events += 1,
    )
    .unwrap();
    assert_eq!(created.files, 4);
    assert!(events > 4);
    assert!(!tmp.path().join("a.zip.partial").exists());

    let info = archive::inspect(&zip_path).unwrap();
    assert_eq!(info.header.level(), Some(Level::Alto));
    let names: Vec<String> = info.entries.unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&"docs/sub/ñandú.txt".to_string()), "{names:?}");
    assert!(names.contains(&"docs/sub/vacía/".to_string()), "{names:?}");

    let out = tmp.path().join("out");
    let extracted = archive::extract(&zip_path, &alto(), &out, false, &mut quiet()).unwrap();
    assert_eq!(extracted.files, 4);
    assert_eq!(files_under(&out.join("docs")), files_under(&root));

    let mtime = fs::metadata(out.join("docs/hola.txt"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(mtime, archive::unix_to_system_time(1_500_000_000));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(out.join("docs/hola.txt"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
    }
}

#[test]
fn roundtrip_hidden_names() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let zip_path = tmp.path().join("h.zip");
    archive::create(
        &zip_path,
        &[root.clone()],
        &extremo(),
        &opts(Level::ExtremoDoble, true),
        &mut quiet(),
    )
    .unwrap();

    let info = archive::inspect(&zip_path).unwrap();
    assert_eq!(info.header.level(), Some(Level::ExtremoDoble));
    assert!(info.entries.is_none());

    let mut raw = ZipArchive::new(File::open(&zip_path).unwrap()).unwrap();
    let visible: Vec<String> = raw.file_names().map(|n| n.unwrap().into_owned()).collect();
    assert_eq!(visible, [INNER_NAME]);
    assert!(raw.by_index_raw(0).unwrap().encrypted());
    let bytes = fs::read(&zip_path).unwrap();
    assert!(!bytes.windows(8).any(|w| w == b"hola.txt"));

    let listed = archive::list_hidden(&zip_path, &extremo()).unwrap();
    assert!(listed.iter().any(|e| e.name == "docs/hola.txt"));

    let out = tmp.path().join("out");
    archive::extract(&zip_path, &extremo(), &out, false, &mut quiet()).unwrap();
    assert_eq!(files_under(&out.join("docs")), files_under(&root));
}

#[test]
fn wrong_secrets_are_rejected_and_small_errors_tolerated() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let zip_path = tmp.path().join("a.zip");
    archive::create(
        &zip_path,
        &[root],
        &extremo(),
        &opts(Level::ExtremoDoble, false),
        &mut quiet(),
    )
    .unwrap();
    let out = tmp.path().join("out");
    let try_open = |secret: &Secret| archive::extract(&zip_path, secret, &out, false, &mut quiet());

    let mut far = extremo();
    far.places[2].lat += 1000.0 / METERS_PER_DEG;
    assert!(matches!(try_open(&far), Err(Error::WrongSecret)));

    let mut swapped = extremo();
    swapped.places.swap(0, 1);
    swapped.words.swap(0, 1);
    assert!(matches!(try_open(&swapped), Err(Error::WrongSecret)));

    let mut word = extremo();
    word.words[3] = vec![0, 1848];
    assert!(matches!(try_open(&word), Err(Error::WrongSecret)));

    assert!(!out.exists());

    let mut near = extremo();
    for p in &mut near.places {
        p.lat += 70.0 / METERS_PER_DEG;
    }
    for w in &mut near.words {
        w.reverse();
    }
    assert_eq!(try_open(&near).unwrap().files, 4);
}

fn strip_comment(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    let eocd = bytes.windows(4).rposition(|w| w == b"PK\x05\x06").unwrap();
    bytes[eocd + 20] = 0;
    bytes[eocd + 21] = 0;
    bytes.truncate(eocd + 22);
    fs::write(path, bytes).unwrap();
}

#[test]
fn header_survives_losing_the_zip_comment() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    for hide in [false, true] {
        let zip_path = tmp.path().join(format!("c{hide}.zip"));
        let created = archive::create(
            &zip_path,
            &[root.clone()],
            &alto(),
            &opts(Level::Alto, hide),
            &mut quiet(),
        )
        .unwrap();
        strip_comment(&zip_path);
        assert!(ZipArchive::new(File::open(&zip_path).unwrap())
            .unwrap()
            .comment()
            .is_empty());
        assert_eq!(archive::inspect(&zip_path).unwrap().header, created.header);
        let out = tmp.path().join(format!("out{hide}"));
        archive::extract(&zip_path, &alto(), &out, false, &mut quiet()).unwrap();
    }
}

#[test]
fn tampering_is_detected() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());

    let injected = tmp.path().join("inj.zip");
    archive::create(
        &injected,
        &[root.clone()],
        &alto(),
        &opts(Level::Alto, false),
        &mut quiet(),
    )
    .unwrap();
    let file = File::options().read(true).write(true).open(&injected).unwrap();
    let mut w = ZipWriter::new_append(file).unwrap();
    w.start_file("docs/intruso.txt", SimpleFileOptions::default())
        .unwrap();
    w.write_all(b"no deberia extraerse").unwrap();
    w.finish().unwrap();
    let out = tmp.path().join("out1");
    let err = archive::extract(&injected, &alto(), &out, false, &mut quiet()).unwrap_err();
    assert!(
        matches!(err, Error::UnencryptedEntry(ref n) if n == "docs/intruso.txt"),
        "{err}"
    );

    let flipped = tmp.path().join("flip.zip");
    archive::create(
        &flipped,
        &[root.join("sub/datos.bin")],
        &alto(),
        &opts(Level::Alto, false),
        &mut quiet(),
    )
    .unwrap();
    let mut bytes = fs::read(&flipped).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x01;
    fs::write(&flipped, bytes).unwrap();
    let out = tmp.path().join("out2");
    let err = archive::extract(&flipped, &alto(), &out, false, &mut quiet()).unwrap_err();
    assert!(matches!(err, Error::Corrupt(_)), "{err}");
    assert!(
        !out.join("datos.bin").exists(),
        "no debe quedar un archivo a medias"
    );
}

#[test]
fn never_overwrites_unless_asked() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let zip_path = tmp.path().join("a.zip");
    archive::create(
        &zip_path,
        &[root.clone()],
        &alto(),
        &opts(Level::Alto, false),
        &mut quiet(),
    )
    .unwrap();
    let again = archive::create(
        &zip_path,
        &[root.clone()],
        &alto(),
        &opts(Level::Alto, false),
        &mut quiet(),
    );
    assert!(matches!(again, Err(Error::AlreadyExists(_))));

    let out = tmp.path().join("out");
    archive::extract(&zip_path, &alto(), &out, false, &mut quiet()).unwrap();
    let second = archive::extract(&zip_path, &alto(), &out, false, &mut quiet());
    assert!(matches!(second, Err(Error::AlreadyExists(_))));
    archive::extract(&zip_path, &alto(), &out, true, &mut quiet()).unwrap();

    let mut overwrite = opts(Level::Alto, false);
    overwrite.overwrite = true;
    archive::create(&zip_path, &[root], &alto(), &overwrite, &mut quiet()).unwrap();
}

#[test]
fn shape_and_input_errors() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let zip_path = tmp.path().join("a.zip");
    let four = Secret::places_only(places(4));
    assert!(matches!(
        archive::create(
            &zip_path,
            &[root.clone()],
            &four,
            &opts(Level::Alto, false),
            &mut quiet()
        ),
        Err(Error::PlaceCount { expected: 5, got: 4 })
    ));
    assert!(matches!(
        archive::create(
            &zip_path,
            &[root.clone()],
            &alto(),
            &opts(Level::ExtremoDoble, false),
            &mut quiet()
        ),
        Err(Error::WordCount { .. })
    ));
    let empty = tmp.path().join("nada");
    fs::create_dir(&empty).unwrap();
    assert!(matches!(
        archive::create(
            &zip_path,
            &[empty],
            &alto(),
            &opts(Level::Alto, false),
            &mut quiet()
        ),
        Err(Error::NothingToAdd)
    ));
    assert!(!zip_path.exists());

    let plain = tmp.path().join("plain.zip");
    let mut w = ZipWriter::new(File::create(&plain).unwrap());
    w.start_file("a.txt", SimpleFileOptions::default()).unwrap();
    w.write_all(b"a").unwrap();
    w.finish().unwrap();
    assert!(matches!(archive::inspect(&plain), Err(Error::NotGeoZip)));
}

#[test]
fn each_archive_gets_its_own_salt_and_key() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let mut passwords = Vec::new();
    for i in 0..2 {
        let zip_path = tmp.path().join(format!("{i}.zip"));
        let created = archive::create(
            &zip_path,
            &[root.clone()],
            &alto(),
            &opts(Level::Alto, false),
            &mut quiet(),
        )
        .unwrap();
        passwords.push(derive_password(&created.header, &alto()).unwrap().to_string());
    }
    assert_ne!(passwords[0], passwords[1]);
}

fn old_zip(path: &Path, password: Option<&str>) {
    let mut w = ZipWriter::new(File::create(path).unwrap());
    w.add_directory("carpeta/", SimpleFileOptions::default()).unwrap();
    for (name, body) in [
        ("carpeta/uno.txt", "uno ".repeat(2000)),
        ("dos.txt", "dos".to_string()),
    ] {
        let o = SimpleFileOptions::default().unix_permissions(0o600);
        match password {
            Some(p) => w
                .start_file(name, o.with_aes_encryption(AesMode::Aes256, p))
                .unwrap(),
            None => w.start_file(name, o).unwrap(),
        }
        w.write_all(body.as_bytes()).unwrap();
    }
    w.finish().unwrap();
}

#[test]
fn migrate_plain_and_encrypted_zips() {
    let tmp = TempDir::new().unwrap();
    for (label, old_pw, hide) in [("plain", None, false), ("aes", Some("vieja"), true)] {
        let src = tmp.path().join(format!("{label}.zip"));
        old_zip(&src, old_pw);
        assert_eq!(archive::zip_needs_password(&src).unwrap(), old_pw.is_some());

        let dst = tmp.path().join(format!("{label}-geo.zip"));
        if old_pw.is_some() {
            for wrong in [None, Some("otra")] {
                let r = archive::migrate(&src, wrong, &dst, &alto(), &opts(Level::Alto, hide), &mut quiet());
                assert!(matches!(r, Err(Error::WrongOldPassword)), "{r:?}");
                assert!(!dst.exists());
            }
        }
        let created = archive::migrate(
            &src,
            old_pw,
            &dst,
            &alto(),
            &opts(Level::Alto, hide),
            &mut quiet(),
        )
        .unwrap();
        assert_eq!(created.files, 2);

        let out = tmp.path().join(format!("{label}-out"));
        archive::extract(&dst, &alto(), &out, false, &mut quiet()).unwrap();
        assert_eq!(
            fs::read_to_string(out.join("carpeta/uno.txt")).unwrap(),
            "uno ".repeat(2000)
        );
        assert_eq!(fs::read_to_string(out.join("dos.txt")).unwrap(), "dos");

        let twice = archive::migrate(
            &dst,
            None,
            &tmp.path().join("x.zip"),
            &alto(),
            &opts(Level::Alto, false),
            &mut quiet(),
        );
        assert!(matches!(twice, Err(Error::AlreadyGeoZip)));
    }
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--help").output().is_ok()
}

#[test]
fn seven_zip_opens_it_with_the_derived_password() {
    if !have("7z") {
        eprintln!("7z no está instalado; se omite la prueba de interoperabilidad");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let zip_path = tmp.path().join("a.zip");
    let created = archive::create(
        &zip_path,
        &[root.clone()],
        &alto(),
        &opts(Level::Alto, false),
        &mut quiet(),
    )
    .unwrap();
    let password = derive_password(&created.header, &alto()).unwrap();

    let out = tmp.path().join("out7z");
    let run = |pw: &str| {
        Command::new("7z")
            .arg("x")
            .arg(format!("-p{pw}"))
            .arg(format!("-o{}", out.display()))
            .arg("-y")
            .arg(&zip_path)
            .output()
            .unwrap()
    };
    assert!(!run("contraseña-equivocada").status.success());
    let _ = fs::remove_dir_all(&out);
    let ok = run(&password);
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stdout));
    assert_eq!(files_under(&out.join("docs")), files_under(&root));
}

#[test]
fn migrate_legacy_zipcrypto_made_by_info_zip() {
    if !have("zip") {
        eprintln!("zip no está instalado; se omite la prueba");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let src = tmp.path().join("legacy.zip");
    let status = Command::new("zip")
        .current_dir(tmp.path())
        .args(["-q", "-r", "-P", "antigua", "legacy.zip", "docs"])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(archive::zip_needs_password(&src).unwrap());

    let dst = tmp.path().join("nuevo.zip");
    archive::migrate(
        &src,
        Some("antigua"),
        &dst,
        &alto(),
        &opts(Level::Alto, false),
        &mut quiet(),
    )
    .unwrap();
    let out = tmp.path().join("out");
    archive::extract(&dst, &alto(), &out, false, &mut quiet()).unwrap();
    assert_eq!(files_under(&out.join("docs")), files_under(&root));

    let mut f = File::open(out.join("docs/hola.txt")).unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    assert_eq!(s, "hola, mundo\n");
}

#[test]
fn roundtrip_one_word_per_place() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let zip_path = tmp.path().join("one.zip");
    let secret = Secret::new(
        places(5),
        vec![vec![10], vec![3], vec![1024], vec![1849], vec![7]],
    );
    archive::create(
        &zip_path,
        &[root.clone()],
        &secret,
        &opts(Level::Extremo, true),
        &mut quiet(),
    )
    .unwrap();
    assert_eq!(
        archive::inspect(&zip_path).unwrap().header.level(),
        Some(Level::Extremo)
    );

    let out = tmp.path().join("out");
    let mut wrong = secret.clone();
    wrong.words[2] = vec![1025];
    assert!(matches!(
        archive::extract(&zip_path, &wrong, &out, false, &mut quiet()),
        Err(Error::WrongSecret)
    ));
    archive::extract(&zip_path, &secret, &out, false, &mut quiet()).unwrap();
    assert_eq!(files_under(&out.join("docs")), files_under(&root));
}

#[test]
fn plain_zip_roundtrip_and_password_protected_extraction() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let zip_path = tmp.path().join("plain.zip");
    let packed = archive::create_plain(
        &zip_path,
        &[root.clone()],
        false,
        Compression::Normal,
        &mut quiet(),
    )
    .unwrap();
    assert_eq!(packed.files, 4);
    assert!(matches!(archive::inspect(&zip_path), Err(Error::NotGeoZip)));

    let info = archive::inspect_plain(&zip_path).unwrap();
    assert!(!info.needs_password);
    assert!(info.entries.iter().any(|e| e.name == "docs/hola.txt"));

    let out = tmp.path().join("out");
    archive::extract_plain(&zip_path, None, &out, false, &mut quiet()).unwrap();
    assert_eq!(files_under(&out.join("docs")), files_under(&root));

    let locked = tmp.path().join("locked.zip");
    old_zip(&locked, Some("vieja"));
    assert!(archive::inspect_plain(&locked).unwrap().needs_password);
    let out = tmp.path().join("out-locked");
    for wrong in [None, Some("otra")] {
        let result = archive::extract_plain(&locked, wrong, &out, false, &mut quiet());
        assert!(matches!(result, Err(Error::WrongOldPassword)), "{result:?}");
    }
    archive::extract_plain(&locked, Some("vieja"), &out, false, &mut quiet()).unwrap();
    assert_eq!(fs::read_to_string(out.join("dos.txt")).unwrap(), "dos");

    let preview = tmp.path().join("preview");
    let single = archive::extract_plain_entry(&zip_path, "docs/hola.txt", &preview).unwrap();
    assert_eq!(single, preview.join("hola.txt"));
    assert_eq!(fs::read_to_string(single).unwrap(), "hola, mundo\n");
    assert!(archive::extract_plain_entry(&zip_path, "docs/sub/", &preview).is_err());
    assert!(archive::extract_plain_entry(&zip_path, "no-existe.txt", &preview).is_err());

    let geo = tmp.path().join("geo.zip");
    archive::create(&geo, &[root], &alto(), &opts(Level::Alto, false), &mut quiet()).unwrap();
    assert!(matches!(archive::inspect_plain(&geo), Err(Error::AlreadyGeoZip)));
    assert!(matches!(
        archive::extract_plain(&geo, None, &tmp.path().join("x"), false, &mut quiet()),
        Err(Error::AlreadyGeoZip)
    ));
}

#[test]
fn extracts_only_the_selection_relative_to_a_folder() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let geo = tmp.path().join("geo.zip");
    archive::create(
        &geo,
        &[root.clone()],
        &alto(),
        &opts(Level::Alto, false),
        &mut quiet(),
    )
    .unwrap();
    let plain = tmp.path().join("plain.zip");
    archive::create_plain(&plain, &[root], false, Compression::Normal, &mut quiet()).unwrap();

    let scope = Scope {
        only: vec!["docs/sub/".to_string(), "docs/hola.txt".to_string()],
        relative_to: "docs".to_string(),
        verify_only: false,
    };
    let from_geo = tmp.path().join("from-geo");
    let done = archive::extract_scoped(&geo, &alto(), &from_geo, false, &scope, &mut quiet()).unwrap();
    let from_plain = tmp.path().join("from-plain");
    archive::extract_plain_scoped(&plain, None, &from_plain, false, &scope, &mut quiet()).unwrap();

    assert_eq!(done.files, 3);
    for out in [from_geo, from_plain] {
        let names: Vec<String> = files_under(&out).into_iter().map(|(name, _)| name).collect();
        assert_eq!(
            names,
            ["hola.txt", "sub/", "sub/datos.bin", "sub/vacía/", "sub/ñandú.txt"]
        );
    }
}

#[test]
fn verifying_reads_everything_and_writes_nothing() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let verify = Scope {
        verify_only: true,
        ..Scope::default()
    };
    let nowhere = tmp.path().join("nowhere");
    for hide in [false, true] {
        let geo = tmp.path().join(format!("geo{hide}.zip"));
        archive::create(
            &geo,
            &[root.clone()],
            &alto(),
            &opts(Level::Alto, hide),
            &mut quiet(),
        )
        .unwrap();
        let done = archive::extract_scoped(&geo, &alto(), &nowhere, false, &verify, &mut quiet()).unwrap();
        assert_eq!(done.files, 4);
        assert!(!nowhere.exists());

        let mut wrong = alto();
        wrong.places[0].lat += 0.01;
        let result = archive::extract_scoped(&geo, &wrong, &nowhere, false, &verify, &mut quiet());
        assert!(matches!(result, Err(Error::WrongSecret)));

        let mut bytes = fs::read(&geo).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 1;
        fs::write(&geo, bytes).unwrap();
        let result = archive::extract_scoped(&geo, &alto(), &nowhere, false, &verify, &mut quiet());
        assert!(matches!(result, Err(Error::Corrupt(_))), "{result:?}");
    }
}

fn names_in(path: &Path, secret: &Secret) -> Vec<String> {
    let mut names: Vec<String> = archive::list_hidden(path, secret)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    names
}

#[test]
fn editing_a_protected_archive_adds_replaces_and_removes() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let extra = tmp.path().join("nuevo.txt");
    fs::write(&extra, "recién añadido").unwrap();
    let replacement_dir = tmp.path().join("otro");
    fs::create_dir(&replacement_dir).unwrap();
    fs::write(replacement_dir.join("hola.txt"), "sustituido").unwrap();

    for hide in [false, true] {
        let geo = tmp.path().join(format!("edit{hide}.zip"));
        archive::create(
            &geo,
            &[root.clone()],
            &alto(),
            &opts(Level::Alto, hide),
            &mut quiet(),
        )
        .unwrap();

        let add = Edit {
            add: vec![extra.clone(), replacement_dir.join("hola.txt")],
            add_under: "docs/".to_string(),
            ..Edit::default()
        };
        let no_secret = archive::edit(&geo, None, &add, Compression::Normal, &mut quiet());
        assert!(matches!(no_secret, Err(Error::WrongSecret)));
        let mut wrong = alto();
        wrong.places[1].lon += 0.01;
        let bad = archive::edit(&geo, Some(&wrong), &add, Compression::Normal, &mut quiet());
        assert!(matches!(bad, Err(Error::WrongSecret)));
        archive::edit(&geo, Some(&alto()), &add, Compression::Best, &mut quiet()).unwrap();

        let remove = Edit {
            remove: vec!["docs/sub/".to_string(), "docs/vacío.bin".to_string()],
            ..Edit::default()
        };
        let secret_for_removal = if hide { Some(alto()) } else { None };
        archive::edit(
            &geo,
            secret_for_removal.as_ref(),
            &remove,
            Compression::Normal,
            &mut quiet(),
        )
        .unwrap();

        assert_eq!(
            names_in(&geo, &alto()),
            ["docs/", "docs/hola.txt", "docs/nuevo.txt"]
        );
        let out = tmp.path().join(format!("edit-out{hide}"));
        archive::extract(&geo, &alto(), &out, false, &mut quiet()).unwrap();
        assert_eq!(
            fs::read_to_string(out.join("docs/hola.txt")).unwrap(),
            "sustituido"
        );
        assert_eq!(
            fs::read_to_string(out.join("docs/nuevo.txt")).unwrap(),
            "recién añadido"
        );
        assert_eq!(archive::inspect(&geo).unwrap().header.hidden_names, hide);

        let everything = Edit {
            remove: vec!["docs/".to_string()],
            ..Edit::default()
        };
        let before = fs::read(&geo).unwrap();
        let emptied = archive::edit(
            &geo,
            Some(&alto()),
            &everything,
            Compression::Normal,
            &mut quiet(),
        );
        assert!(matches!(emptied, Err(Error::WouldBeEmpty)), "{emptied:?}");
        assert_eq!(fs::read(&geo).unwrap(), before);
        assert!(!tmp.path().join(format!("edit{hide}.zip.partial")).exists());
    }
}

#[test]
fn editing_a_plain_zip() {
    let tmp = TempDir::new().unwrap();
    let root = sample_tree(tmp.path());
    let extra = tmp.path().join("nuevo.txt");
    fs::write(&extra, "recién añadido").unwrap();
    let plain = tmp.path().join("plain.zip");
    archive::create_plain(&plain, &[root], false, Compression::Fast, &mut quiet()).unwrap();

    let change = Edit {
        remove: vec!["docs/sub/".to_string()],
        add: vec![extra.clone()],
        add_under: String::new(),
    };
    archive::edit_plain(&plain, &change, Compression::Normal, &mut quiet()).unwrap();
    let mut names: Vec<String> = archive::inspect_plain(&plain)
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    assert_eq!(names, ["docs/", "docs/hola.txt", "docs/vacío.bin", "nuevo.txt"]);
    let out = tmp.path().join("out");
    archive::extract_plain(&plain, None, &out, false, &mut quiet()).unwrap();
    assert_eq!(
        fs::read_to_string(out.join("nuevo.txt")).unwrap(),
        "recién añadido"
    );
    assert_eq!(
        fs::read_to_string(out.join("docs/hola.txt")).unwrap(),
        "hola, mundo\n"
    );

    let locked = tmp.path().join("locked.zip");
    old_zip(&locked, Some("vieja"));
    let add = Edit {
        add: vec![extra],
        ..Edit::default()
    };
    assert!(matches!(
        archive::edit_plain(&locked, &add, Compression::Normal, &mut quiet()),
        Err(Error::CannotAddToPasswordZip)
    ));
    let remove = Edit {
        remove: vec!["dos.txt".to_string()],
        ..Edit::default()
    };
    archive::edit_plain(&locked, &remove, Compression::Normal, &mut quiet()).unwrap();
    let out = tmp.path().join("out-locked");
    archive::extract_plain(&locked, Some("vieja"), &out, false, &mut quiet()).unwrap();
    assert!(!out.join("dos.txt").exists());
    assert_eq!(
        fs::read_to_string(out.join("carpeta/uno.txt")).unwrap(),
        "uno ".repeat(2000)
    );
}

#[test]
fn compression_levels_change_the_size_but_not_the_contents() {
    let tmp = TempDir::new().unwrap();
    let text = tmp.path().join("texto.txt");
    fs::write(&text, "la misma frase una y otra vez. ".repeat(4000)).unwrap();
    let mut sizes = Vec::new();
    for (i, compression) in [Compression::Store, Compression::Fast, Compression::Best]
        .into_iter()
        .enumerate()
    {
        let zip_path = tmp.path().join(format!("c{i}.zip"));
        archive::create_plain(&zip_path, &[text.clone()], false, compression, &mut quiet()).unwrap();
        sizes.push(fs::metadata(&zip_path).unwrap().len());
        let out = tmp.path().join(format!("c{i}"));
        archive::extract_plain(&zip_path, None, &out, false, &mut quiet()).unwrap();
        assert_eq!(fs::read(out.join("texto.txt")).unwrap(), fs::read(&text).unwrap());
    }
    assert!(
        sizes[0] > 100_000 && sizes[1] < sizes[0] / 10 && sizes[2] <= sizes[1],
        "{sizes:?}"
    );
}
