//! Guard: no integration test declares its own credential literal (#126).
//!
//! Test credentials come from `tests/support/mod.rs`, which generates them
//! at run time. A credential written into a test file as a literal is what
//! static analysis reports as a hard-coded credential, and every new test
//! file that copied the pattern raised the same alerts again. This test
//! scans the test sources and fails on that pattern's return.
//!
//! # Scan contract
//!
//! **Scope.** Every `.rs` file under `crates/consolebook-server/tests/`,
//! recursively, including `support/` and this file.
//!
//! **Literal.** A string literal token: `"…"`, raw `r"…"` / `r#"…"#` (any
//! number of `#`), and the byte and C forms `b"…"`, `br#"…"#`, `c"…"`,
//! `cr#"…"#`. Text inside comments is not code and is skipped; character
//! literals are not string literals. A *literal expression* is a literal
//! as the first token of an expression, after any leading `&`, so a method
//! chain on it (`"…".to_owned()`) counts; `String::from(literal)` counts
//! too.
//!
//! **Sensitive name.** An identifier or JSON key that contains `password`,
//! `passphrase`, or `secret`, ignoring case (`PASSWORD`, `new_password`,
//! `PHC_PASSWORD`, `current_password`, `client_secret`, …).
//!
//! **Flagged shapes.**
//! 1. *Binding*: `const`, `static`, or `let` (optionally `mut`) of a
//!    sensitive name whose initializer is a literal expression.
//! 2. *Field*: `sensitive_name: literal` (a struct-literal field).
//! 3. *JSON key*: a sensitive key literal followed by `:` and a literal
//!    expression — the `json!` shape, matched wherever it appears.
//! 4. *Embedded JSON*: a literal whose text holds a sensitive JSON key
//!    with a string value (`"password":"…"`, escaped or raw). A value that
//!    begins with `{` is a format interpolation, not a credential.
//! 5. *Call*: a literal expression passed
//!    - as any argument to a function or method whose name is sensitive
//!      (`hash_password`, `verify_password`, `check_password_policy`);
//!    - as the last of two or more arguments to `initialize` or
//!      `use_reset_code`, whose last parameter is the password;
//!    - as the last argument to a function or method whose name contains
//!      `login` or `sign_in`, when another argument is a literal too. A
//!      sign-in helper's last parameter is its password when it takes
//!      one and its username when it does not (`live_login(&addr,
//!      "avery.admin")`), so a lone literal is a username; a username and
//!      a password both written in are two.
//!
//! **Allowlist.** Exact path, shape, and subject; each entry must still
//! match, so a stale entry fails too. Today there is one:
//! - `upgrade_compat.rs`, binding `PHC_PASSWORD`: the cross-version
//!   compatibility fixture. `PHC_FROM_ARGON2_0_5` was hashed from this
//!   exact password by the argon2 0.5 build; neither can be generated at
//!   run time without losing what the test proves.
//!
//! The credential-absence property test
//! (`installation_export/properties.rs`) reads stored credential columns as
//! its positive control. It declares no literal, so it needs no entry.
//!
//! **Not caught, by design.** A credential reaching a sink only through a
//! non-sensitive name (`let x = "…"; hash_password(x)`); a literal at a
//! non-final position of a sign-in or reset helper, or a sign-in helper's
//! literal password beside a username variable; a credential built
//! by `format!`, `concat!`, or arithmetic; literals in non-Rust fixtures
//! (`tests/fixtures/`); and anything outside `tests/`. Review covers
//! those; this guard stops the shape that recurred.
//!
//! Findings name the file, line, shape, and subject (the name, key, or
//! callee), never the literal's text.

use std::fmt;
use std::path::{Path, PathBuf};

const SENSITIVE: [&str; 3] = ["password", "passphrase", "secret"];

/// Callees whose last parameter is the password.
const LAST_ARGUMENT_CALLEES: [&str; 2] = ["initialize", "use_reset_code"];
/// Sign-in helpers: the last argument is the password when another
/// argument is a literal too.
const SIGN_IN_FRAGMENTS: [&str; 2] = ["login", "sign_in"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Binding,
    Field,
    JsonKey,
    EmbeddedJson,
    Call,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Finding {
    path: String,
    line: usize,
    shape: Shape,
    subject: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {:?} `{}`",
            self.path, self.line, self.shape, self.subject
        )
    }
}

struct Allowed {
    path: &'static str,
    shape: Shape,
    subject: &'static str,
    reason: &'static str,
}

const ALLOWLIST: [Allowed; 1] = [Allowed {
    path: "upgrade_compat.rs",
    shape: Shape::Binding,
    subject: "PHC_PASSWORD",
    reason: "cross-version fixture: PHC_FROM_ARGON2_0_5 was hashed from it by argon2 0.5",
}];

impl Allowed {
    fn covers(&self, finding: &Finding) -> bool {
        finding.path == self.path && finding.shape == self.shape && finding.subject == self.subject
    }
}

// ----------------------------------------------------------------- lexer

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Str(String),
    Punct(char),
    Other,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    line: usize,
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A string literal's prefix at `at`: (characters before the hashes,
/// raw?). `r"`, `r#"`, `br"`, `cr#"`, `b"`, `c"`.
fn string_prefix(chars: &[char], at: usize) -> Option<(usize, bool)> {
    let rest = |n: usize| chars.get(at + n).copied();
    let raw_after = |n: usize| {
        let mut k = n;
        while rest(k) == Some('#') {
            k += 1;
        }
        rest(k) == Some('"')
    };
    match (rest(0), rest(1)) {
        (Some('r'), _) if raw_after(1) => Some((1, true)),
        (Some('b' | 'c'), Some('r')) if raw_after(2) => Some((2, true)),
        (Some('b' | 'c'), Some('"')) => Some((1, false)),
        _ => None,
    }
}

fn tokenize(src: &str) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let (mut at, mut line) = (0, 1);
    while at < chars.len() {
        let (tok, end) = lex(&chars, at);
        if let Some(tok) = tok {
            tokens.push(Token { tok, line });
        }
        line += chars[at..end].iter().filter(|c| **c == '\n').count();
        at = end;
    }
    tokens
}

/// The token that starts at `at` (none for whitespace and comments) and
/// the index just past it.
fn lex(chars: &[char], at: usize) -> (Option<Tok>, usize) {
    let c = chars[at];
    let next = chars.get(at + 1).copied();
    let ident_end = |from: usize| {
        let mut i = from;
        while i < chars.len() && is_ident_char(chars[i]) {
            i += 1;
        }
        i
    };
    if c.is_whitespace() {
        (None, at + 1)
    } else if c == '/' && next == Some('/') {
        let end = chars[at..].iter().position(|c| *c == '\n');
        (None, end.map_or(chars.len(), |n| at + n))
    } else if c == '/' && next == Some('*') {
        (None, block_comment_end(chars, at))
    } else if let Some((skip, raw)) = string_prefix(chars, at) {
        let (text, end) = if raw {
            raw_string(chars, at + skip)
        } else {
            quoted_string(chars, at + skip)
        };
        (Some(Tok::Str(text)), end)
    } else if c == '"' {
        let (text, end) = quoted_string(chars, at);
        (Some(Tok::Str(text)), end)
    } else if c == 'r' && next == Some('#') && chars.get(at + 2).is_some_and(|c| is_ident_start(*c))
    {
        // A raw identifier, `r#name`.
        let end = ident_end(at + 2);
        (Some(Tok::Ident(chars[at + 2..end].iter().collect())), end)
    } else if is_ident_start(c) {
        let end = ident_end(at);
        (Some(Tok::Ident(chars[at..end].iter().collect())), end)
    } else if c.is_ascii_digit() {
        (Some(Tok::Other), ident_end(at))
    } else if c == '\'' {
        // A character literal (`'x'`, `'\n'`, `'\u{..}'`) or a lifetime.
        let end = if next == Some('\\') {
            let close = chars[at + 2..].iter().skip(1).position(|c| *c == '\'');
            close.map_or(chars.len(), |n| at + 4 + n)
        } else if chars.get(at + 2) == Some(&'\'') {
            at + 3
        } else {
            ident_end(at + 1)
        };
        (Some(Tok::Other), end.min(chars.len()))
    } else {
        (Some(Tok::Punct(c)), at + 1)
    }
}

/// The index just past the (nested) block comment that opens at `at`.
fn block_comment_end(chars: &[char], at: usize) -> usize {
    let (mut i, mut depth) = (at, 0usize);
    while i < chars.len() {
        if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
            depth += 1;
            i += 2;
        } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    chars.len()
}

/// `"…"` starting at the opening quote; returns its text and the index
/// after the closing quote.
fn quoted_string(chars: &[char], open: usize) -> (String, usize) {
    let mut i = open + 1;
    while i < chars.len() && chars[i] != '"' {
        i += if chars[i] == '\\' { 2 } else { 1 };
    }
    let end = i.min(chars.len());
    (
        chars[open + 1..end].iter().collect(),
        (i + 1).min(chars.len()),
    )
}

/// `#…#"…"#…#` starting at the first `#` or quote after the `r`.
fn raw_string(chars: &[char], at: usize) -> (String, usize) {
    let mut hashes = 0;
    while chars.get(at + hashes) == Some(&'#') {
        hashes += 1;
    }
    let body = at + hashes + 1;
    let mut i = body;
    while i < chars.len() {
        if chars[i] == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#')) {
            return (chars[body..i].iter().collect(), i + 1 + hashes);
        }
        i += 1;
    }
    (chars[body.min(chars.len())..].iter().collect(), chars.len())
}

// ----------------------------------------------------------------- rules

fn sensitive(name: &str) -> bool {
    let lower = name.to_lowercase();
    SENSITIVE.iter().any(|word| lower.contains(word))
}

fn ident(tokens: &[Token], at: usize) -> Option<&str> {
    match tokens.get(at).map(|t| &t.tok) {
        Some(Tok::Ident(name)) => Some(name),
        _ => None,
    }
}

fn punct(tokens: &[Token], at: usize, c: char) -> bool {
    tokens.get(at).map(|t| &t.tok) == Some(&Tok::Punct(c))
}

/// The line of the literal expression starting at `at`, if one does.
fn literal_expression(tokens: &[Token], mut at: usize) -> Option<usize> {
    while punct(tokens, at, '&') {
        at += 1;
    }
    if let Some(Token {
        tok: Tok::Str(_),
        line,
    }) = tokens.get(at)
    {
        return Some(*line);
    }
    let from = ident(tokens, at) == Some("String")
        && punct(tokens, at + 1, ':')
        && punct(tokens, at + 2, ':')
        && ident(tokens, at + 3) == Some("from")
        && punct(tokens, at + 4, '(');
    if from {
        return literal_expression(tokens, at + 5);
    }
    None
}

/// The start index of each argument of the call whose `(` is at `open`.
fn arguments(tokens: &[Token], open: usize) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut depth = 0usize;
    let mut at = open;
    while let Some(token) = tokens.get(at) {
        match token.tok {
            Tok::Punct('(' | '[' | '{') => {
                depth += 1;
                if depth == 1 {
                    starts.push(at + 1);
                }
            }
            Tok::Punct(')' | ']' | '}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Tok::Punct(',') if depth == 1 => starts.push(at + 1),
            _ => {}
        }
        at += 1;
    }
    // A trailing comma leaves an empty last argument.
    starts.retain(|start| !punct(tokens, *start, ')'));
    starts
}

/// The text of a literal holds a sensitive JSON key with a string value.
/// Every quote is tried as a key's opening quote, so a quote earlier in
/// the text cannot pair with the wrong one and hide a key.
fn embedded_json_key(text: &str) -> Option<String> {
    let text = text.replace("\\\"", "\"");
    let mut rest = text.as_str();
    while let Some(open) = rest.find('"') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('"') else { break };
        let key = &rest[..close];
        if sensitive(key)
            && let Some(value) = rest[close + 1..].trim_start().strip_prefix(':')
            && let Some(value) = value.trim_start().strip_prefix('"')
            && !value.starts_with('{')
        {
            return Some(key.to_owned());
        }
    }
    None
}

fn scan_source(path: &str, src: &str) -> Vec<Finding> {
    let tokens = tokenize(src);
    let mut findings = Vec::new();
    let mut flag = |line: usize, shape: Shape, subject: &str| {
        findings.push(Finding {
            path: path.to_owned(),
            line,
            shape,
            subject: subject.to_owned(),
        });
    };
    for (at, token) in tokens.iter().enumerate() {
        match &token.tok {
            Tok::Ident(keyword) if ["const", "static", "let"].contains(&keyword.as_str()) => {
                let mut name_at = at + 1;
                if ident(&tokens, name_at) == Some("mut") {
                    name_at += 1;
                }
                let Some(name) = ident(&tokens, name_at).filter(|name| sensitive(name)) else {
                    continue;
                };
                let mut eq = name_at + 1;
                while eq < tokens.len() && !punct(&tokens, eq, '=') && !punct(&tokens, eq, ';') {
                    eq += 1;
                }
                if punct(&tokens, eq, '=')
                    && let Some(line) = literal_expression(&tokens, eq + 1)
                {
                    flag(line, Shape::Binding, name);
                }
            }
            Tok::Ident(name)
                if sensitive(name)
                    && punct(&tokens, at + 1, ':')
                    && !punct(&tokens, at + 2, ':')
                    && (at == 0 || !punct(&tokens, at - 1, ':')) =>
            {
                if let Some(line) = literal_expression(&tokens, at + 2) {
                    flag(line, Shape::Field, name);
                }
            }
            Tok::Ident(callee) if punct(&tokens, at + 1, '(') => {
                if at > 0 && ident(&tokens, at - 1) == Some("fn") {
                    continue;
                }
                let args = arguments(&tokens, at + 1);
                let literals = args
                    .iter()
                    .filter(|start| literal_expression(&tokens, **start).is_some())
                    .count();
                let last_is_password = args.len() >= 2
                    && (LAST_ARGUMENT_CALLEES.contains(&callee.as_str())
                        || (SIGN_IN_FRAGMENTS.iter().any(|f| callee.contains(f)) && literals >= 2));
                let checked: &[usize] = if sensitive(callee) {
                    &args
                } else if last_is_password {
                    &args[args.len() - 1..]
                } else {
                    &[]
                };
                for start in checked {
                    if let Some(line) = literal_expression(&tokens, *start) {
                        flag(line, Shape::Call, callee);
                    }
                }
            }
            Tok::Str(text) => {
                if sensitive(text)
                    && punct(&tokens, at + 1, ':')
                    && let Some(line) = literal_expression(&tokens, at + 2)
                {
                    flag(line, Shape::JsonKey, text);
                }
                if let Some(key) = embedded_json_key(text) {
                    flag(token.line, Shape::EmbeddedJson, &key);
                }
            }
            _ => {}
        }
    }
    findings
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read test directory")
        .map(|entry| entry.expect("directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Every finding under `root`, with paths relative to it, and the files
/// scanned.
fn scan_tree(root: &Path) -> (Vec<Finding>, Vec<String>) {
    let mut files = Vec::new();
    rust_files(root, &mut files);
    let mut findings = Vec::new();
    let mut scanned = Vec::new();
    for file in files {
        let relative = file.strip_prefix(root).expect("under root");
        let label = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let src = std::fs::read_to_string(&file).expect("read test source");
        findings.extend(scan_source(&label, &src));
        scanned.push(label);
    }
    (findings, scanned)
}

fn not_allowlisted(findings: Vec<Finding>) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|finding| !ALLOWLIST.iter().any(|allowed| allowed.covers(finding)))
        .collect()
}

fn listing(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

// ----------------------------------------------------------------- tests

#[test]
fn no_test_declares_its_own_credential_literal() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let (findings, scanned) = scan_tree(&root);
    // The walk is real: it reaches nested modules and this file.
    for expected in [
        "installation_export/properties.rs",
        "support/mod.rs",
        "write_transactions/accounts.rs",
        "no_credential_literals.rs",
    ] {
        assert!(scanned.iter().any(|s| s == expected), "scanned {expected}");
    }
    for allowed in &ALLOWLIST {
        assert!(
            findings.iter().any(|finding| allowed.covers(finding)),
            "stale allowlist entry {} `{}` ({})",
            allowed.path,
            allowed.subject,
            allowed.reason
        );
    }
    let remaining = not_allowlisted(findings);
    assert!(
        remaining.is_empty(),
        "test credentials come from `tests/support` (`support::password()` and friends); \
         these declare their own:\n{}",
        listing(&remaining)
    );
}

/// One synthetic source per forbidden shape, with the finding it must
/// produce.
fn forbidden_cases() -> Vec<(String, Shape, &'static str)> {
    let key = |key: &str, value: &str| format!("json!({{ \"{key}\": {value} }})");
    vec![
        (
            r#"const PASSWORD: &'static str = "x";"#.into(),
            Shape::Binding,
            "PASSWORD",
        ),
        (
            r#"static ADMIN_SECRET: &'static str = "x";"#.into(),
            Shape::Binding,
            "ADMIN_SECRET",
        ),
        (
            r#"let new_password = "x";"#.into(),
            Shape::Binding,
            "new_password",
        ),
        (
            r#"let mut passphrase: String = "x".to_owned();"#.into(),
            Shape::Binding,
            "passphrase",
        ),
        (
            r#"let password = String::from("x");"#.into(),
            Shape::Binding,
            "password",
        ),
        (
            r##"const PASSWORD: &str = r#"x"#;"##.into(),
            Shape::Binding,
            "PASSWORD",
        ),
        (r#"let password = r"x";"#.into(), Shape::Binding, "password"),
        // A quote in a character literal opens no string.
        (
            r#"let q = '"'; let e = '\''; let password = "x";"#.into(),
            Shape::Binding,
            "password",
        ),
        (
            r#"Login { password: "x" }"#.into(),
            Shape::Field,
            "password",
        ),
        (key("password", r#""x""#), Shape::JsonKey, "password"),
        (
            key("new_password", r#""x""#),
            Shape::JsonKey,
            "new_password",
        ),
        (
            key("current_password", r##"r#"x"#"##),
            Shape::JsonKey,
            "current_password",
        ),
        (
            format!("let body = {:?};", key("password", r#""x""#)),
            Shape::EmbeddedJson,
            "password",
        ),
        (
            r#"secrets::hash_password("x")"#.into(),
            Shape::Call,
            "hash_password",
        ),
        (
            r#"secrets::verify_password("x", &phc)"#.into(),
            Shape::Call,
            "verify_password",
        ),
        (
            r#"fx.login("avery.admin", "x")"#.into(),
            Shape::Call,
            "login",
        ),
        (
            r#"support::sign_in(app, "avery.admin", &"x".to_owned())"#.into(),
            Shape::Call,
            "sign_in",
        ),
        (
            r#"users::use_reset_code(&pool, "a", &code, "x")"#.into(),
            Shape::Call,
            "use_reset_code",
        ),
        (
            r#"setup::initialize(&p, &c, "A", "a", "A", r"x",)"#.into(),
            Shape::Call,
            "initialize",
        ),
    ]
}

/// Each forbidden shape is found, naming the file and line.
#[test]
fn each_forbidden_shape_is_found_with_its_file_and_line() {
    for (body, shape, subject) in forbidden_cases() {
        let src = format!("// line 1\n\n{body}\n");
        let found = scan_source("synthetic.rs", &src);
        let expected = Finding {
            path: "synthetic.rs".into(),
            line: 3,
            shape,
            subject: subject.into(),
        };
        assert!(
            found.contains(&expected),
            "{body}\nexpected {expected}\nfound:\n{}",
            listing(&found)
        );
    }
}

/// The shapes the sweep uses, the allowlisted fixture, and look-alikes
/// that carry no credential all scan clean.
#[test]
fn allowed_shapes_yield_no_finding() {
    let clean = [
        "const PASSWORD_MIN: usize = 12;",
        "let password = support::password();",
        "static PASSWORD: OnceLock<String> = OnceLock::new();",
        "let wrong = support::wrong_password();",
        "let fresh = support::fresh_password();",
        "fn login(&self, username: &str, password: &str) -> String { todo!() }",
        "fx.login(\"avery.admin\", support::password()).await;",
        "fx.login(\"casey.api\").await;",
        "live_login(&addr, \"avery.admin\").await;",
        "secrets::hash_password(support::password()).expect(\"hash\");",
        "let secret = text.into_iter().collect();",
        "sqlx::query(\"SELECT password_hash FROM user\");",
        "(\"user.password_hash\", \"credential\")",
        "format!(r#\"{{\"username\":\"{u}\",\"password\":\"{}\"}}\"#, support::password())",
        "// const PASSWORD: &str = \"in a comment\";",
        "/* let password = \"in a block comment\"; */",
        "let c = '\"'; let password = support::password();",
    ];
    for src in clean {
        let found = scan_source("synthetic.rs", src);
        assert!(found.is_empty(), "{src}\nfound:\n{}", listing(&found));
    }
    let key = |value: &str| format!("json!({{ \"pass{}\": {value} }})", "word");
    let found = scan_source("synthetic.rs", &key("support::password()"));
    assert!(found.is_empty(), "{}", listing(&found));
}

/// The allowlist is by exact path, shape, and subject: the fixture is
/// covered where it lives and nowhere else, and nothing else in that file
/// is.
#[test]
fn the_allowlist_covers_only_the_named_fixture() {
    let fixture = r#"const PHC_PASSWORD: &'static str = "x";"#;
    assert_eq!(
        not_allowlisted(scan_source("upgrade_compat.rs", fixture)),
        []
    );
    assert_eq!(
        not_allowlisted(scan_source("other.rs", fixture)).len(),
        1,
        "another file"
    );
    assert_eq!(
        not_allowlisted(scan_source("nested/upgrade_compat.rs", fixture)).len(),
        1,
        "another path"
    );
    let second = r#"const OTHER_PASSWORD: &'static str = "x";"#;
    assert_eq!(
        not_allowlisted(scan_source("upgrade_compat.rs", second)).len(),
        1,
        "another subject"
    );
}

/// The token boundaries the rules rely on: a literal spanning lines
/// reports its first line, and a quote inside a raw string or comment does
/// not open a literal.
#[test]
fn literals_are_located_by_their_first_line() {
    let src =
        "let a = \"one\ntwo\";\n/* \" */\nlet b = r#\"\"\"#;\nconst PASSWORD: &str =\n    \"x\";";
    let found = scan_source("synthetic.rs", src);
    assert_eq!(
        found,
        vec![Finding {
            path: "synthetic.rs".into(),
            line: 6,
            shape: Shape::Binding,
            subject: "PASSWORD".into(),
        }]
    );
}
