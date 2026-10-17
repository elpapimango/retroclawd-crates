//! Version, build number and git commit of a binary or library, stamped at build time.
//!
//! A crate that wants to report itself adds this crate as a **build dependency**, calls [`emit`] from its `build.rs`, and then uses
//! [`build_info!`] (a [`BuildInfo`]) or [`version_tag!`] (a `&'static str`) in its code:
//!
//! ```ignore
//! // build.rs
//! fn main() { build_info::emit(); }
//! // lib.rs
//! const VERSION_TAG: &str = build_info::version_tag!(); // "0.1.0+412.a1b2c3d4e5.2026-10-17"
//! ```
//!
//! What is stamped, from the git repository the crate is built in:
//!
//! * **version** -- the crate's own `CARGO_PKG_VERSION`;
//! * **build number** -- `git rev-list --count HEAD`, the number of commits up to the one built from: it only ever grows, and
//!   needs no counter file;
//! * **commit** -- the abbreviated `HEAD` hash (the commit that was pushed, once it has been), and **date** -- that commit's date;
//! * **dirty** -- tracked files differ from `HEAD` (submodules and untracked files are ignored: a submodule reports for itself).
//!
//! Without git (a source tarball) the values come from the environment variables `RC_BUILD_NUMBER`, `RC_GIT_COMMIT`, `RC_GIT_DATE`
//! and `RC_GIT_DIRTY` (`1`), and are `0` / `unknown` otherwise.
//!
//! The *tag* is the whole stamp in one string with semver build metadata: `0.1.0+412.a1b2c3d4e5.2026-10-17` and, for a dirty tree,
//! `...2026-10-17.dirty`. Plugins put it in their descriptor's `version`; [`Info::parse`] reads it back in the host.

use std::fmt;

/// What a crate was built from (see the module documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: &'static str,
    pub build: &'static str,
    pub commit: &'static str,
    pub date: &'static str,
    pub dirty: bool,
}

impl BuildInfo {
    /// The tag form, `0.1.0+412.a1b2c3d4e5.2026-10-17[.dirty]`.
    pub fn tag(&self) -> String {
        format!(
            "{}+{}.{}.{}{}",
            self.version,
            self.build,
            self.commit,
            self.date,
            if self.dirty { ".dirty" } else { "" }
        )
    }

    pub fn info(&self) -> Info {
        Info {
            version: self.version.to_string(),
            build: self.build.to_string(),
            commit: self.commit.to_string(),
            date: self.date.to_string(),
            dirty: self.dirty,
        }
    }
}

impl fmt::Display for BuildInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.info().fmt(f)
    }
}

/// The same stamp as owned strings, as read back from a plugin's version tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub version: String,
    pub build: String,
    pub commit: String,
    pub date: String,
    pub dirty: bool,
}

impl Info {
    /// Reads a tag written by [`BuildInfo::tag`]. A plain version (`0.1.0`, from a plugin built without this crate) comes back with
    /// an empty build, commit and date.
    pub fn parse(tag: &str) -> Info {
        let Some((version, meta)) = tag.split_once('+') else {
            return Info {
                version: tag.to_string(),
                build: String::new(),
                commit: String::new(),
                date: String::new(),
                dirty: false,
            };
        };
        let mut parts: Vec<&str> = meta.split('.').collect();
        let dirty = parts.last() == Some(&"dirty");
        if dirty {
            parts.pop();
        }
        let part = |i: usize| parts.get(i).copied().unwrap_or_default().to_string();
        Info {
            version: version.to_string(),
            build: part(0),
            commit: part(1),
            date: part(2),
            dirty,
        }
    }
}

impl fmt::Display for Info {
    /// `0.1.0 build 412 (a1b2c3d4e5, 2026-10-17)`, with `, dirty` when the tree had changes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.version)?;
        if self.build.is_empty() && self.commit.is_empty() {
            return Ok(());
        }
        write!(f, " build {}", self.build)?;
        let mut detail: Vec<&str> = Vec::new();
        if !self.commit.is_empty() {
            detail.push(&self.commit);
        }
        if !self.date.is_empty() {
            detail.push(&self.date);
        }
        if self.dirty {
            detail.push("dirty");
        }
        if !detail.is_empty() {
            write!(f, " ({})", detail.join(", "))?;
        }
        Ok(())
    }
}

/// The [`BuildInfo`] of the crate this is written in. Needs [`emit`] in that crate's `build.rs`.
#[macro_export]
macro_rules! build_info {
    () => {
        $crate::BuildInfo {
            version: env!("CARGO_PKG_VERSION"),
            build: env!("RC_BUILD_NUMBER"),
            commit: env!("RC_GIT_COMMIT"),
            date: env!("RC_GIT_DATE"),
            dirty: !env!("RC_GIT_DIRTY_SUFFIX").is_empty(),
        }
    };
}

/// The tag of the crate this is written in, as a `&'static str` (for a plugin's descriptor). Needs [`emit`] in its `build.rs`.
#[macro_export]
macro_rules! version_tag {
    () => {
        concat!(
            env!("CARGO_PKG_VERSION"),
            "+",
            env!("RC_BUILD_NUMBER"),
            ".",
            env!("RC_GIT_COMMIT"),
            ".",
            env!("RC_GIT_DATE"),
            env!("RC_GIT_DIRTY_SUFFIX")
        )
    };
}

/// For a `build.rs`: asks git about the repository the crate lives in and prints the `cargo:rustc-env` lines the macros read.
pub fn emit() {
    use std::path::PathBuf;
    use std::process::Command;

    let dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_default();
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (!text.is_empty()).then_some(text)
    };
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());

    let commit = git(&["rev-parse", "--short=10", "HEAD"])
        .or_else(|| env("RC_GIT_COMMIT"))
        .unwrap_or_else(|| "unknown".to_string());
    let build = git(&["rev-list", "--count", "HEAD"])
        .or_else(|| env("RC_BUILD_NUMBER"))
        .unwrap_or_else(|| "0".to_string());
    let date = git(&["log", "-1", "--format=%cs", "HEAD"])
        .or_else(|| env("RC_GIT_DATE"))
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = match git(&[
        "status",
        "--porcelain",
        "--untracked-files=no",
        "--ignore-submodules=all",
    ]) {
        Some(_) => true,
        // `git` ran but printed nothing: clean. Without git, trust the environment.
        None => {
            git(&["rev-parse", "HEAD"]).is_none() && env("RC_GIT_DIRTY").as_deref() == Some("1")
        }
    };
    println!("cargo:rustc-env=RC_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=RC_BUILD_NUMBER={build}");
    println!("cargo:rustc-env=RC_GIT_DATE={date}");
    println!(
        "cargo:rustc-env=RC_GIT_DIRTY_SUFFIX={}",
        if dirty { ".dirty" } else { "" }
    );

    // Rerun when the checked-out commit changes (HEAD, or the branch it names, or the packed refs) and when the crate's own files
    // do (the dirty flag). A change in another crate of the same repository does not rerun this script, so the dirty flag can lag
    // until the next commit or the next edit here -- the commit and build number are exact.
    println!("cargo:rerun-if-changed=.");
    println!("cargo:rerun-if-env-changed=RC_BUILD_NUMBER");
    println!("cargo:rerun-if-env-changed=RC_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=RC_GIT_DATE");
    println!("cargo:rerun-if-env-changed=RC_GIT_DIRTY");
    let mut watch = vec![
        "HEAD".to_string(),
        "packed-refs".to_string(),
        "index".to_string(),
    ];
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(reference);
    }
    for name in watch {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            let path = PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                dir.join(path)
            };
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: BuildInfo = BuildInfo {
        version: "0.1.0",
        build: "412",
        commit: "a1b2c3d4e5",
        date: "2026-10-17",
        dirty: false,
    };

    #[test]
    fn the_tag_round_trips_and_reads_back_as_text() {
        let tag = INFO.tag();
        assert_eq!(tag, "0.1.0+412.a1b2c3d4e5.2026-10-17");
        let parsed = Info::parse(&tag);
        assert_eq!(parsed, INFO.info());
        assert_eq!(
            parsed.to_string(),
            "0.1.0 build 412 (a1b2c3d4e5, 2026-10-17)"
        );
        assert_eq!(INFO.to_string(), parsed.to_string());
    }

    #[test]
    fn a_dirty_tree_shows_in_the_tag_and_the_text() {
        let dirty = BuildInfo {
            dirty: true,
            ..INFO
        };
        assert_eq!(dirty.tag(), "0.1.0+412.a1b2c3d4e5.2026-10-17.dirty");
        let parsed = Info::parse(&dirty.tag());
        assert!(parsed.dirty);
        assert_eq!(parsed.commit, "a1b2c3d4e5");
        assert_eq!(
            parsed.to_string(),
            "0.1.0 build 412 (a1b2c3d4e5, 2026-10-17, dirty)"
        );
    }

    #[test]
    fn a_plain_version_from_an_older_plugin_is_still_readable() {
        let parsed = Info::parse("0.1.0");
        assert_eq!(parsed.version, "0.1.0");
        assert!(parsed.build.is_empty() && parsed.commit.is_empty() && !parsed.dirty);
        assert_eq!(parsed.to_string(), "0.1.0");
        // An unknown commit (built without git) is stated, not hidden.
        let unknown = Info::parse("0.1.0+0.unknown.unknown");
        assert_eq!(unknown.to_string(), "0.1.0 build 0 (unknown, unknown)");
    }
}
