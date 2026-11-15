use std::fs;
use std::io;
use std::path::PathBuf;

const APP_ID: &str = "geozip";

fn home_subdir(variable: &str, fallback: &str) -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(variable).filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(fallback))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
}

fn entry_path() -> io::Result<PathBuf> {
    Ok(home_subdir("XDG_DATA_HOME", ".local/share")?.join(format!("applications/{APP_ID}.desktop")))
}

fn icon_path() -> io::Result<PathBuf> {
    Ok(
        home_subdir("XDG_DATA_HOME", ".local/share")?
            .join(format!("icons/hicolor/256x256/apps/{APP_ID}.png")),
    )
}

fn file_manager_menu_path() -> io::Result<PathBuf> {
    Ok(home_subdir("XDG_DATA_HOME", ".local/share")?.join(format!("kio/servicemenus/{APP_ID}.desktop")))
}

fn file_manager_menu_contents() -> io::Result<String> {
    let executable = std::env::current_exe()?;
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Service\n\
         MimeType=application/octet-stream;inode/directory;\n\
         Actions=compress\n\
         X-KDE-Priority=TopLevel\n\n\
         [Desktop Action compress]\n\
         Name=Comprimir con GeoZip\n\
         Name[en]=Compress with GeoZip\n\
         Icon={APP_ID}\n\
         Exec=\"{}\" %F\n",
        executable.display()
    ))
}

fn write_executable(path: &PathBuf, contents: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

fn opt_out_path() -> io::Result<PathBuf> {
    Ok(home_subdir("XDG_CONFIG_HOME", ".config")?.join(format!("{APP_ID}/no-desktop-entry")))
}

fn entry_contents() -> io::Result<String> {
    let executable = std::env::current_exe()?;
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=GeoZip\n\
         Comment=ZIP con AES-256 cuya clave son lugares\n\
         Exec=\"{}\" %F\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Utility;Archiving;\n\
         MimeType=application/zip;\n\
         StartupWMClass={APP_ID}\n",
        executable.display()
    ))
}

pub fn is_registered() -> bool {
    entry_path().is_ok_and(|path| path.exists())
}

pub fn register(icon_png: &[u8]) -> io::Result<()> {
    let icon = icon_path()?;
    let entry = entry_path()?;
    for path in [&icon, &entry] {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(icon, icon_png)?;
    fs::write(entry, entry_contents()?)?;
    write_executable(&file_manager_menu_path()?, &file_manager_menu_contents()?)?;
    let _ = fs::remove_file(opt_out_path()?);
    Ok(())
}

pub fn unregister() -> io::Result<()> {
    for path in [entry_path()?, icon_path()?, file_manager_menu_path()?] {
        match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    let opt_out = opt_out_path()?;
    if let Some(parent) = opt_out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(opt_out, "")
}

#[cfg_attr(debug_assertions, allow(dead_code))]
pub fn register_unless_opted_out(icon_png: &[u8]) {
    let opted_out = opt_out_path().is_ok_and(|path| path.exists());
    let matches = |path: io::Result<PathBuf>, wanted: io::Result<String>| match (
        path.and_then(fs::read_to_string),
        wanted,
    ) {
        (Ok(current), Ok(wanted)) => current == wanted,
        _ => false,
    };
    let up_to_date = matches(entry_path(), entry_contents())
        && matches(file_manager_menu_path(), file_manager_menu_contents())
        && icon_path().is_ok_and(|path| path.exists());
    if !opted_out && !up_to_date {
        let _ = register(icon_png);
    }
}
