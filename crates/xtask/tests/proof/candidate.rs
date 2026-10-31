//! The candidate as git has it: names read exactly, not as display text.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;

use xtask::proof::candidate::{name, state};
use xtask::proof::host::sha256;

use crate::{git, repo};

#[test]
fn an_untracked_name_is_hashed_as_it_is_on_disk() {
    let root = repo();
    let name = "caf\u{e9} 'notes'.txt";
    fs::write(root.path().join(name), "invented").unwrap();
    let s = state(root.path()).unwrap();
    assert_eq!(
        s.untracked_sha256.get(name),
        Some(&Some(sha256(b"invented"))),
        "{s:?}"
    );
    assert!(s.dirty);
}

#[test]
fn a_worktree_rename_keeps_its_source_and_whats_after_it() {
    let root = repo();
    let r = root.path();
    fs::write(r.join("README"), "invented readme\n").unwrap();
    git(r, &["add", "README"]);
    git(r, &["commit", "-q", "-m", "invented"]);
    fs::rename(r.join("README"), r.join("renamed")).unwrap();
    git(r, &["add", "-N", "renamed"]);
    fs::write(r.join("zzz-after"), "invented").unwrap();
    let s = state(r).unwrap();
    assert!(
        s.status.contains(&" R README -> renamed".to_owned()),
        "{s:?}"
    );
    assert!(s.untracked_sha256.contains_key("zzz-after"), "{s:?}");
}

/// No two names are recorded the same: not names that aren't UTF-8, not
/// one with the replacement character, not one that reads like an escape.
#[test]
fn every_name_is_kept_apart() {
    let root = repo();
    let r = root.path();
    let names: [&[u8]; 6] = [
        b"a\x80",
        b"a\x81",
        "a\u{fffd}".as_bytes(),
        b"a\\x80",
        b"a>b",
        "caf\u{e9}".as_bytes(),
    ];
    for (i, n) in names.iter().enumerate() {
        fs::write(r.join(OsStr::from_bytes(n)), format!("invented {i}")).unwrap();
    }
    let s = state(r).unwrap();
    assert_eq!(s.untracked_sha256.len(), names.len(), "{s:?}");
    for (i, n) in names.iter().enumerate() {
        assert_eq!(
            s.untracked_sha256.get(&name(n)),
            Some(&Some(sha256(format!("invented {i}").as_bytes()))),
            "{s:?}"
        );
    }
    assert_eq!(
        names.map(name),
        [
            "a\\x80",
            "a\\x81",
            "a\u{fffd}",
            "a\\\\x80",
            "a\\>b",
            "caf\u{e9}"
        ]
    );
    fs::write(r.join(OsStr::from_bytes(b"a\x81")), "changed").unwrap();
    assert_ne!(state(r).unwrap(), s, "a changed file went unseen");
}

/// A rename's two names split only one way, whatever they hold.
#[test]
fn a_renames_names_split_one_way() {
    let renamed = |from: &str, to: &str| {
        let root = repo();
        let r = root.path();
        fs::write(r.join(from), "invented\n").unwrap();
        git(r, &["add", "--", from]);
        git(r, &["commit", "-q", "-m", "invented"]);
        git(r, &["mv", "--", from, to]);
        state(r).unwrap().status
    };
    let one = renamed("a -> b", "c");
    assert_eq!(one, ["R  a -\\> b -> c"]);
    assert_ne!(one, renamed("a", "b -> c"));
}

#[test]
fn a_clean_candidate_is_its_commit() {
    let root = repo();
    let s = state(root.path()).unwrap();
    assert!(
        !s.dirty && s.status.is_empty() && s.untracked_sha256.is_empty(),
        "{s:?}"
    );
    assert_eq!(s.head.len(), 40);
    assert_eq!(s.tree.len(), 40);
}
