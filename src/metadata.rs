use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::process::Command;

use crate::appimage::is_appimage;
use crate::version;

#[derive(Debug, Clone)]
pub struct AppMetadata {
    pub name: String,
    pub icon: Option<String>,
    pub exec: Option<String>,
    pub categories: Vec<String>,
    pub desktop_file: String,
    /// Path of the best icon file inside the SquashFS, if one was found.
    pub icon_path: Option<String>,
    /// From `X-AppImage-Version`, or guessed from the file name.
    pub version: Option<String>,
    pub squashfs_offset: u64,
}

#[derive(Debug)]
pub enum MetadataError {
    Io(std::io::Error),
    NotAppImage,
    MissingSquashfs,
    MissingDesktopEntry,
    ToolFailed(String),
}

impl std::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::NotAppImage => write!(f, "File is not a supported AppImage"),
            Self::MissingSquashfs => write!(f, "No valid SquashFS filesystem found"),
            Self::MissingDesktopEntry => write!(f, "No desktop entry found"),
            Self::ToolFailed(e) => write!(f, "unsquashfs failed: {e}"),
        }
    }
}

impl std::error::Error for MetadataError {}

impl From<std::io::Error> for MetadataError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

fn run_unsquashfs(args: &[String]) -> Result<String, MetadataError> {
    run_unsquashfs_bytes(args).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn run_unsquashfs_bytes(args: &[String]) -> Result<Vec<u8>, MetadataError> {
    let output = Command::new("unsquashfs")
        .args(args)
        .output()
        .map_err(|e| {
            MetadataError::ToolFailed(format!(
                "{e}. Install the squashfs-tools package (provides unsquashfs)"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(MetadataError::ToolFailed(stderr.trim().to_owned()));
    }

    Ok(output.stdout)
}

/// Returns true if `unsquashfs` accepts a SquashFS filesystem at `offset`.
fn is_valid_squashfs(path: &Path, offset: u64) -> bool {
    let args = vec![
        "-s".to_string(),
        "-offset".to_string(),
        offset.to_string(),
        path.to_string_lossy().into_owned(),
    ];

    run_unsquashfs(&args)
        .map(|output| output.contains("Found a valid SQUASHFS"))
        .unwrap_or(false)
}

/// A type-2 AppImage is an ELF runtime with the SquashFS image appended, so
/// the image starts where the ELF file ends: the end of the section header
/// table (e_shoff + e_shentsize * e_shnum).
fn elf_end_offset(path: &Path) -> io::Result<Option<u64>> {
    let mut header = [0u8; 0x40];

    if File::open(path)?.read_exact(&mut header).is_err() {
        return Ok(None);
    }

    let little = match header[5] {
        1 => true,
        2 => false,
        _ => return Ok(None),
    };

    let u16_at = |o: usize| {
        let bytes = [header[o], header[o + 1]];
        u64::from(if little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    };

    let (shoff, shentsize, shnum) = match header[4] {
        1 => {
            let bytes: [u8; 4] = header[0x20..0x24].try_into().unwrap();
            let shoff = if little {
                u32::from_le_bytes(bytes)
            } else {
                u32::from_be_bytes(bytes)
            };
            (u64::from(shoff), u16_at(0x2E), u16_at(0x30))
        }
        2 => {
            let bytes: [u8; 8] = header[0x28..0x30].try_into().unwrap();
            let shoff = if little {
                u64::from_le_bytes(bytes)
            } else {
                u64::from_be_bytes(bytes)
            };
            (shoff, u16_at(0x3A), u16_at(0x3C))
        }
        _ => return Ok(None),
    };

    Ok(shoff.checked_add(shentsize * shnum))
}

/// Fallback for images whose ELF headers don't point at the SquashFS: stream
/// the file in chunks looking for the magic, validating each candidate.
fn scan_for_squashfs(path: &Path) -> Result<u64, MetadataError> {
    const MAGIC: &[u8; 4] = b"hsqs";
    const CHUNK: usize = 1 << 20;

    let mut file = File::open(path)?;
    let mut buffer = vec![0u8; CHUNK + MAGIC.len() - 1];
    let mut carried = 0usize;
    let mut base = 0u64; // file offset of buffer[0]

    loop {
        let read = file.read(&mut buffer[carried..])?;
        if read == 0 {
            return Err(MetadataError::MissingSquashfs);
        }

        let filled = carried + read;

        for (i, window) in buffer[..filled].windows(MAGIC.len()).enumerate() {
            if window == MAGIC && is_valid_squashfs(path, base + i as u64) {
                return Ok(base + i as u64);
            }
        }

        // Keep the tail so a magic split across chunks is still found.
        let keep = (MAGIC.len() - 1).min(filled);
        buffer.copy_within(filled - keep..filled, 0);
        base += (filled - keep) as u64;
        carried = keep;
    }
}

fn find_squashfs_offset(path: &Path) -> Result<u64, MetadataError> {
    if let Some(offset) = elf_end_offset(path)?
        && is_valid_squashfs(path, offset)
    {
        return Ok(offset);
    }

    scan_for_squashfs(path)
}

fn parse_desktop_entry(contents: &str, desktop_file: String, offset: u64) -> AppMetadata {
    let mut in_desktop_entry = false;
    let mut name = None;
    let mut icon = None;
    let mut exec = None;
    let mut version = None;
    let mut categories = Vec::new();

    for raw_line in contents.lines() {
        let line = raw_line.trim();

        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }

        if !in_desktop_entry || line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key.trim() {
            "Name" => name = Some(value.trim().to_owned()),
            "Icon" => icon = Some(value.trim().to_owned()),
            "Exec" => exec = Some(value.trim().to_owned()),
            "X-AppImage-Version" => {
                version = Some(value.trim().to_owned()).filter(|v| !v.is_empty())
            }
            "Categories" => {
                categories = value
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            _ => {}
        }
    }

    AppMetadata {
        name: name.unwrap_or_else(|| {
            Path::new(&desktop_file)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown application")
                .to_owned()
        }),
        icon,
        exec,
        categories,
        desktop_file,
        icon_path: None,
        version,
        squashfs_offset: offset,
    }
}

/// File paths inside the SquashFS, relative to its root. Symlinks are listed
/// as "name -> target"; only the name is kept.
fn list_entries(listing: &str) -> Vec<String> {
    const MARKER: &str = "squashfs-root/";

    listing
        .lines()
        .filter_map(|line| {
            let start = line.find(MARKER)? + MARKER.len();
            let entry = line[start..].split(" -> ").next()?.trim();

            (!entry.is_empty()).then(|| entry.to_owned())
        })
        .collect()
}

/// Prefer a desktop entry at the image root; fall back to the first one found.
fn pick_desktop_file(entries: &[String]) -> Option<&String> {
    let mut desktop = entries.iter().filter(|e| e.ends_with(".desktop"));
    let first = desktop.next()?;

    if !first.contains('/') {
        return Some(first);
    }

    desktop.find(|e| !e.contains('/')).or(Some(first))
}

/// Pick the best icon for `icon_name`: scalable SVG first, then the largest
/// PNG, falling back to `.DirIcon`.
fn find_icon(entries: &[String], icon_name: &str) -> Option<String> {
    let best = entries
        .iter()
        .filter_map(|entry| {
            let file = entry.rsplit('/').next()?;
            let extension = file.rsplit_once('.').map(|(_, ext)| ext)?;

            let name_matches = file == icon_name
                || file
                    .strip_suffix(extension)
                    .and_then(|stem| stem.strip_suffix('.'))
                    == Some(icon_name);

            if !name_matches {
                return None;
            }

            let score = match extension {
                "svg" => u32::MAX,
                // Unsized (root-level) PNGs are usually large enough.
                "png" => entry
                    .split('/')
                    .find_map(|part| part.split_once('x')?.0.parse::<u32>().ok())
                    .unwrap_or(256),
                _ => return None,
            };

            Some((score, entry))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, entry)| entry.clone());

    best.or_else(|| entries.iter().find(|e| *e == ".DirIcon").cloned())
}

pub fn inspect(path: &Path) -> Result<AppMetadata, MetadataError> {
    if !is_appimage(path).map_err(MetadataError::Io)? {
        return Err(MetadataError::NotAppImage);
    }

    let offset = find_squashfs_offset(path)?;

    let args = vec![
        "-ll".to_string(),
        "-offset".to_string(),
        offset.to_string(),
        path.to_string_lossy().into_owned(),
    ];

    let entries = list_entries(&run_unsquashfs(&args)?);

    let desktop_file = pick_desktop_file(&entries)
        .cloned()
        .ok_or(MetadataError::MissingDesktopEntry)?;

    let args = vec![
        "-cat".to_string(),
        "-offset".to_string(),
        offset.to_string(),
        path.to_string_lossy().into_owned(),
        desktop_file.clone(),
    ];

    let contents = run_unsquashfs(&args)?;

    let mut info = parse_desktop_entry(&contents, desktop_file, offset);
    info.version = info.version.or_else(|| version::from_filename(path));

    info.icon_path = info
        .icon
        .as_deref()
        .and_then(|icon| find_icon(&entries, icon))
        .or_else(|| find_icon(&entries, ""));

    Ok(info)
}

/// Read the application's icon file out of the AppImage.
pub fn read_icon(path: &Path, info: &AppMetadata) -> Result<Vec<u8>, MetadataError> {
    let icon_path = info
        .icon_path
        .as_ref()
        .ok_or_else(|| MetadataError::ToolFailed("no icon found in the AppImage".into()))?;

    run_unsquashfs_bytes(&[
        "-cat".to_string(),
        "-offset".to_string(),
        info.squashfs_offset.to_string(),
        path.to_string_lossy().into_owned(),
        icon_path.clone(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_desktop_entry_main_section_only() {
        let contents = "[Desktop Entry]\nName=Demo\nName[de]=Beispiel\nIcon=demo\n\
                        Exec=demo %U\nCategories=Utility;Network;\n\
                        [Desktop Action x]\nName=Other\n";

        let info = parse_desktop_entry(contents, "demo.desktop".into(), 42);

        assert_eq!(info.name, "Demo");
        assert_eq!(info.icon.as_deref(), Some("demo"));
        assert_eq!(info.exec.as_deref(), Some("demo %U"));
        assert_eq!(info.categories, ["Utility", "Network"]);
        assert_eq!(info.squashfs_offset, 42);
    }

    #[test]
    fn name_falls_back_to_desktop_file_stem() {
        let info = parse_desktop_entry("[Desktop Entry]\n", "usr/share/foo.desktop".into(), 0);
        assert_eq!(info.name, "foo");
    }

    #[test]
    fn elf_end_offset_reads_section_table_end() {
        let mut header = [0u8; 0x40];
        header[..4].copy_from_slice(b"\x7fELF");
        header[4] = 2; // 64-bit
        header[5] = 1; // little endian
        header[0x28..0x30].copy_from_slice(&1000u64.to_le_bytes());
        header[0x3A..0x3C].copy_from_slice(&64u16.to_le_bytes());
        header[0x3C..0x3E].copy_from_slice(&3u16.to_le_bytes());

        let path = std::env::temp_dir().join(format!("appdrop-elf-test-{}", std::process::id()));
        std::fs::write(&path, header).unwrap();
        let offset = elf_end_offset(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(offset, Some(1000 + 64 * 3));
    }

    fn entries(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn listing_strips_symlink_targets() {
        let listing = "drwxr-xr-x root/root 0 2026-01-01 00:00 squashfs-root\n\
                       lrwxrwxrwx root/root 39 2026-01-01 00:00 squashfs-root/a.desktop -> usr/a.desktop\n\
                       -rw-r--r-- root/root 5 2026-01-01 00:00 squashfs-root/my app.png";

        assert_eq!(list_entries(listing), ["a.desktop", "my app.png"]);
    }

    #[test]
    fn prefers_root_desktop_entry() {
        let list = entries(&["usr/share/applications/a.desktop", "a.desktop"]);
        assert_eq!(pick_desktop_file(&list).unwrap(), "a.desktop");
    }

    #[test]
    fn icon_prefers_svg_then_largest_png() {
        let list = entries(&[
            "usr/share/icons/hicolor/48x48/apps/app.png",
            "usr/share/icons/hicolor/256x256/apps/app.png",
            "usr/share/icons/hicolor/scalable/apps/app.svg",
            "other.png",
        ]);
        assert_eq!(
            find_icon(&list, "app").unwrap(),
            "usr/share/icons/hicolor/scalable/apps/app.svg"
        );

        let list = entries(&[
            "usr/share/icons/hicolor/48x48/apps/app.png",
            "usr/share/icons/hicolor/256x256/apps/app.png",
        ]);
        assert_eq!(
            find_icon(&list, "app").unwrap(),
            "usr/share/icons/hicolor/256x256/apps/app.png"
        );
    }

    #[test]
    fn icon_falls_back_to_dir_icon() {
        let list = entries(&["unrelated.png", ".DirIcon"]);
        assert_eq!(find_icon(&list, "app").unwrap(), ".DirIcon");
        assert_eq!(find_icon(&entries(&["unrelated.png"]), "app"), None);
    }
}
