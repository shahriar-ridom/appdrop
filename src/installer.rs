use crate::metadata::AppMetadata;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn slugify(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    let slug = slug.trim_matches('-').to_string();

    if slug.is_empty() {
        "appimage".to_string()
    } else {
        slug
    }
}

fn paths(info: &AppMetadata) -> io::Result<(PathBuf, PathBuf)> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Could not determine home directory"))?;

    let slug = slugify(&info.name);

    Ok((
        home.join(".local/opt").join(format!("{slug}.AppImage")),
        home.join(".local/share/applications")
            .join(format!("appdrop-{slug}.desktop")),
    ))
}

/// Returns true if AppDrop already has an AppImage or launcher for this app.
pub fn is_installed(info: &AppMetadata) -> bool {
    match paths(info) {
        Ok((appimage, desktop)) => appimage.is_file() || desktop.is_file(),
        Err(_) => false,
    }
}

fn clean_desktop_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', " ")
        .replace('\r', " ")
}

fn quote_exec_arg(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");

    format!("\"{escaped}\"")
}

pub fn install(source: &Path, info: &AppMetadata) -> io::Result<PathBuf> {
    if is_installed(info) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is already installed", info.name),
        ));
    }

    let (destination, desktop_file) = paths(info)?;

    let app_dir = destination
        .parent()
        .ok_or_else(|| io::Error::other("Invalid installation path"))?;

    let desktop_dir = desktop_file
        .parent()
        .ok_or_else(|| io::Error::other("Invalid desktop entry path"))?;

    fs::create_dir_all(app_dir)?;
    fs::create_dir_all(desktop_dir)?;

    fs::copy(source, &destination)?;

    let mut permissions = fs::metadata(&destination)?.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs::set_permissions(&destination, permissions)?;

    let exec_path = quote_exec_arg(&destination.to_string_lossy());

    let extra_args = info
        .exec
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .skip(1)
        .map(|arg| {
            if arg.starts_with('%') {
                arg.to_string()
            } else {
                quote_exec_arg(arg)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let exec_line = if extra_args.is_empty() {
        exec_path
    } else {
        format!("{exec_path} {extra_args}")
    };

    let icon = info
        .icon
        .as_deref()
        .filter(|icon| !icon.is_empty() && !icon.contains('/'))
        .unwrap_or("application-x-executable");

    let categories = if info.categories.is_empty() {
        "Utility;".to_string()
    } else {
        format!("{};", info.categories.join(";"))
    };

    let desktop_entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={}\n\
         Exec={}\n\
         Icon={}\n\
         Terminal=false\n\
         Categories={}\n",
        clean_desktop_value(&info.name),
        exec_line,
        clean_desktop_value(icon),
        clean_desktop_value(&categories),
    );

    if let Err(error) = fs::write(&desktop_file, desktop_entry) {
        // Roll back the copied binary if launcher creation fails.
        let _ = fs::remove_file(&destination);
        return Err(error);
    }

    Ok(destination)
}

/// Removes only the AppImage and desktop launcher managed by AppDrop.
/// The original source file is never deleted.
pub fn uninstall(info: &AppMetadata) -> io::Result<()> {
    let (appimage, desktop) = paths(info)?;

    let appimage_exists = appimage.is_file();
    let desktop_exists = desktop.is_file();

    if !appimage_exists && !desktop_exists {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is not installed by AppDrop", info.name),
        ));
    }

    if desktop_exists {
        fs::remove_file(&desktop)?;
    }

    if appimage_exists {
        fs::remove_file(&appimage)?;
    }

    Ok(())
}
