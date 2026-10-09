use crate::metadata::AppMetadata;

use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

/// Generates a stable ID that distinguishes apps with the same display name
/// when their desktop metadata or launch commands differ.
fn app_id(info: &AppMetadata) -> String {
    let mut hasher = DefaultHasher::new();

    info.name.hash(&mut hasher);
    info.desktop_file.hash(&mut hasher);
    info.exec.hash(&mut hasher);

    format!("{}-{:016x}", slugify(&info.name), hasher.finish())
}

fn paths(info: &AppMetadata) -> io::Result<(PathBuf, PathBuf)> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Could not determine home directory"))?;

    let id = app_id(info);

    Ok((
        home.join(".local/opt/appdrop")
            .join(format!("{id}.AppImage")),
        home.join(".local/share/applications")
            .join(format!("appdrop-{id}.desktop")),
    ))
}

/// Returns true if either managed path exists, including a broken symlink.
/// This prevents a new install from overwriting leftover managed paths.
pub fn is_installed(info: &AppMetadata) -> bool {
    match paths(info) {
        Ok((appimage, desktop)) => {
            appimage.symlink_metadata().is_ok() || desktop.symlink_metadata().is_ok()
        }
        Err(_) => false,
    }
}

fn clean_desktop_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', " ")
        .replace('\r', " ")
        .replace('\t', " ")
}

fn quote_exec_arg(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");

    format!("\"{escaped}\"")
}

fn categories(info: &AppMetadata) -> String {
    let valid: Vec<&str> = info
        .categories
        .iter()
        .map(String::as_str)
        .filter(|category| {
            !category.is_empty()
                && category
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        .collect();

    if valid.is_empty() {
        "Utility;".to_string()
    } else {
        format!("{};", valid.join(";"))
    }
}

fn temporary_path(destination: &Path) -> io::Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("Invalid destination path"))?;

    let filename = destination
        .file_name()
        .ok_or_else(|| io::Error::other("Invalid destination filename"))?
        .to_string_lossy();

    for _ in 0..100 {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{filename}.appdrop-tmp-{}-{counter}",
            std::process::id()
        ));

        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Could not allocate a temporary installation path",
    ))
}

fn create_temp_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn commit_without_overwrite(temp: &Path, destination: &Path) -> io::Result<()> {
    // Both paths are in the same directory. hard_link fails if the target
    // already exists instead of replacing an existing installation.
    fs::hard_link(temp, destination)?;
    fs::remove_file(temp)?;
    Ok(())
}

/// Verify the source is a regular file. Do not require its executable bit:
/// downloaded AppImages can lose that bit, and we set it on the installed copy.
fn validate_source(source: &Path) -> io::Result<()> {
    let metadata = fs::metadata(source)?;

    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "The selected source is not a regular file",
        ));
    }

    Ok(())
}

pub fn install(source: &Path, info: &AppMetadata) -> io::Result<PathBuf> {
    validate_source(source)?;

    let (destination, desktop_file) = paths(info)?;

    if is_installed(info) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is already installed or has leftover files", info.name),
        ));
    }

    let app_dir = destination
        .parent()
        .ok_or_else(|| io::Error::other("Invalid installation path"))?;

    let desktop_dir = desktop_file
        .parent()
        .ok_or_else(|| io::Error::other("Invalid desktop entry path"))?;

    fs::create_dir_all(app_dir)?;
    fs::create_dir_all(desktop_dir)?;

    let app_temp = temporary_path(&destination)?;
    let desktop_temp = temporary_path(&desktop_file)?;

    let install_result = (|| -> io::Result<()> {
        // Copy into a temporary file so a failed copy never leaves a partial
        // AppImage at the final destination.
        {
            let mut output = create_temp_file(&app_temp)?;
            let mut input = File::open(source)?;

            io::copy(&mut input, &mut output)?;
            output.sync_all()?;
        }

        // Preserve read/write permissions where reasonable, and always make
        // the installed copy executable. The source itself is never changed.
        let source_mode = fs::metadata(source)?.permissions().mode();
        let installed_mode = (source_mode & 0o666) | 0o111;

        fs::set_permissions(&app_temp, fs::Permissions::from_mode(installed_mode))?;

        commit_without_overwrite(&app_temp, &destination)?;

        let icon = info
            .icon
            .as_deref()
            .filter(|icon| !icon.is_empty() && !icon.contains('/'))
            .unwrap_or("application-x-executable");

        // Launch the AppImage itself. Its AppRun handles its bundled command
        // and arguments; do not reconstruct its embedded Exec line.
        let exec_line = format!("{} %U", quote_exec_arg(&destination.to_string_lossy()));

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
            categories(info),
        );

        {
            let mut output = create_temp_file(&desktop_temp)?;
            output.write_all(desktop_entry.as_bytes())?;
            output.sync_all()?;
        }

        fs::set_permissions(&desktop_temp, fs::Permissions::from_mode(0o644))?;

        commit_without_overwrite(&desktop_temp, &desktop_file)?;

        Ok(())
    })();

    // Remove temporary files whether installation succeeded or failed.
    let _ = fs::remove_file(&app_temp);
    let _ = fs::remove_file(&desktop_temp);

    if let Err(error) = install_result {
        // Roll back only the managed paths for this installation.
        let _ = fs::remove_file(&desktop_file);
        let _ = fs::remove_file(&destination);
        return Err(error);
    }

    Ok(destination)
}

/// Remove only AppDrop-managed files. Never delete the original download.
pub fn uninstall(info: &AppMetadata) -> io::Result<()> {
    let (appimage, desktop) = paths(info)?;

    let appimage_exists = fs::symlink_metadata(&appimage).is_ok();
    let desktop_exists = fs::symlink_metadata(&desktop).is_ok();

    if !appimage_exists && !desktop_exists {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is not installed by AppDrop", info.name),
        ));
    }

    // Attempt both removals, even if one fails.
    let appimage_result = if appimage_exists {
        fs::remove_file(&appimage)
    } else {
        Ok(())
    };

    let desktop_result = if desktop_exists {
        fs::remove_file(&desktop)
    } else {
        Ok(())
    };

    match (appimage_result, desktop_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(app_error), Err(desktop_error)) => Err(io::Error::other(format!(
            "Could not remove all AppDrop files. \
                 AppImage error: {app_error}; launcher error: {desktop_error}"
        ))),
    }
}
