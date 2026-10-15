//! The documented one-line install fetches the installer published with a
//! release, never the moving `main` branch (ADR 0026). A guard, because the
//! command is repeated across the guide, the README, and the interface.

use std::path::{Path, PathBuf};

const FROM_MAIN: &str = "raw.githubusercontent.com/FieldmouseWorks/consolebook/main/install";
const FROM_RELEASE: &str =
    "github.com/FieldmouseWorks/consolebook/releases/latest/download/install.sh";

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "node_modules" || name == "build" || name == ".svelte-kit" {
            continue;
        }
        if path.is_dir() {
            files_under(&path, out);
        } else if path.extension().is_some_and(|ext| {
            ["md", "sh", "ts", "svelte", "yml"]
                .iter()
                .any(|e| ext == *e)
        }) {
            out.push(path);
        }
    }
}

#[test]
fn install_commands_fetch_the_released_installer() {
    let root = repository();
    let mut files = vec![root.join("README.md")];
    for dir in ["docs", "install", "web/src", ".github"] {
        files_under(&root.join(dir), &mut files);
    }
    let offenders: Vec<String> = files
        .iter()
        .filter(|path| {
            std::fs::read_to_string(path)
                .expect("read")
                .contains(FROM_MAIN)
        })
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "install commands must fetch {FROM_RELEASE}, not main: {offenders:?}"
    );
    let readme = std::fs::read_to_string(root.join("README.md")).expect("README");
    assert!(readme.contains(FROM_RELEASE), "the README leads with it");
}

/// The release workflow stamps the published installer by replacing this
/// exact line; if it changed, the stamp would silently miss and the
/// installer from `releases/latest` would fetch whatever release is latest
/// at that moment, not its own.
#[test]
fn the_installer_carries_the_line_the_release_stamps() {
    let script =
        std::fs::read_to_string(repository().join("install/install.sh")).expect("install.sh");
    let lines = script
        .lines()
        .filter(|line| line.starts_with("RELEASED_VERSION="))
        .collect::<Vec<_>>();
    assert_eq!(lines, [r#"RELEASED_VERSION="""#]);
    assert!(script.contains(r#"version="$RELEASED_VERSION""#));
}
