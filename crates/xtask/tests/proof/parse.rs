//! Reading the sequence: CONTRIBUTING.md's, and failing closed on drift.

use std::path::Path;

use xtask::proof::lex::{quote, runs_e2e, tokens};
use xtask::proof::sequence::{parse, with_system_browser};
use xtask::proof::{BROWSER_PLACEHOLDER, BROWSER_VAR, E2E_MARKER, SECTION};

fn contributing() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join("CONTRIBUTING.md")).unwrap()
}

#[test]
fn reads_the_repositorys_own_sequence() {
    let (steps, alt) = parse(&contributing()).unwrap();
    assert_eq!(steps.iter().filter(|s| runs_e2e(s)).count(), 1, "{steps:?}");
    assert!(
        alt.contains(&format!("{BROWSER_VAR}={BROWSER_PLACEHOLDER}")),
        "{alt}"
    );
    let swapped = with_system_browser(&steps, &alt, "/opt/x y/chromium").unwrap();
    assert!(
        swapped
            .iter()
            .any(|s| s.contains(&format!("{BROWSER_VAR}='/opt/x y/chromium'")))
            && !swapped.iter().any(|s| s.contains(BROWSER_PLACEHOLDER)),
        "{swapped:?}"
    );
    assert_eq!(swapped.len(), steps.len());
}

#[test]
fn an_invocation_of_the_suite_is_one() {
    for line in [
        format!("(cd web && npx playwright install chromium && {E2E_MARKER})"),
        format!("(cd web && {BROWSER_VAR}=/c {E2E_MARKER})"),
        format!("(cd web && {BROWSER_VAR}='/opt/x y/chromium' {E2E_MARKER})"),
        format!("exit 0 && {E2E_MARKER}"),
    ] {
        assert!(runs_e2e(&line), "{line}");
    }
}

#[test]
fn text_that_only_mentions_or_masks_the_suite_isnt() {
    for line in [
        format!("echo {E2E_MARKER}"),
        "npm run e2e-other".to_owned(),
        format!(": # ; {E2E_MARKER}"),
        format!("{E2E_MARKER} | true"),
        format!("{E2E_MARKER}; true"),
        format!("{E2E_MARKER} || true"),
        format!("{E2E_MARKER} & true"),
        // Unreadable: an open quote.
        format!("(cd web && '{E2E_MARKER}"),
        format!("{E2E_MARKER} \\"),
        format!("'{E2E_MARKER}'"),
    ] {
        assert!(!runs_e2e(&line), "{line}");
    }
}

/// The lexer reads as a POSIX shell does: quotes, escapes, comments to their
/// line's end, and runs of operators as one token.
#[test]
fn the_lexer_reads_as_the_shell_does() {
    let cases: [(&str, Option<&[&str]>); 9] = [
        ("a  'b c'\t\"d\\\"e\\f\"", Some(&["a", "b c", "d\"e\\f"])),
        ("x''y '' z", Some(&["xy", "", "z"])),
        ("(cd web&&npm)", Some(&["(", "cd", "web", "&&", "npm", ")"])),
        ("a;;b ))&&c", Some(&["a", ";;", "b", "))&&", "c"])),
        ("foo#bar baz", Some(&["foo"])),
        ("a # c\nb", Some(&["a", "b"])),
        ("\\(x\\ y", Some(&["(x y"])),
        ("'open", None),
        ("trailing\\", None),
    ];
    for (line, want) in cases {
        let want = want.map(|w| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert_eq!(tokens(line), want, "{line:?}");
    }
}

#[test]
fn a_path_is_quoted_only_when_it_must_be() {
    assert_eq!(quote("/opt/chromium-1.2/bin"), "/opt/chromium-1.2/bin");
    assert_eq!(quote("/opt/x y/chromium"), "'/opt/x y/chromium'");
    assert_eq!(quote("it's"), r#"'it'"'"'s'"#);
    assert_eq!(quote(""), "''");
}

#[test]
fn drift_fails_closed() {
    let alt = format!("(cd web && {BROWSER_VAR}={BROWSER_PLACEHOLDER} {E2E_MARKER})");
    // Each case fails on its own drift: the rest of it is well formed.
    let alt_block = format!("```sh\n{alt}\n```\n");
    let cases = [
        ("missing section", "# Contributing\n".to_owned()),
        ("missing block", format!("{SECTION}\n\nno commands\n")),
        (
            "unterminated block",
            format!("{SECTION}\n```sh\ncargo test\n"),
        ),
        ("empty block", format!("{SECTION}\n```sh\n\n```\n")),
        (
            "a gate block without the browser suite",
            format!("{SECTION}\n```sh\ncargo test\n```\n\n{alt_block}"),
        ),
        (
            "a duplicated browser suite",
            format!(
                "{SECTION}\n```sh\n(cd web && {E2E_MARKER})\n(cd web && {E2E_MARKER})\n```\n\n\
                 {alt_block}"
            ),
        ),
        (
            "a system-browser line that skips the suite",
            format!(
                "{SECTION}\n```sh\n(cd web && {E2E_MARKER})\n```\n\n```sh\n\
                 echo {BROWSER_VAR}={BROWSER_PLACEHOLDER}\n```\n"
            ),
        ),
        (
            "no system-browser alternative",
            format!("{SECTION}\n```sh\n(cd web && {E2E_MARKER})\n```\n"),
        ),
        (
            "a gate step that only mentions the suite",
            format!(
                "{SECTION}\n```sh\necho {E2E_MARKER}\n```\n\n```sh\n\
                 (cd web && {BROWSER_VAR}={BROWSER_PLACEHOLDER} {E2E_MARKER})\n```\n"
            ),
        ),
        (
            "a block that only follows the section",
            format!(
                "{SECTION}\n\n## Next\n```sh\n(cd web && {E2E_MARKER})\n```\n\n```sh\n\
                 (cd web && {BROWSER_VAR}={BROWSER_PLACEHOLDER} {E2E_MARKER})\n```\n"
            ),
        ),
    ];
    for (label, text) in cases {
        assert!(parse(&text).is_err(), "accepts {label}");
    }
    assert!(with_system_browser(&["cargo test".to_owned()], &alt, "/c").is_err());
    let twice = format!("{alt} # {BROWSER_PLACEHOLDER}");
    let steps = [format!("(cd web && {E2E_MARKER})")];
    assert!(with_system_browser(&steps, &twice, "/c").is_err());
    // The one well-formed sequence, for contrast.
    let good = format!(
        "{SECTION}\n```sh\n# a comment\ncargo test\n(cd web && {E2E_MARKER})\n```\n\n```sh\n{alt}\n```\n"
    );
    let (steps, got) = parse(&good).unwrap();
    assert_eq!(
        steps,
        ["cargo test".to_owned(), format!("(cd web && {E2E_MARKER})")]
    );
    assert_eq!(got, alt);
}
