//! The official release targets and the names of their assets.
//!
//! Each release carries one archive per target, named `hookah-<version>-<triple>.<ext>` (`tar.gz`
//! on Linux and macOS, `zip` on Windows), plus `SHA256SUMS`. The archive holds a directory of the
//! same name with the executable, `LICENSE`, and `README.md` in it.

use super::Version;

/// Name of the checksum list every release carries.
pub const CHECKSUMS_ASSET: &str = "SHA256SUMS";

/// How an archive is packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    /// A gzip-compressed tar archive.
    TarGz,
    /// A zip archive.
    Zip,
}

/// A platform hookah is released for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `x86_64-unknown-linux-musl`
    LinuxX64,
    /// `aarch64-unknown-linux-musl`
    LinuxArm64,
    /// `x86_64-apple-darwin`
    MacX64,
    /// `aarch64-apple-darwin`
    MacArm64,
    /// `x86_64-pc-windows-msvc`
    WindowsX64,
    /// `aarch64-pc-windows-msvc`
    WindowsArm64,
}

impl Target {
    /// The target for a Rust target triple; `None` for a platform that has no release.
    pub fn from_triple(triple: &str) -> Option<Self> {
        Some(match triple {
            "x86_64-unknown-linux-musl" => Self::LinuxX64,
            "aarch64-unknown-linux-musl" => Self::LinuxArm64,
            "x86_64-apple-darwin" => Self::MacX64,
            "aarch64-apple-darwin" => Self::MacArm64,
            "x86_64-pc-windows-msvc" => Self::WindowsX64,
            "aarch64-pc-windows-msvc" => Self::WindowsArm64,
            _ => return None,
        })
    }

    /// The Rust target triple, as used in asset names.
    pub fn triple(self) -> &'static str {
        match self {
            Self::LinuxX64 => "x86_64-unknown-linux-musl",
            Self::LinuxArm64 => "aarch64-unknown-linux-musl",
            Self::MacX64 => "x86_64-apple-darwin",
            Self::MacArm64 => "aarch64-apple-darwin",
            Self::WindowsX64 => "x86_64-pc-windows-msvc",
            Self::WindowsArm64 => "aarch64-pc-windows-msvc",
        }
    }

    /// How this target's archive is packed.
    pub fn archive_kind(self) -> ArchiveKind {
        match self {
            Self::WindowsX64 | Self::WindowsArm64 => ArchiveKind::Zip,
            _ => ArchiveKind::TarGz,
        }
    }

    /// The archive of `version`: `hookah-1.2.3-aarch64-apple-darwin.tar.gz`.
    pub fn archive_name(self, version: &Version) -> String {
        let ext = match self.archive_kind() {
            ArchiveKind::TarGz => "tar.gz",
            ArchiveKind::Zip => "zip",
        };
        format!("hookah-{version}-{}.{ext}", self.triple())
    }

    /// The executable inside the archive.
    pub fn binary_name(self) -> &'static str {
        match self.archive_kind() {
            ArchiveKind::TarGz => "hookah",
            ArchiveKind::Zip => "hookah.exe",
        }
    }
}

/// The SHA-256 of `asset` in a `SHA256SUMS` text (`<64 hex>  <name>`, a `*` before the name marks
/// binary mode), lowercased. `None` when the asset is not listed or its entry is malformed.
pub fn checksum_of(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start().trim_start_matches('*');
        let valid = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        (name == asset && valid).then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const ALL: [(&str, Target, ArchiveKind, &str); 6] = [
        (
            "x86_64-unknown-linux-musl",
            Target::LinuxX64,
            ArchiveKind::TarGz,
            "hookah",
        ),
        (
            "aarch64-unknown-linux-musl",
            Target::LinuxArm64,
            ArchiveKind::TarGz,
            "hookah",
        ),
        (
            "x86_64-apple-darwin",
            Target::MacX64,
            ArchiveKind::TarGz,
            "hookah",
        ),
        (
            "aarch64-apple-darwin",
            Target::MacArm64,
            ArchiveKind::TarGz,
            "hookah",
        ),
        (
            "x86_64-pc-windows-msvc",
            Target::WindowsX64,
            ArchiveKind::Zip,
            "hookah.exe",
        ),
        (
            "aarch64-pc-windows-msvc",
            Target::WindowsArm64,
            ArchiveKind::Zip,
            "hookah.exe",
        ),
    ];

    #[test]
    fn every_release_target_round_trips_with_its_archive_and_binary() {
        for (triple, target, kind, binary) in ALL {
            assert_eq!(Target::from_triple(triple), Some(target));
            assert_eq!(target.triple(), triple);
            assert_eq!(target.archive_kind(), kind);
            assert_eq!(target.binary_name(), binary);
        }
    }

    #[test]
    fn archive_names_match_the_release_workflow() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(
            Target::LinuxArm64.archive_name(&v),
            "hookah-1.2.3-aarch64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(
            Target::MacX64.archive_name(&v),
            "hookah-1.2.3-x86_64-apple-darwin.tar.gz"
        );
        assert_eq!(
            Target::WindowsX64.archive_name(&v),
            "hookah-1.2.3-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn other_platforms_have_no_release() {
        for triple in [
            "x86_64-unknown-linux-gnu",
            "i686-pc-windows-msvc",
            "x86_64-pc-windows-gnu",
            "aarch64-unknown-freebsd",
            "",
        ] {
            assert_eq!(Target::from_triple(triple), None, "{triple}");
        }
    }

    #[test]
    fn finds_a_checksum_by_exact_name() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  hookah-1.0.0-x.tar.gz\n{b} *hookah-1.0.0-y.zip\n");
        assert_eq!(checksum_of(&sums, "hookah-1.0.0-x.tar.gz"), Some(a));
        assert_eq!(
            checksum_of(&sums, "hookah-1.0.0-y.zip"),
            Some("b".repeat(64))
        );
        assert_eq!(checksum_of(&sums, "hookah-1.0.0"), None);
        assert_eq!(checksum_of(&sums, "x.tar.gz"), None);
    }

    #[test]
    fn a_malformed_entry_is_not_a_checksum() {
        let short = format!("{}  f\n", "a".repeat(63));
        let nonhex = format!("{}  f\n", "g".repeat(64));
        assert_eq!(checksum_of(&short, "f"), None);
        assert_eq!(checksum_of(&nonhex, "f"), None);
        assert_eq!(checksum_of("", "f"), None);
        assert_eq!(checksum_of("onlyonefield\n", "f"), None);
    }
}
