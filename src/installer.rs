use crate::metadata::{self, AppMetadata};

use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// Every location AppDrop manages for one installed application.
struct Paths {
    home: PathBuf,
    id: String,
    appimage: PathBuf,
    desktop: PathBuf,
    record: PathBuf,
}

impl Paths {
    fn icon_name(&self) -> String {
        format!("appdrop-{}", self.id)
    }

    fn icon_root(&self) -> PathBuf {
        self.home.join(".local/share/icons/hicolor")
    }
}

impl Paths {
    fn new(home: PathBuf, id: String) -> Paths {
        Paths {
            appimage: home.join(OPT_DIR).join(format!("{id}.AppImage")),
            desktop: home
                .join(APPLICATIONS_DIR)
                .join(format!("appdrop-{id}.desktop")),
            record: home.join(RECORD_DIR).join(format!("{id}.record")),
            home,
            id,
        }
    }
}

const OPT_DIR: &str = ".local/opt/appdrop";
const APPLICATIONS_DIR: &str = ".local/share/applications";
const RECORD_DIR: &str = ".local/share/appdrop";

fn home_dir() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Could not determine home directory"))
}

fn paths(info: &AppMetadata) -> io::Result<Paths> {
    Ok(Paths::new(home_dir()?, app_id(info)))
}

/// Returns true if any managed path exists, including a broken symlink.
/// This prevents a new install from overwriting leftover managed paths.
pub fn is_installed(info: &AppMetadata) -> bool {
    match paths(info) {
        Ok(p) => [&p.appimage, &p.desktop, &p.record]
            .iter()
            .any(|path| path.symlink_metadata().is_ok()),
        Err(_) => false,
    }
}

/// What AppDrop remembers about an installation, so it can be removed later
/// without the original download and without recomputing any paths.
#[derive(Debug, PartialEq)]
struct Record {
    name: String,
    version: Option<String>,
    appimage: PathBuf,
    desktop: PathBuf,
    icon: Option<PathBuf>,
    source: PathBuf,
}

impl Record {
    fn serialize(&self) -> String {
        let mut out = format!(
            "name={}\nappimage={}\ndesktop={}\nsource={}\n",
            self.name.replace('\n', " "),
            self.appimage.display(),
            self.desktop.display(),
            self.source.display(),
        );

        if let Some(icon) = &self.icon {
            out.push_str(&format!("icon={}\n", icon.display()));
        }

        if let Some(version) = &self.version {
            out.push_str(&format!("version={}\n", version.replace('\n', " ")));
        }

        out
    }

    fn parse(contents: &str) -> Option<Record> {
        let (mut name, mut version, mut appimage, mut desktop, mut icon, mut source) =
            (None, None, None, None, None, None);

        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };

            match key {
                "name" => name = Some(value.to_owned()),
                "version" => version = Some(value.to_owned()),
                "appimage" => appimage = Some(PathBuf::from(value)),
                "desktop" => desktop = Some(PathBuf::from(value)),
                "icon" => icon = Some(PathBuf::from(value)),
                "source" => source = Some(PathBuf::from(value)),
                _ => {}
            }
        }

        Some(Record {
            name: name?,
            version,
            appimage: appimage?,
            desktop: desktop?,
            icon,
            source: source.unwrap_or_default(),
        })
    }
}

/// A record is plain text the user could edit, so only ever delete paths that
/// sit under ~/.local and cannot climb out of it.
fn is_managed_path(path: &Path, home: &Path) -> bool {
    path.starts_with(home.join(".local"))
        && !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

fn clean_desktop_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace(['\n', '\r', '\t'], " ")
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

enum IconKind {
    Png { width: u32 },
    Svg,
}

fn detect_icon(bytes: &[u8]) -> Option<IconKind> {
    const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

    if bytes.starts_with(PNG_MAGIC) && bytes.len() >= 24 {
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        return Some(IconKind::Png { width });
    }

    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    head.contains("<svg").then_some(IconKind::Svg)
}

/// Smallest standard hicolor size that fits `width`, capped at 512.
fn hicolor_size(width: u32) -> u32 {
    [16, 22, 24, 32, 48, 64, 128, 256, 512]
        .into_iter()
        .find(|size| *size >= width)
        .unwrap_or(512)
}

/// Extract the AppImage's icon into the user's hicolor theme. Icons are a
/// nicety, so any failure just means the generic icon is used.
fn install_icon(source: &Path, info: &AppMetadata, paths: &Paths) -> Option<PathBuf> {
    let bytes = metadata::read_icon(source, info).ok()?;

    let (directory, extension) = match detect_icon(&bytes)? {
        IconKind::Svg => ("scalable".to_string(), "svg"),
        IconKind::Png { width } => {
            let size = hicolor_size(width);
            (format!("{size}x{size}"), "png")
        }
    };

    let destination = paths
        .icon_root()
        .join(directory)
        .join("apps")
        .join(format!("{}.{extension}", paths.icon_name()));

    fs::create_dir_all(destination.parent()?).ok()?;

    let temp = temporary_path(&destination).ok()?;
    let written = create_temp_file(&temp)
        .and_then(|mut file| file.write_all(&bytes).and_then(|()| file.sync_all()))
        .and_then(|()| fs::rename(&temp, &destination));

    if written.is_err() {
        let _ = fs::remove_file(&temp);
        return None;
    }

    Some(destination)
}

/// Ask the desktop to notice the new or removed launcher and icon. Both tools
/// are optional, so missing ones are ignored.
fn refresh_desktop_caches(paths: &Paths) {
    let _ = Command::new("update-desktop-database")
        .arg(paths.home.join(".local/share/applications"))
        .output();

    let _ = Command::new("gtk-update-icon-cache")
        .args(["-q", "-f", "-t"])
        .arg(paths.icon_root())
        .output();
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

fn commit(temp: &Path, destination: &Path, overwrite: bool) -> io::Result<()> {
    if overwrite {
        // rename() atomically replaces the destination, and a running copy of
        // the old AppImage keeps working because its inode stays alive.
        fs::rename(temp, destination)
    } else {
        commit_without_overwrite(temp, destination)
    }
}

fn read_record(paths: &Paths) -> Option<Record> {
    fs::read_to_string(&paths.record)
        .ok()
        .and_then(|contents| Record::parse(&contents))
}

/// `None` when the app is not installed; otherwise the installed version,
/// which is itself `None` for installs that never recorded one.
pub fn installed_version(info: &AppMetadata) -> Option<Option<String>> {
    if !is_installed(info) {
        return None;
    }

    Some(
        paths(info)
            .ok()
            .and_then(|p| read_record(&p))
            .and_then(|record| record.version),
    )
}

pub fn install(source: &Path, info: &AppMetadata) -> io::Result<PathBuf> {
    deploy(source, info, false)
}

/// Replace an installed app with another AppImage of it (newer or older).
pub fn update(source: &Path, info: &AppMetadata) -> io::Result<PathBuf> {
    deploy(source, info, true)
}

fn deploy(source: &Path, info: &AppMetadata, overwrite: bool) -> io::Result<PathBuf> {
    validate_source(source)?;

    let paths = paths(info)?;

    if overwrite {
        if !is_installed(info) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not installed, so it cannot be updated", info.name),
            ));
        }
    } else if is_installed(info) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is already installed or has leftover files", info.name),
        ));
    }

    let destination = paths.appimage.clone();
    let desktop_file = paths.desktop.clone();
    let old_icon = read_record(&paths).and_then(|record| record.icon);

    let app_dir = destination
        .parent()
        .ok_or_else(|| io::Error::other("Invalid installation path"))?;

    let desktop_dir = desktop_file
        .parent()
        .ok_or_else(|| io::Error::other("Invalid desktop entry path"))?;

    let record_dir = paths
        .record
        .parent()
        .ok_or_else(|| io::Error::other("Invalid record path"))?;

    fs::create_dir_all(app_dir)?;
    fs::create_dir_all(desktop_dir)?;
    fs::create_dir_all(record_dir)?;

    let app_temp = temporary_path(&destination)?;
    let desktop_temp = temporary_path(&desktop_file)?;
    let record_temp = temporary_path(&paths.record)?;

    let mut installed_icon: Option<PathBuf> = None;

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

        commit(&app_temp, &destination, overwrite)?;

        installed_icon = install_icon(source, info, &paths);

        // An update that fails to extract an icon keeps the existing one.
        let icon_file = installed_icon.clone().or_else(|| old_icon.clone());

        let icon = if icon_file.is_some() {
            paths.icon_name()
        } else {
            info.icon
                .as_deref()
                .filter(|icon| !icon.is_empty() && !icon.contains('/'))
                .unwrap_or("application-x-executable")
                .to_owned()
        };

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
            clean_desktop_value(&icon),
            categories(info),
        );

        {
            let mut output = create_temp_file(&desktop_temp)?;
            output.write_all(desktop_entry.as_bytes())?;
            output.sync_all()?;
        }

        fs::set_permissions(&desktop_temp, fs::Permissions::from_mode(0o644))?;

        commit(&desktop_temp, &desktop_file, overwrite)?;

        let record = Record {
            name: info.name.clone(),
            version: info.version.clone(),
            appimage: destination.clone(),
            desktop: desktop_file.clone(),
            icon: icon_file,
            source: source.to_path_buf(),
        };

        {
            let mut output = create_temp_file(&record_temp)?;
            output.write_all(record.serialize().as_bytes())?;
            output.sync_all()?;
        }

        commit(&record_temp, &paths.record, overwrite)?;

        Ok(())
    })();

    // Remove temporary files whether installation succeeded or failed.
    let _ = fs::remove_file(&app_temp);
    let _ = fs::remove_file(&desktop_temp);
    let _ = fs::remove_file(&record_temp);

    if let Err(error) = install_result {
        // A failed update leaves the existing install alone. A failed fresh
        // install rolls back only the managed paths it created.
        if !overwrite {
            let _ = fs::remove_file(&paths.record);
            let _ = fs::remove_file(&desktop_file);
            let _ = fs::remove_file(&destination);

            if let Some(icon) = &installed_icon {
                let _ = fs::remove_file(icon);
            }
        }

        return Err(error);
    }

    // The new version may ship a different icon size or format.
    if let (Some(old), Some(new)) = (&old_icon, &installed_icon)
        && old != new
        && is_managed_path(old, &paths.home)
    {
        let _ = fs::remove_file(old);
    }

    refresh_desktop_caches(&paths);

    Ok(destination)
}

/// Remove only AppDrop-managed files. Never delete the original download.
pub fn uninstall(info: &AppMetadata) -> io::Result<()> {
    remove_install(&paths(info)?, &info.name)
}

/// Uninstall by library id, without needing the original AppImage.
pub fn uninstall_id(id: &str, name: &str) -> io::Result<()> {
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid application id",
        ));
    }

    remove_install(&Paths::new(home_dir()?, id.to_owned()), name)
}

/// Uses the install record when there is one, so it works even if the paths
/// would be computed differently today.
fn remove_install(paths: &Paths, name: &str) -> io::Result<()> {
    let record = read_record(paths);

    let mut targets = vec![paths.appimage.clone(), paths.desktop.clone()];

    if let Some(record) = &record {
        for path in [&record.appimage, &record.desktop]
            .into_iter()
            .chain(record.icon.as_ref())
        {
            if is_managed_path(path, &paths.home) && !targets.contains(path) {
                targets.push(path.clone());
            }
        }
    }

    targets.retain(|path| fs::symlink_metadata(path).is_ok());

    let record_exists = fs::symlink_metadata(&paths.record).is_ok();

    if targets.is_empty() && !record_exists {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{name} is not installed by AppDrop"),
        ));
    }

    // Attempt every removal, even if one fails. The record goes last so a
    // failed uninstall can be retried.
    let failures: Vec<String> = targets
        .iter()
        .filter_map(|path| {
            fs::remove_file(path)
                .err()
                .map(|e| format!("{}: {e}", path.display()))
        })
        .collect();

    if !failures.is_empty() {
        return Err(io::Error::other(format!(
            "Could not remove all AppDrop files. {}",
            failures.join("; ")
        )));
    }

    if record_exists {
        fs::remove_file(&paths.record)?;
    }

    refresh_desktop_caches(paths);

    Ok(())
}

/// One application AppDrop has installed, as shown in the library.
#[derive(Debug, Clone)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub appimage: PathBuf,
    pub icon: Option<PathBuf>,
    pub size: u64,
}

fn desktop_name(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("Name=").map(str::to_owned))
}

fn ids_in(directory: &Path, extension: &str) -> Vec<String> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?;

            (path.extension()? == extension && !stem.starts_with('.')).then(|| stem.to_owned())
        })
        .collect()
}

/// Everything AppDrop has installed, sorted by name. Installs from before
/// records existed are found through their AppImage in the managed folder.
pub fn list_installed() -> Vec<InstalledApp> {
    let Ok(home) = home_dir() else {
        return Vec::new();
    };

    let mut ids = ids_in(&home.join(OPT_DIR), "AppImage");
    ids.extend(ids_in(&home.join(RECORD_DIR), "record"));
    ids.sort();
    ids.dedup();

    let mut apps: Vec<InstalledApp> = ids
        .into_iter()
        .filter_map(|id| {
            let paths = Paths::new(home.clone(), id.clone());
            let size = fs::metadata(&paths.appimage).ok()?.len();
            let record = read_record(&paths);

            let name = record
                .as_ref()
                .map(|r| r.name.clone())
                .or_else(|| desktop_name(&paths.desktop))
                .unwrap_or_else(|| id.clone());

            Some(InstalledApp {
                version: record.as_ref().and_then(|r| r.version.clone()),
                icon: record
                    .and_then(|r| r.icon)
                    .filter(|icon| icon.exists() && is_managed_path(icon, &home)),
                appimage: paths.appimage,
                id,
                name,
                size,
            })
        })
        .collect();

    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

/// Start an installed app, detached from AppDrop.
pub fn launch(app: &InstalledApp) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    let mut child = Command::new(&app.appimage)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    // Reap the child when it exits so it doesn't linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, exec: &str) -> AppMetadata {
        AppMetadata {
            name: name.into(),
            icon: None,
            exec: Some(exec.into()),
            categories: vec![],
            desktop_file: "app.desktop".into(),
            icon_path: None,
            version: None,
            squashfs_offset: 0,
        }
    }

    #[test]
    fn slugify_normalises_names() {
        assert_eq!(slugify("My App 2.0!"), "my-app-2-0");
        assert_eq!(slugify("---"), "appimage");
        assert_eq!(slugify(""), "appimage");
    }

    #[test]
    fn app_id_is_stable_and_distinguishes_apps() {
        let a = info("App", "app");
        assert_eq!(app_id(&a), app_id(&info("App", "app")));
        assert_ne!(app_id(&a), app_id(&info("App", "other")));
        assert!(app_id(&a).starts_with("app-"));
    }

    #[test]
    fn exec_arguments_are_quoted_and_escaped() {
        assert_eq!(quote_exec_arg("/a b/c"), "\"/a b/c\"");
        assert_eq!(quote_exec_arg("a\"$`\\"), "\"a\\\"\\$\\`\\\\\"");
    }

    #[test]
    fn desktop_values_cannot_inject_lines() {
        assert_eq!(clean_desktop_value("a\nExec=evil"), "a Exec=evil");
    }

    #[test]
    fn categories_are_filtered_and_defaulted() {
        let mut app = info("A", "a");
        assert_eq!(categories(&app), "Utility;");

        app.categories = vec!["Game".into(), "bad cat".into(), "".into(), "Tools".into()];
        assert_eq!(categories(&app), "Game;Tools;");
    }

    #[test]
    fn detects_icon_formats() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        png.extend_from_slice(&300u32.to_be_bytes());
        png.extend_from_slice(&[0; 8]);

        assert!(matches!(
            detect_icon(&png),
            Some(IconKind::Png { width: 300 })
        ));
        assert!(matches!(
            detect_icon(b"<?xml version=\"1.0\"?><svg></svg>"),
            Some(IconKind::Svg)
        ));
        assert!(detect_icon(b"not an icon").is_none());
    }

    #[test]
    fn icon_sizes_round_up_to_standard_sizes() {
        assert_eq!(hicolor_size(10), 16);
        assert_eq!(hicolor_size(48), 48);
        assert_eq!(hicolor_size(300), 512);
        assert_eq!(hicolor_size(4096), 512);
    }

    #[test]
    fn record_round_trips() {
        let record = Record {
            name: "Demo App".into(),
            version: Some("1.2.3".into()),
            appimage: "/h/.local/opt/appdrop/x.AppImage".into(),
            desktop: "/h/.local/share/applications/appdrop-x.desktop".into(),
            icon: Some("/h/.local/share/icons/hicolor/256x256/apps/appdrop-x.png".into()),
            source: "/h/Downloads/Demo.AppImage".into(),
        };

        assert_eq!(Record::parse(&record.serialize()), Some(record));
        assert_eq!(Record::parse("name=only"), None);
    }

    #[test]
    fn only_paths_under_local_are_managed() {
        let home = Path::new("/home/u");

        assert!(is_managed_path(Path::new("/home/u/.local/opt/a"), home));
        assert!(!is_managed_path(Path::new("/home/u/Documents/a"), home));
        assert!(!is_managed_path(
            Path::new("/home/u/.local/../.ssh/key"),
            home
        ));
        assert!(!is_managed_path(Path::new("/etc/passwd"), home));
    }
}
