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

/// Hardening directives `install/consolebook.service` must carry under
/// `[Service]` (#125). Keep `UMask=0027` aligned with ADR 0023's `0750`
/// data/copy directories, and enforce defense-in-depth sandboxing below the
/// 4.0 exposure ceiling on `systemd-analyze security`.
const REQUIRED_UNIT_HARDENING: &[&str] = &[
    "UMask=0027",
    "NoNewPrivileges=true",
    "CapabilityBoundingSet=",
    "ProtectSystem=strict",
    "ProtectHome=true",
    "PrivateTmp=true",
    "PrivateDevices=true",
    "ProtectClock=true",
    "ProtectHostname=true",
    "ProtectKernelTunables=true",
    "ProtectKernelModules=true",
    "ProtectKernelLogs=true",
    "ProtectControlGroups=true",
    "ProtectProc=invisible",
    "RestrictNamespaces=true",
    "RestrictRealtime=true",
    "RestrictSUIDSGID=true",
    "LockPersonality=true",
    "MemoryDenyWriteExecute=true",
    "RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX",
    "SystemCallArchitectures=native",
    "SystemCallFilter=@system-service",
    "SystemCallFilter=~@privileged @resources",
];

fn missing_service_hardening(unit: &str) -> Vec<&'static str> {
    let mut in_service = false;
    let mut directives = Vec::new();
    for raw in unit.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_service = line == "[Service]";
            continue;
        }
        if in_service {
            directives.push(line);
        }
    }
    REQUIRED_UNIT_HARDENING
        .iter()
        .copied()
        .filter(|required| !directives.contains(required))
        .collect()
}

#[test]
fn the_systemd_unit_carries_its_hardening_directives() {
    let unit = std::fs::read_to_string(repository().join("install/consolebook.service"))
        .expect("consolebook.service");
    let missing = missing_service_hardening(&unit);
    assert!(
        missing.is_empty(),
        "install/consolebook.service is missing hardening directives: {missing:?}"
    );

    // Negative controls: removing or commenting out any required directive, or
    // moving it outside [Service], must be caught by the guard.
    for required in REQUIRED_UNIT_HARDENING {
        let dropped: String = unit
            .lines()
            .filter(|line| line.trim() != *required)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            missing_service_hardening(&dropped),
            vec![*required],
            "dropping `{required}` must fail the hardening check"
        );

        let commented: String = unit
            .lines()
            .map(|line| {
                if line.trim() == *required {
                    format!("# {line}")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            missing_service_hardening(&commented),
            vec![*required],
            "commenting out `{required}` must fail the hardening check"
        );
    }

    let wrong_section = unit.replacen("ProtectClock=true", "", 1) + "\n[Unit]\nProtectClock=true\n";
    assert_eq!(
        missing_service_hardening(&wrong_section),
        vec!["ProtectClock=true"],
        "a directive outside [Service] must not satisfy the guard"
    );
}
