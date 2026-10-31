//! The candidate as git has it: names read exactly, not as display text.

use std::fs;

use xtask::proof::candidate::state;
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
