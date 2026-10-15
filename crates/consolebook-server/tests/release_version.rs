//! The product version has one source, the Cargo workspace version, and
//! every other place that states it must agree (ADR 0022). These checks
//! fail a build where the web package or the changelog drifted from it.

use std::path::{Path, PathBuf};

use consolebook_server::{BUILD_COMMIT, BUILD_IDENTITY, VERSION};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|err| panic!("reading {relative}: {err}"))
}

#[test]
fn version_is_semver_core() {
    let parts: Vec<&str> = VERSION.split('.').collect();
    assert_eq!(parts.len(), 3, "{VERSION} is not MAJOR.MINOR.PATCH");
    for part in parts {
        assert!(
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()),
            "{VERSION} has a non-numeric component {part:?}"
        );
    }
}

#[test]
fn web_package_and_lockfile_state_the_cargo_version() {
    let package: serde_json::Value =
        serde_json::from_str(&read("web/package.json")).expect("web/package.json is JSON");
    assert_eq!(package["version"], VERSION, "web/package.json version");

    let lock: serde_json::Value = serde_json::from_str(&read("web/package-lock.json"))
        .expect("web/package-lock.json is JSON");
    assert_eq!(lock["version"], VERSION, "web/package-lock.json version");
    assert_eq!(
        lock["packages"][""]["version"], VERSION,
        "web/package-lock.json root package version"
    );
}

#[test]
fn changelog_has_a_section_for_the_current_version() {
    let changelog = read("CHANGELOG.md");
    let heading = format!("## [{VERSION}]");
    assert!(
        changelog.lines().any(|line| line.starts_with(&heading)),
        "CHANGELOG.md has no `{heading}` section"
    );
}

#[test]
fn build_identity_names_version_and_commit() {
    assert_ne!(BUILD_COMMIT, "");
    assert_eq!(BUILD_IDENTITY, format!("{VERSION} ({BUILD_COMMIT})"));
}
