#![allow(dead_code)] // each integration test uses a subset of these helpers

//! Fixture access.
//!
//! Fixtures are not committed — they are rebuilt on demand by
//! `fixtures/generate.mjs`, which is also what keeps them honest about what they
//! claim to contain. A test asks for one by name and gets it built if missing.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

static BUILD_LOCK: Mutex<()> = Mutex::new(());

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

/// Path to a fixture, generating it first if it is not there yet.
pub fn fixture(name: &str) -> PathBuf {
    let path = repo_root()
        .join("fixtures/out")
        .join(format!("{name}.xlsx"));
    if path.exists() {
        return path;
    }

    // One generator at a time: `cargo test` runs these in parallel threads and
    // two of them asking for the same missing fixture would race on the file.
    let _guard = BUILD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if path.exists() {
        return path;
    }

    let status = Command::new("node")
        .arg("fixtures/generate.mjs")
        .arg(name)
        .current_dir(repo_root())
        .status()
        .unwrap_or_else(|e| panic!("cannot run the fixture generator ({e}); is node installed?"));

    assert!(status.success(), "fixture generator failed for {name}");
    assert!(path.exists(), "generator produced nothing for {name}");

    path
}

/// Number of file descriptors this process holds. Linux only; used to prove a
/// reader gives them all back.
#[cfg(target_os = "linux")]
pub fn open_descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .map(|entries| entries.count())
        .unwrap_or(0)
}

/// One run of the `xlsx-measure` binary.
///
/// Peak RSS and descriptor counts are only meaningful per process, and
/// `cargo test` runs its tests as threads of a single one. So the measurements
/// happen in a child, and the tests read what it reports.
pub struct Report {
    raw: String,
}

impl Report {
    /// Numeric field, or `None` when the run reported `null` (a platform where
    /// the figure cannot be obtained).
    pub fn number(&self, key: &str) -> Option<f64> {
        self.field(key).and_then(|value| value.parse().ok())
    }

    /// String field, unquoted.
    pub fn text(&self, key: &str) -> Option<String> {
        self.field(key)
            .map(|value| value.trim_matches('"').to_owned())
    }

    fn field(&self, key: &str) -> Option<&str> {
        let needle = format!("\"{key}\":");
        let start = self.raw.find(&needle)? + needle.len();
        let rest = &self.raw[start..];
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

impl std::fmt::Debug for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

pub fn measure(args: &[&str]) -> Report {
    let output = Command::new(env!("CARGO_BIN_EXE_xlsx-measure"))
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("run xlsx-measure");

    assert!(
        output.status.success(),
        "xlsx-measure failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    Report {
        raw: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    }
}
