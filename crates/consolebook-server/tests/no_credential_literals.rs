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
//! recursively, including `support/` and this file. Symbolic links are not
//! followed (a linked file or directory is skipped): the tree's own files
//! are what the gate proves, and a link could loop or leave the tree. A
//! source that is not UTF-8 fails the scan, naming the file.
//!
//! **Literal.** A string literal token: `"…"`, raw `r"…"` / `r#"…"#` (any
//! number of `#`), and the byte and C forms `b"…"`, `br#"…"#`, `c"…"`,
//! `cr#"…"#`. Text inside comments is not code and is skipped; character
//! literals are not string literals. The empty literal `""` is never a
//! credential and is never flagged. A *literal expression* is a non-empty
//! literal as the first token of an expression, after any leading `&`, so
//! a method chain on it (`"…".to_owned()`) counts; `String::from(literal)`
//! counts too.
//!
//! **Sensitive name.** An identifier or key that contains `password`,
//! `passphrase`, or `secret`, ignoring case (`PASSWORD`, `new_password`,
//! `PHC_PASSWORD`, `current_password`, `client_secret`, …).
//!
//! **Flagged shapes.**
//! 1. *Binding*: `const`, `static`, or `let` (optionally `mut`) of a
//!    sensitive name whose initializer, up to its `;`, holds any non-empty
//!    literal: arrays, `vec![…]`, `Some(…)`, parentheses, `match` and `if`
//!    arms, closures, `Into::into(…)`, `format!`, and `concat!` included.
//! 2. *Field*: `sensitive_name: literal expression` (a struct field).
//! 3. *JSON key*: a sensitive key literal followed by `:` and a literal
//!    expression — the `json!` shape, matched wherever it appears.
//! 4. *Key pair*: a key literal right after `(` that ends in a sensitive
//!    word, followed by `,` and a literal expression — form tuples
//!    `("password", "…")` and `map.insert("password", "…")`. Pairs are
//!    common data, so the key must open the pair and end in the word:
//!    `("user.password_hash", "credential")` names a column, and a list
//!    of words `["password", "secret"]` is not a pair.
//! 5. *Embedded JSON*: a literal whose text holds a sensitive JSON key
//!    with a non-empty string value (`"password":"…"`, escaped once or
//!    raw). A value that begins with `{` is a format interpolation; an
//!    empty value, or one the literal ends before, is an absence check.
//! 6. *Call*: a literal expression passed to a function, method, or macro
//!    (`name(…)`, `name!(…)`, `name::<T>(…)`)
//!    - as any argument when the callee's name is sensitive
//!      (`hash_password`, `verify_password`, `check_password_policy`);
//!    - as the last of two or more arguments to `initialize` or
//!      `use_reset_code`, whose last parameter is the password;
//!    - as the last argument when the callee's name contains `login`,
//!      `log_in`, `sign_in`, `signin`, or `authenticate` (ignoring case)
//!      and another argument is a literal too. A sign-in helper's last
//!      parameter is its password when it takes one and its username when
//!      it does not (`live_login(&addr, "avery.admin")`), so a lone
//!      literal is a username; a username and a password both written in
//!      are two.
//!
//! **Fails closed.** These rules read names, not meaning, so some
//! non-credentials are flagged: `let password_error = "…"`, `let
//! secret_kind = "…"`, a `PASSWORD_HASH_COLUMN` constant, a
//! placeholder such as `"password": "<redacted>"`, `initialize("agency",
//! "tz")`, and `get_login_page(…, "/login", "text/html")`. Rename the
//! binding, field, or helper so it does not read as a credential. Do not
//! split or assemble strings to slip past the scan.
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
//! **Not caught, by design.** Each item has a case in
//! `no_credential_literals/cases.txt` that must stay unflagged, and every
//! `missed` case there names an item here, so this list and the code
//! cannot drift apart. Review covers these.
//! - *non-sensitive names*: `let pwd = "…"`, `const ADMIN_PW`, `let creds =
//!   ("a", "…")`; the scan reads names, and these do not say what they hold.
//! - *destructuring*: `let (u, password) = (…)`; the binding rule reads a
//!   single name, and a pattern can bind it anywhere.
//! - *reassignment*: `password = "…"` after `let mut password`; an
//!   assignment has no keyword to anchor on, and `x = y` is everywhere.
//! - *Some in a field*: `Login { password: Some("…") }`; a field's value
//!   is read as one literal expression, not searched.
//! - *index assignment*: `body["password"] = "…"`; the key is not followed
//!   by `:` or `,`, and indexing is mostly reads.
//! - *double-escaped JSON*: `"{\\\"password\\\":…}"`; JSON escaped inside
//!   JSON text is read once, as written in the source.
//! - *brace-led JSON value*: `"password":"{…}"` in a literal reads as a
//!   format interpolation.
//! - *non-English or fullwidth names*: `contraseña`, `ｐａｓｓｗｏｒｄ`; the
//!   sensitive words are the English ASCII ones the tests use.
//! - *values built by a macro*: `"password": format!(…)`, `login(app, "a",
//!   concat!(…))`; an argument or value is read as one literal expression.
//! - *included file contents*: `hash_password(include_str!("…"))`; the
//!   included file is not Rust source, so it is not scanned.
//! - *username variable beside a literal password*: `login(app, username,
//!   "…")`; one literal in a sign-in call is taken as the username.
//! - *constructors and other helpers*: `LoginRequest::new("a", "…")`,
//!   `SignIn::new(…)`; only the callee's own name is read.
//! - *non-.rs files*: `tests/fixtures/` and this guard's case corpus are
//!   data, not code (tree-level; checked by
//!   `the_walk_skips_links_and_data_and_names_bad_files`).
//! - *symbolic links*: see **Scope** (tree-level; checked by the same test).
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
const SIGN_IN_FRAGMENTS: [&str; 5] = ["login", "log_in", "sign_in", "signin", "authenticate"];

/// The case corpus: synthetic sources and what the scan must find in each.
const CASES: &str = include_str!("no_credential_literals/cases.txt");
/// This file, for the header's list of what is not caught.
const THIS_FILE: &str = include_str!("no_credential_literals.rs");
/// Not-caught items proved on a tree rather than by a corpus case.
const TREE_LEVEL_MISSES: [&str; 2] = ["non-.rs files", "symbolic links"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Binding,
    Field,
    JsonKey,
    KeyPair,
    EmbeddedJson,
    Call,
}

impl Shape {
    fn parse(name: &str) -> Self {
        match name {
            "Binding" => Self::Binding,
            "Field" => Self::Field,
            "JsonKey" => Self::JsonKey,
            "KeyPair" => Self::KeyPair,
            "EmbeddedJson" => Self::EmbeddedJson,
            "Call" => Self::Call,
            other => panic!("unknown shape {other} in cases.txt"),
        }
    }
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

fn ends_sensitive(key: &str) -> bool {
    let lower = key.to_lowercase();
    SENSITIVE.iter().any(|word| lower.ends_with(word))
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

/// The line of a non-empty literal token at `at`.
fn non_empty_literal(tokens: &[Token], at: usize) -> Option<usize> {
    match tokens.get(at) {
        Some(Token {
            tok: Tok::Str(text),
            line,
        }) if !text.is_empty() => Some(*line),
        _ => None,
    }
}

/// The line of the literal expression starting at `at`, if one does.
fn literal_expression(tokens: &[Token], mut at: usize) -> Option<usize> {
    while punct(tokens, at, '&') {
        at += 1;
    }
    let from = ident(tokens, at) == Some("String")
        && punct(tokens, at + 1, ':')
        && punct(tokens, at + 2, ':')
        && ident(tokens, at + 3) == Some("from")
        && punct(tokens, at + 4, '(');
    if from {
        return literal_expression(tokens, at + 5);
    }
    non_empty_literal(tokens, at)
}

/// The line of the first non-empty literal in the initializer that starts
/// at `at` and ends at its `;` (or where its enclosing block closes).
fn literal_in_initializer(tokens: &[Token], mut at: usize) -> Option<usize> {
    let mut depth = 0usize;
    while let Some(token) = tokens.get(at) {
        match token.tok {
            Tok::Punct('(' | '[' | '{') => depth += 1,
            Tok::Punct(')' | ']' | '}') => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            Tok::Punct(';') if depth == 0 => return None,
            _ => {
                if let Some(line) = non_empty_literal(tokens, at) {
                    return Some(line);
                }
            }
        }
        at += 1;
    }
    None
}

/// The `(` (or `[`) that opens the call of the callee at `at`: `name(`,
/// `name!(`, `name![`, or `name::<…>(`.
fn call_open(tokens: &[Token], at: usize) -> Option<usize> {
    if punct(tokens, at + 1, '(') {
        return Some(at + 1);
    }
    if punct(tokens, at + 1, '!') && (punct(tokens, at + 2, '(') || punct(tokens, at + 2, '[')) {
        return Some(at + 2);
    }
    if punct(tokens, at + 1, ':') && punct(tokens, at + 2, ':') && punct(tokens, at + 3, '<') {
        let mut depth = 0usize;
        let mut close = at + 3;
        while close < tokens.len() {
            if punct(tokens, close, '<') {
                depth += 1;
            } else if punct(tokens, close, '>') {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            close += 1;
        }
        return punct(tokens, close + 1, '(').then_some(close + 1);
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
    starts.retain(|start| {
        !(punct(tokens, *start, ')') || punct(tokens, *start, ']') || *start >= tokens.len())
    });
    starts
}

/// The arguments of the call at `open` that the call rule reads.
fn checked_arguments(tokens: &[Token], callee: &str, open: usize) -> Vec<usize> {
    let args = arguments(tokens, open);
    if sensitive(callee) {
        return args;
    }
    let lower = callee.to_lowercase();
    let literals = args
        .iter()
        .filter(|start| literal_expression(tokens, **start).is_some())
        .count();
    let last_is_password = args.len() >= 2
        && (LAST_ARGUMENT_CALLEES.contains(&callee)
            || (SIGN_IN_FRAGMENTS.iter().any(|f| lower.contains(f)) && literals >= 2));
    if last_is_password {
        args[args.len() - 1..].to_vec()
    } else {
        Vec::new()
    }
}

/// The text of a literal holds a sensitive JSON key with a non-empty
/// string value that is not a format interpolation. Every quote is tried
/// as a key's opening quote, so a quote earlier in the text cannot pair
/// with the wrong one and hide a key.
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
            && !(value.is_empty() || value.starts_with(['{', '"']))
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
                    && let Some(line) = literal_in_initializer(&tokens, eq + 1)
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
            Tok::Ident(callee) => {
                if at > 0 && ident(&tokens, at - 1) == Some("fn") {
                    continue;
                }
                let Some(open) = call_open(&tokens, at) else {
                    continue;
                };
                for start in checked_arguments(&tokens, callee, open) {
                    if let Some(line) = literal_expression(&tokens, start) {
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
                if ends_sensitive(text)
                    && at > 0
                    && punct(&tokens, at - 1, '(')
                    && punct(&tokens, at + 1, ',')
                    && let Some(line) = literal_expression(&tokens, at + 2)
                {
                    flag(line, Shape::KeyPair, text);
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

// ------------------------------------------------------------------ walk

/// Every `.rs` file under `dir`, recursively, never following a link.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("read {}: {err}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let kind = std::fs::symlink_metadata(&path)
            .unwrap_or_else(|err| panic!("stat {}: {err}", path.display()))
            .file_type();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
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
        let src = std::fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read test source {label}: {err}"));
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

// ----------------------------------------------------------------- cases

/// What a corpus case expects.
enum Expect {
    /// Exactly these findings (none: the source scans clean).
    Findings(Vec<(Shape, String, usize)>),
    /// No finding, and the header's not-caught list names this item.
    Missed(String),
    /// Malformed source: the scan only has to finish.
    Robust,
}

struct Case {
    id: String,
    expect: Expect,
    source: String,
}

/// `=== <id> findings [Shape:subject:line,…]`, `=== <id> missed <item>`,
/// or `=== <id> robust`, each followed by its source. Lines before the
/// first case are the corpus's own notes.
fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for line in CASES.split_inclusive('\n') {
        let Some(spec) = line.strip_prefix("=== ") else {
            if let Some(case) = cases.last_mut() {
                case.source.push_str(line);
            }
            continue;
        };
        let spec = spec.trim_end();
        let (id, rest) = spec.split_once(' ').expect("case id and kind");
        let (kind, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        let expect = match kind {
            "findings" => Expect::Findings(
                arg.split(',')
                    .filter(|item| !item.is_empty())
                    .map(|item| {
                        let mut parts = item.split(':');
                        let shape = Shape::parse(parts.next().expect("shape"));
                        let subject = parts.next().expect("subject").to_owned();
                        let line = parts.next().expect("line").parse().expect("line number");
                        (shape, subject, line)
                    })
                    .collect(),
            ),
            "missed" => Expect::Missed(arg.to_owned()),
            "robust" => Expect::Robust,
            other => panic!("case {id}: unknown kind {other}"),
        };
        cases.push(Case {
            id: id.to_owned(),
            expect,
            source: String::new(),
        });
    }
    cases
}

/// The items of the header's **Not caught** list: `//! - *item*: …`.
fn not_caught_items() -> Vec<String> {
    let section = THIS_FILE
        .split("**Not caught, by design.**")
        .nth(1)
        .expect("not-caught section")
        .split("\n\n")
        .next()
        .expect("section text");
    section
        .lines()
        .filter_map(|line| line.strip_prefix("//! - *"))
        .map(|item| item.split_once("*:").expect("item label").0.to_owned())
        .collect()
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

/// Every case in the corpus scans as it says: forbidden shapes are found
/// at their file and line, allowed shapes scan clean, malformed sources
/// finish, and each documented miss stays unflagged.
#[test]
fn the_case_corpus_scans_as_written() {
    let cases = cases();
    assert!(cases.len() > 100, "{} cases", cases.len());
    let documented = not_caught_items();
    for case in &cases {
        let path = format!("cases.txt#{}", case.id);
        let found = scan_source(&path, &case.source);
        match &case.expect {
            Expect::Findings(expected) => {
                let expected: Vec<Finding> = expected
                    .iter()
                    .map(|(shape, subject, line)| Finding {
                        path: path.clone(),
                        line: *line,
                        shape: *shape,
                        subject: subject.clone(),
                    })
                    .collect();
                assert_eq!(found, expected, "case {}", case.id);
            }
            Expect::Missed(item) => {
                assert!(found.is_empty(), "case {}:\n{}", case.id, listing(&found));
                assert!(
                    documented.contains(item),
                    "case {}: `{item}` is not in the header's not-caught list",
                    case.id
                );
            }
            Expect::Robust => {}
        }
    }
    // Every not-caught item has a case, or is proved on a tree.
    for item in &documented {
        let has_case = cases
            .iter()
            .any(|case| matches!(&case.expect, Expect::Missed(m) if m == item));
        assert!(
            has_case || TREE_LEVEL_MISSES.contains(&item.as_str()),
            "not-caught item `{item}` has no `missed` case"
        );
    }
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

/// The walk scans `.rs` files in nested directories, skips data files and
/// symbolic links, and names a source it cannot read.
#[cfg(unix)]
#[test]
fn the_walk_skips_links_and_data_and_names_bad_files() {
    let forbidden = r#"const PASSWORD: &'static str = "x";"#;
    let tree = tempfile::tempdir().expect("temp dir");
    let outside = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir(tree.path().join("nested")).expect("mkdir");
    std::fs::write(tree.path().join("nested/real.rs"), forbidden).expect("write");
    std::fs::write(tree.path().join("data.txt"), forbidden).expect("write");
    std::fs::write(outside.path().join("linked.rs"), forbidden).expect("write");
    std::os::unix::fs::symlink(outside.path(), tree.path().join("linked_dir")).expect("link");
    std::os::unix::fs::symlink(
        outside.path().join("linked.rs"),
        tree.path().join("linked_file.rs"),
    )
    .expect("link");
    let (findings, scanned) = scan_tree(tree.path());
    assert_eq!(scanned, ["nested/real.rs"]);
    assert_eq!(listing(&findings), "nested/real.rs:1: Binding `PASSWORD`");

    std::fs::write(tree.path().join("broken.rs"), [0xff, 0xfe, b'\n']).expect("write");
    let panic = std::panic::catch_unwind(|| scan_tree(tree.path())).expect_err("non-UTF-8 fails");
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("broken.rs"), "{message}");
}
