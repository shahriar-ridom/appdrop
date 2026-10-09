use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub fn is_appimage(path: &Path) -> io::Result<bool> {
    if !std::fs::metadata(path)?.is_file() {
        return Ok(false);
    }

    let mut file = File::open(path)?;
    let mut header = [0u8; 11];

    if file.read_exact(&mut header).is_err() {
        return Ok(false);
    }

    Ok(&header[0..4] == b"\x7fELF" && &header[8..10] == b"AI" && matches!(header[10], 1 | 2))
}
