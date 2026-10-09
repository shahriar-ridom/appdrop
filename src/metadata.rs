use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::appimage::is_appimage;

#[derive(Debug, Clone)]
pub struct AppMetadata {
    pub name: String,
    pub icon: Option<String>,
    pub exec: Option<String>,
    pub categories: Vec<String>,
    pub desktop_file: String,
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
    let output = Command::new("unsquashfs")
        .args(args)
        .output()
        .map_err(|e| {
            MetadataError::ToolFailed(format!(
                "{e}. Install it with: sudo pacman -S squashfs-tools"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(MetadataError::ToolFailed(stderr.trim().to_owned()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn find_squashfs_offset(path: &Path) -> Result<u64, MetadataError> {
    // Initial implementation: scan for candidate signatures.
    // A signature alone is not trusted; each candidate is validated.
    let data = fs::read(path)?;

    for (offset, window) in data.windows(4).enumerate() {
        if window != b"hsqs" {
            continue;
        }

        let offset = offset as u64;

        let args = vec![
            "-s".to_string(),
            "-offset".to_string(),
            offset.to_string(),
            path.to_string_lossy().into_owned(),
        ];

        if let Ok(output) = run_unsquashfs(&args) {
            if output.contains("Found a valid SQUASHFS") {
                return Ok(offset);
            }
        }
    }

    Err(MetadataError::MissingSquashfs)
}

fn parse_desktop_entry(contents: &str, desktop_file: String, offset: u64) -> AppMetadata {
    let mut in_desktop_entry = false;
    let mut name = None;
    let mut icon = None;
    let mut exec = None;
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
        squashfs_offset: offset,
    }
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

    let listing = run_unsquashfs(&args)?;

    // Find the first desktop entry in the SquashFS listing.
    let desktop_file = listing
        .lines()
        .filter_map(|line| {
            let marker = "squashfs-root/";
            let start = line.find(marker)? + marker.len();
            let candidate = line[start..].trim();

            if candidate.ends_with(".desktop") {
                Some(candidate.to_owned())
            } else {
                None
            }
        })
        .next()
        .ok_or(MetadataError::MissingDesktopEntry)?;

    let args = vec![
        "-cat".to_string(),
        "-offset".to_string(),
        offset.to_string(),
        path.to_string_lossy().into_owned(),
        desktop_file.clone(),
    ];

    let contents = run_unsquashfs(&args)?;

    Ok(parse_desktop_entry(&contents, desktop_file, offset))
}

// Reserved for the next milestone: resolving extracted icon assets.
#[allow(dead_code)]
fn _appimage_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}
