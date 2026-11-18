//! Taking the executable out of a release archive, in memory.
//!
//! The release workflow packs `hookah-<version>-<triple>/{hookah,LICENSE,README.md}` into a
//! `.tar.gz` (Linux, macOS) or a `.zip` (Windows, with `hookah.exe`). Only the one regular file
//! with the executable's name, at most one directory deep, is read; nothing is written to disk
//! by path, so an archive cannot place a file anywhere.

use std::io::{Cursor, Read};
use std::path::{Component, Path};

use hookah_application::ports::UpdateError;
use hookah_domain::update::ArchiveKind;

/// The largest executable accepted.
const MAX_BINARY_BYTES: u64 = 134_217_728;

/// The content of `entry` inside the archive `bytes`.
pub fn extract(bytes: &[u8], kind: ArchiveKind, entry: &str) -> Result<Vec<u8>, UpdateError> {
    let found = match kind {
        ArchiveKind::TarGz => from_tar_gz(bytes, entry),
        ArchiveKind::Zip => from_zip(bytes, entry),
    }
    .map_err(|e| UpdateError::failed(format!("cannot read the release archive: {e}")))?;
    found.ok_or_else(|| UpdateError::failed(format!("the release archive has no {entry}")))
}

/// Whether `path` is `entry` or `<dir>/entry`, with no `..` or absolute parts.
fn is_entry(path: &Path, entry: &str) -> bool {
    let parts: Vec<Component<'_>> = path.components().collect();
    let plain = parts.iter().all(|c| matches!(c, Component::Normal(_)));
    plain && parts.len() <= 2 && path.file_name().is_some_and(|name| name == entry)
}

/// Reads all of `reader`, failing when it holds more than `max` bytes.
fn read_limited(reader: impl Read, max: u64) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    reader.take(max.saturating_add(1)).read_to_end(&mut out)?;
    if out.len() as u64 > max {
        return Err(std::io::Error::other("the executable is implausibly large"));
    }
    Ok(out)
}

fn from_tar_gz(bytes: &[u8], entry: &str) -> std::io::Result<Option<Vec<u8>>> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for item in archive.entries()? {
        let item = item?;
        if item.header().entry_type().is_file() && is_entry(&item.path()?, entry) {
            return read_limited(item, MAX_BINARY_BYTES).map(Some);
        }
    }
    Ok(None)
}

fn from_zip(bytes: &[u8], entry: &str) -> std::io::Result<Option<Vec<u8>>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(std::io::Error::other)?;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(std::io::Error::other)?;
        if file.is_file()
            && let Some(path) = file.enclosed_name()
            && is_entry(&path, entry)
        {
            return read_limited(file, MAX_BINARY_BYTES).map(Some);
        }
    }
    Ok(None)
}

/// Builds archives like the release workflow does, for the tests below.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod fixture {
    use std::io::Write;

    /// A `.tar.gz` holding each `(path, content)`.
    pub fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *content).unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    /// A `.zip` holding each `(path, content)`.
    pub fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (path, content) in files {
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file(*path, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::fixture::{tar_gz, zip};
    use super::*;

    const LAYOUT: [(&str, &[u8]); 3] = [
        ("hookah-1.0.0-x/LICENSE", b"license"),
        ("hookah-1.0.0-x/hookah", b"BINARY"),
        ("hookah-1.0.0-x/README.md", b"readme"),
    ];

    #[test]
    fn takes_the_executable_out_of_a_tar_gz() {
        let archive = tar_gz(&LAYOUT);
        assert_eq!(
            extract(&archive, ArchiveKind::TarGz, "hookah").unwrap(),
            b"BINARY"
        );
    }

    #[test]
    fn takes_the_executable_out_of_a_zip() {
        let archive = zip(&[
            ("hookah-1.0.0-x/LICENSE", b"license"),
            ("hookah-1.0.0-x/hookah.exe", b"EXE"),
        ]);
        assert_eq!(
            extract(&archive, ArchiveKind::Zip, "hookah.exe").unwrap(),
            b"EXE"
        );
    }

    #[test]
    fn a_bare_entry_without_a_directory_is_fine() {
        assert_eq!(
            extract(&tar_gz(&[("hookah", b"B")]), ArchiveKind::TarGz, "hookah").unwrap(),
            b"B"
        );
    }

    #[test]
    fn a_missing_entry_is_an_error() {
        let e = extract(&tar_gz(&LAYOUT), ArchiveKind::TarGz, "hookah.exe").unwrap_err();
        assert!(e.message().contains("no hookah.exe"));
        let e = extract(&zip(&[("d/other", b"x")]), ArchiveKind::Zip, "hookah.exe").unwrap_err();
        assert!(e.message().contains("no hookah.exe"));
    }

    #[test]
    fn only_a_shallow_regular_file_counts() {
        let deep = tar_gz(&[("a/b/hookah", b"deep")]);
        assert!(extract(&deep, ArchiveKind::TarGz, "hookah").is_err());
        let deep_zip = zip(&[("a/b/hookah.exe", b"deep")]);
        assert!(extract(&deep_zip, ArchiveKind::Zip, "hookah.exe").is_err());
        // A name that merely ends with the entry name is another file.
        let other = tar_gz(&[("d/not-hookah", b"x")]);
        assert!(extract(&other, ArchiveKind::TarGz, "hookah").is_err());
    }

    #[test]
    fn a_directory_entry_is_not_the_executable() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "d/hookah", std::io::empty())
            .unwrap();
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        let archive = gz.finish().unwrap();
        assert!(extract(&archive, ArchiveKind::TarGz, "hookah").is_err());
    }

    #[test]
    fn the_size_limit_is_inclusive() {
        assert_eq!(read_limited(&b"abcd"[..], 4).unwrap(), b"abcd");
        assert_eq!(read_limited(&b"abc"[..], 4).unwrap(), b"abc");
        let e = read_limited(&b"abcde"[..], 4).unwrap_err();
        assert!(e.to_string().contains("implausibly large"));
        assert_eq!(read_limited(&b""[..], 0).unwrap(), b"");
    }

    #[test]
    fn garbage_is_an_error_for_both_formats() {
        for kind in [ArchiveKind::TarGz, ArchiveKind::Zip] {
            let e = extract(b"this is not an archive", kind, "hookah").unwrap_err();
            assert!(e.message().contains("cannot read"), "{e}");
        }
    }

    #[test]
    fn a_wrong_kind_is_an_error() {
        assert!(extract(&zip(&[("hookah", b"x")]), ArchiveKind::TarGz, "hookah").is_err());
        assert!(extract(&tar_gz(&LAYOUT), ArchiveKind::Zip, "hookah").is_err());
    }

    #[test]
    fn entry_matching_rejects_traversal_and_absolute_paths() {
        assert!(is_entry(Path::new("hookah"), "hookah"));
        assert!(is_entry(Path::new("d/hookah"), "hookah"));
        assert!(!is_entry(Path::new("../hookah"), "hookah"));
        assert!(!is_entry(Path::new("/hookah"), "hookah"));
        assert!(!is_entry(Path::new("a/b/hookah"), "hookah"));
        assert!(!is_entry(Path::new("d/Hookah"), "hookah"));
    }
}
