use std::cmp::Ordering;
use std::path::Path;

/// Guess a version from an AppImage file name, e.g. `Foo-v1.2.3-x86_64.AppImage`
/// gives `1.2.3`. Only dotted tokens count, so `x86_64` or `gow3` are ignored.
pub fn from_filename(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let stem = match name.len().checked_sub(".appimage".len()) {
        Some(split)
            if name.is_char_boundary(split) && name[split..].eq_ignore_ascii_case(".appimage") =>
        {
            &name[..split]
        }
        _ => name,
    };

    stem.split(['-', '_', ' '])
        .map(|token| token.strip_prefix(['v', 'V']).unwrap_or(token))
        .find(|token| {
            token.starts_with(|c: char| c.is_ascii_digit())
                && token.contains('.')
                && token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '+' || c == '~')
        })
        .map(|token| token.trim_end_matches('.').to_owned())
}

#[derive(Debug, PartialEq)]
enum Run<'a> {
    Number(u64),
    Text(&'a str),
}

fn runs(version: &str) -> Vec<Run<'_>> {
    let mut out = Vec::new();

    for segment in version.split(|c: char| !c.is_ascii_alphanumeric()) {
        let mut rest = segment;

        while let Some(first) = rest.chars().next() {
            let digits = first.is_ascii_digit();
            let end = rest
                .find(|c: char| c.is_ascii_digit() != digits)
                .unwrap_or(rest.len());
            let (run, tail) = rest.split_at(end);

            out.push(if digits {
                run.parse().map(Run::Number).unwrap_or(Run::Text(run))
            } else {
                Run::Text(run)
            });
            rest = tail;
        }
    }

    out
}

/// Natural version ordering. Numbers compare numerically, so 1.10 > 1.9, and a
/// trailing text run marks a pre-release, so 1.0-beta < 1.0.
pub fn compare(a: &str, b: &str) -> Ordering {
    let (a, b) = (runs(a), runs(b));

    for (x, y) in a.iter().zip(&b) {
        let ordering = match (x, y) {
            (Run::Number(x), Run::Number(y)) => x.cmp(y),
            (Run::Text(x), Run::Text(y)) => x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase()),
            (Run::Number(_), Run::Text(_)) => Ordering::Greater,
            (Run::Text(_), Run::Number(_)) => Ordering::Less,
        };

        if ordering != Ordering::Equal {
            return ordering;
        }
    }

    match a.len().cmp(&b.len()) {
        Ordering::Equal => Ordering::Equal,
        // The longer side wins if its extra part is a number (1.0.1 > 1.0),
        // and loses if it is text (1.0-beta < 1.0).
        Ordering::Greater => match a[b.len()] {
            Run::Number(0) => Ordering::Equal,
            Run::Number(_) => Ordering::Greater,
            Run::Text(_) => Ordering::Less,
        },
        Ordering::Less => match b[a.len()] {
            Run::Number(0) => Ordering::Equal,
            Run::Number(_) => Ordering::Less,
            Run::Text(_) => Ordering::Greater,
        },
    }
}

/// Label for the "replace the installed copy" button, or `None` when the
/// offered AppImage is the same version (or its version is unknown).
pub fn update_label(installed: Option<&str>, offered: Option<&str>) -> Option<String> {
    let offered = offered?;

    match installed {
        None => Some(format!("Update to {offered}")),
        Some(installed) => match compare(offered, installed) {
            Ordering::Greater => Some(format!("Update {installed} → {offered}")),
            Ordering::Less => Some(format!("Install older version {offered}")),
            Ordering::Equal => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(file: &str) -> Option<String> {
        from_filename(Path::new(file))
    }

    #[test]
    fn extracts_versions_from_file_names() {
        assert_eq!(
            name("hyprdeck-v0.1.0-x86_64.AppImage").as_deref(),
            Some("0.1.0")
        );
        assert_eq!(
            name("BitChord-1.8-linux-x86_64.AppImage").as_deref(),
            Some("1.8")
        );
        assert_eq!(name("Foo_2.0.1_amd64.appimage").as_deref(), Some("2.0.1"));
        assert_eq!(name("gow3-x86_64.AppImage"), None);
        assert_eq!(name("plain.AppImage"), None);
    }

    #[test]
    fn compares_naturally() {
        assert_eq!(compare("1.10", "1.9"), Ordering::Greater);
        assert_eq!(compare("1.2.0", "1.2"), Ordering::Equal);
        assert_eq!(compare("1.0.1", "1.0"), Ordering::Greater);
        assert_eq!(compare("1.0-beta", "1.0"), Ordering::Less);
        assert_eq!(compare("2.0", "10.0"), Ordering::Less);
        assert_eq!(compare("1.0-alpha", "1.0-beta"), Ordering::Less);
        assert_eq!(compare("1.4", "1.4"), Ordering::Equal);
    }

    #[test]
    fn update_labels() {
        assert_eq!(
            update_label(Some("1.2"), Some("1.4")).as_deref(),
            Some("Update 1.2 → 1.4")
        );
        assert_eq!(
            update_label(Some("1.4"), Some("1.2")).as_deref(),
            Some("Install older version 1.2")
        );
        assert_eq!(
            update_label(None, Some("1.4")).as_deref(),
            Some("Update to 1.4")
        );
        assert_eq!(update_label(Some("1.4"), Some("1.4")), None);
        assert_eq!(update_label(Some("1.4"), None), None);
    }
}
