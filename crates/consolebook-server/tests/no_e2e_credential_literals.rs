//! Guard: no browser e2e test declares its own credential literal (#143).
//!
//! Test credentials for `web/e2e/` come from `web/e2e/fixtures.ts`, which
//! generates them at run time, mirroring `tests/support/mod.rs` and
//! `tests/no_credential_literals.rs` (#126). A credential written into a
//! spec file as a literal is the same class of hard-coded credential that
//! static analysis flags, and every new spec copied from an older one
//! carries the pattern forward. This test scans `web/e2e/` and fails on
//! that pattern's return.
//!
//! # Scan contract
//!
//! **Scope.** Every `.ts` file under `web/e2e/`, recursively, including
//! `fixtures.ts`. Symbolic links are not followed (a linked file or
//! directory is skipped): the tree's own files are what the gate proves,
//! and a link could loop or leave the tree. A source that is not UTF-8
//! fails the scan, naming the file.
//!
//! **Literal.** A non-empty string literal token: `'…'`, `"…"`, or a
//! static template literal (backtick-quoted with no `${…}` interpolation).
//! Text inside comments (`//` and `/* … */`) and regular-expression
//! literals is not a string literal and is skipped. The empty literal
//! (`''`, `""`, or an empty template literal) is never a credential and is
//! never flagged. A *literal expression* is a non-empty literal as the
//! first token of an expression (so `'…'.trim()` counts), or
//! `String(literal)`.
//!
//! **Sensitive name.** An identifier, property key, or form label that
//! contains `password`, `passphrase`, or `secret`, ignoring case
//! (`PASSWORD`, `ADMIN_PASSWORD`, `newPassword`, `client_secret`,
//! `'Administrator password'`, …).
//!
//! **Flagged shapes.**
//! 1. *Binding*: `const`, `let`, or `var` of a sensitive name whose
//!    initializer, up to its `;` (or the next top-level statement), holds
//!    any non-empty literal.
//! 2. *Field*: `sensitive_name: literal expression` or
//!    `'sensitive_name': literal expression` (an object property or JSON
//!    key).
//! 3. *Embedded JSON*: a literal whose text holds a sensitive JSON key
//!    with a non-empty string value (`"password":"…"`, escaped once or
//!    raw). A value that begins with `{` is an interpolation; an empty
//!    value, or one the literal ends before, is an absence check.
//! 4. *Fill*: `.getByLabel(<sensitive literal>, …).fill(<literal expression>)`
//!    or `.getByPlaceholder(<sensitive literal>, …).fill(<literal expression>)`
//!    (Playwright filling a credential field directly with a literal).
//! 5. *Call*: a literal expression passed to a function or method call
//!    (`name(…)` or `name<T>(…)`)
//!    - as any argument when the callee's name is sensitive
//!      (`setPassword`, `hashPassword`, `verifySecret`);
//!    - as the last of two or more arguments to `resetAndLogin`,
//!      `initialize`, or `useResetCode`, whose last parameter is the
//!      password;
//!    - as the last argument when the callee's name contains `login`,
//!      `log_in`, `sign_in`, `signin`, or `authenticate` (ignoring case)
//!      and another argument is a literal too.
//!
//! **Fails closed.** These rules read names, not meaning, so some
//! non-credentials are flagged: `const passwordError = '…'`, a placeholder
//! such as `password: '<redacted>'`, or `getLoginPage(url, '/login',
//! 'text/html')`. Rename the binding, field, or helper so it does not read
//! as a credential. Do not split or assemble strings to slip past the scan.
//!
//! **Not caught, by design.** Each item has a case in
//! `no_e2e_credential_literals/cases.txt` that must stay unflagged, and
//! every `missed` case there names an item here, so this list and the code
//! cannot drift apart. Review covers these.
//! - *non-sensitive names*: `const pwd = '…'`, `const ADMIN_PW = '…'`; the
//!   scan reads names, and these do not say what they hold.
//! - *destructuring*: `const [u, password] = ['a', '…']`; the binding rule
//!   reads a single name, and a pattern can bind it anywhere.
//! - *reassignment*: `password = '…'` after `let password: string`; an
//!   assignment has no keyword to anchor on, and `x = y` is everywhere.
//! - *index assignment*: `body['password'] = '…'`; the key is not followed
//!   by `:` and indexing is mostly reads.
//! - *double-escaped JSON*: `'{\\\"password\\\":\\\"x\\\"}'`; JSON escaped
//!   inside JSON text is read once, as written in the source.
//! - *brace-led JSON value*: `'{"password":"{pw}"}'` reads as an
//!   interpolation.
//! - *interpolated template expression*: a backtick template containing
//!   `${…}` (such as `password: …${prefix}123…`); a template with `${…}` is
//!   not read as a single static literal.
//! - *username variable beside a literal password*: `signIn(ctx, username,
//!   '…')`; one literal in a sign-in call is taken as the username.
//! - *non-.ts files*: fixture data and this guard's case corpus are data,
//!   not TypeScript source (tree-level; checked by
//!   `the_walk_skips_links_and_data_and_names_bad_files`).
//! - *symbolic links*: see **Scope** (tree-level; checked by the same test).
//!
//! Findings name the file, line, shape, and subject (the name, key, label,
//! or callee), never the literal's text.

use std::fmt;
use std::path::{Path, PathBuf};

const SENSITIVE: [&str; 3] = ["password", "passphrase", "secret"];
const LAST_ARGUMENT_CALLEES: [&str; 3] = ["resetAndLogin", "initialize", "useResetCode"];
const SIGN_IN_FRAGMENTS: [&str; 5] = ["login", "log_in", "sign_in", "signin", "authenticate"];
const STATEMENT_STARTERS: [&str; 12] = [
    "const", "let", "var", "function", "async", "export", "import", "test", "await", "return",
    "if", "for",
];
const CASES: &str = include_str!("no_e2e_credential_literals/cases.txt");
const THIS_FILE: &str = include_str!("no_e2e_credential_literals.rs");
const TREE_LEVEL_MISSES: [&str; 2] = ["non-.ts files", "symbolic links"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Binding,
    Field,
    EmbeddedJson,
    Fill,
    Call,
}

impl Shape {
    fn parse(name: &str) -> Self {
        match name {
            "Binding" => Self::Binding,
            "Field" => Self::Field,
            "EmbeddedJson" => Self::EmbeddedJson,
            "Fill" => Self::Fill,
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
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn regex_can_follow(prev: Option<&Tok>) -> bool {
    match prev {
        None => true,
        Some(Tok::Punct(c)) => !matches!(c, ')' | ']'),
        Some(Tok::Ident(word)) => matches!(
            word.as_str(),
            "return" | "case" | "throw" | "in" | "of" | "typeof" | "void" | "delete" | "await"
        ),
        Some(Tok::Str(_) | Tok::Other) => false,
    }
}

fn tokenize(src: &str) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let mut template_brace_depths: Vec<usize> = Vec::new();
    let mut brace_depth = 0usize;
    let (mut at, mut line) = (0, 1);

    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        let (tok, end) = if c.is_whitespace() {
            (None, at + 1)
        } else if c == '/' && next == Some('/') {
            let end = chars[at..].iter().position(|ch| *ch == '\n');
            (None, end.map_or(chars.len(), |n| at + n))
        } else if c == '/' && next == Some('*') {
            (None, block_comment_end(&chars, at))
        } else if c == '/' && regex_can_follow(tokens.last().map(|t: &Token| &t.tok)) {
            (Some(Tok::Other), regex_end(&chars, at))
        } else if c == '\'' || c == '"' {
            let (text, end) = quoted_string(&chars, at, c);
            (Some(Tok::Str(text)), end)
        } else if c == '`' {
            let (tok, end, opened_expr) = template_part(&chars, at + 1, true);
            if opened_expr {
                template_brace_depths.push(brace_depth);
                brace_depth += 1;
            }
            (Some(tok), end)
        } else if c == '}' && template_brace_depths.last() == Some(&(brace_depth.saturating_sub(1)))
        {
            brace_depth -= 1;
            template_brace_depths.pop();
            let (tok, end, opened_expr) = template_part(&chars, at + 1, false);
            if opened_expr {
                template_brace_depths.push(brace_depth);
                brace_depth += 1;
            }
            (Some(tok), end)
        } else if is_ident_start(c) {
            let mut end = at + 1;
            while end < chars.len() && is_ident_char(chars[end]) {
                end += 1;
            }
            (Some(Tok::Ident(chars[at..end].iter().collect())), end)
        } else if c.is_ascii_digit() {
            let mut end = at + 1;
            while end < chars.len() && (is_ident_char(chars[end]) || chars[end] == '.') {
                end += 1;
            }
            (Some(Tok::Other), end)
        } else {
            if c == '{' {
                brace_depth += 1;
            } else if c == '}' {
                brace_depth = brace_depth.saturating_sub(1);
            }
            (Some(Tok::Punct(c)), at + 1)
        };
        if let Some(tok) = tok {
            tokens.push(Token { tok, line });
        }
        line += chars[at..end].iter().filter(|ch| **ch == '\n').count();
        at = end;
    }
    tokens
}

fn block_comment_end(chars: &[char], at: usize) -> usize {
    let mut i = at + 2;
    while i < chars.len() {
        if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
            return i + 2;
        }
        i += 1;
    }
    chars.len()
}

fn regex_end(chars: &[char], open: usize) -> usize {
    let mut i = open + 1;
    let mut in_class = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            i = (i + 2).min(chars.len());
            continue;
        }
        if c == '\n' {
            return i;
        }
        if c == '[' {
            in_class = true;
        } else if c == ']' {
            in_class = false;
        } else if c == '/' && !in_class {
            i += 1;
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            return i;
        }
        i += 1;
    }
    chars.len()
}

fn quoted_string(chars: &[char], open: usize, quote: char) -> (String, usize) {
    let mut i = open + 1;
    while i < chars.len() && chars[i] != quote {
        i += if chars[i] == '\\' { 2 } else { 1 };
    }
    let end = i.min(chars.len());
    (
        chars[open + 1..end].iter().collect(),
        (i + 1).min(chars.len()),
    )
}

/// Lexes from just after an opening backtick (when `is_head` is true) or a
/// closing `}` of `${…}` until the next `${` or closing backtick. A static
/// template literal (`is_head` and closed by a backtick) becomes `Tok::Str`;
/// any interpolated template segment becomes `Tok::Other`.
fn template_part(chars: &[char], start: usize, is_head: bool) -> (Tok, usize, bool) {
    let mut i = start;
    while i < chars.len() {
        if chars[i] == '\\' {
            i = (i + 2).min(chars.len());
            continue;
        }
        if chars[i] == '`' {
            let tok = if is_head {
                Tok::Str(chars[start..i].iter().collect())
            } else {
                Tok::Other
            };
            return (tok, i + 1, false);
        }
        if chars[i] == '$' && chars.get(i + 1) == Some(&'{') {
            return (Tok::Other, i + 2, true);
        }
        i += 1;
    }
    let tok = if is_head {
        Tok::Str(chars[start..].iter().collect())
    } else {
        Tok::Other
    };
    (tok, chars.len(), false)
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

fn non_empty_literal(tokens: &[Token], at: usize) -> Option<usize> {
    match tokens.get(at) {
        Some(Token {
            tok: Tok::Str(text),
            line,
        }) if !text.is_empty() => Some(*line),
        _ => None,
    }
}

fn literal_expression(tokens: &[Token], at: usize) -> Option<usize> {
    if ident(tokens, at) == Some("String") && punct(tokens, at + 1, '(') {
        return literal_expression(tokens, at + 2);
    }
    non_empty_literal(tokens, at)
}

fn literal_in_initializer(tokens: &[Token], mut at: usize) -> Option<usize> {
    let mut depth = 0usize;
    let start_line = tokens.get(at)?.line;
    let mut prev_line = start_line;
    while let Some(token) = tokens.get(at) {
        if depth == 0
            && token.line > prev_line
            && let Tok::Ident(word) = &token.tok
            && STATEMENT_STARTERS.contains(&word.as_str())
        {
            return None;
        }
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
        prev_line = token.line;
        at += 1;
    }
    None
}

fn call_open(tokens: &[Token], at: usize) -> Option<usize> {
    if punct(tokens, at + 1, '(') {
        return Some(at + 1);
    }
    if punct(tokens, at + 1, '<') {
        let mut depth = 0usize;
        let mut close = at + 1;
        while close < tokens.len() {
            if punct(tokens, close, '<') {
                depth += 1;
            } else if punct(tokens, close, '>') {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            } else if punct(tokens, close, ';') || punct(tokens, close, '{') {
                return None;
            }
            close += 1;
        }
        return punct(tokens, close + 1, '(').then_some(close + 1);
    }
    None
}

fn matching_close(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut at = open;
    while let Some(token) = tokens.get(at) {
        match token.tok {
            Tok::Punct('(' | '[' | '{') => depth += 1,
            Tok::Punct(')' | ']' | '}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

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
    starts.retain(|start| !(punct(tokens, *start, ')') || *start >= tokens.len()));
    starts
}

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
            Tok::Ident(keyword) if ["const", "let", "var"].contains(&keyword.as_str()) => {
                let name_at = at + 1;
                let Some(name) = ident(&tokens, name_at).filter(|name| sensitive(name)) else {
                    continue;
                };
                let mut eq = name_at + 1;
                let mut type_depth = 0usize;
                while eq < tokens.len() {
                    if punct(&tokens, eq, '(') || punct(&tokens, eq, '[') || punct(&tokens, eq, '{')
                    {
                        type_depth += 1;
                    } else if punct(&tokens, eq, ')')
                        || punct(&tokens, eq, ']')
                        || punct(&tokens, eq, '}')
                    {
                        type_depth = type_depth.saturating_sub(1);
                    } else if type_depth == 0
                        && (punct(&tokens, eq, '=') || punct(&tokens, eq, ';'))
                    {
                        break;
                    }
                    eq += 1;
                }
                if punct(&tokens, eq, '=')
                    && !punct(&tokens, eq + 1, '=')
                    && let Some(line) = literal_in_initializer(&tokens, eq + 1)
                {
                    flag(line, Shape::Binding, name);
                }
            }
            Tok::Ident(name) if sensitive(name) && punct(&tokens, at + 1, ':') => {
                if let Some(line) = literal_expression(&tokens, at + 2) {
                    flag(line, Shape::Field, name);
                }
            }
            Tok::Ident(callee) => {
                if at > 0 && ident(&tokens, at - 1) == Some("function") {
                    continue;
                }
                let Some(open) = call_open(&tokens, at) else {
                    continue;
                };
                if matches!(callee.as_str(), "getByLabel" | "getByPlaceholder")
                    && let Some(first_arg) = arguments(&tokens, open).first().copied()
                    && let Some(Token {
                        tok: Tok::Str(label),
                        ..
                    }) = tokens.get(first_arg)
                    && sensitive(label)
                    && let Some(close) = matching_close(&tokens, open)
                    && punct(&tokens, close + 1, '.')
                    && ident(&tokens, close + 2) == Some("fill")
                    && let Some(fill_open) = call_open(&tokens, close + 2)
                    && let Some(fill_arg) = arguments(&tokens, fill_open).first().copied()
                    && let Some(line) = literal_expression(&tokens, fill_arg)
                {
                    flag(line, Shape::Fill, label);
                }
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
                    flag(line, Shape::Field, text);
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

fn ts_files(dir: &Path, out: &mut Vec<PathBuf>) {
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
            ts_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "ts") {
            out.push(path);
        }
    }
}

fn scan_tree(root: &Path) -> (Vec<Finding>, Vec<String>) {
    let mut files = Vec::new();
    ts_files(root, &mut files);
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
            .unwrap_or_else(|err| panic!("read e2e source {label}: {err}"));
        findings.extend(scan_source(&label, &src));
        scanned.push(label);
    }
    (findings, scanned)
}

fn listing(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

// ----------------------------------------------------------------- cases

enum Expect {
    Findings(Vec<(Shape, String, usize)>),
    Missed(String),
    Robust,
}

struct Case {
    id: String,
    expect: Expect,
    source: String,
}

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
fn no_e2e_spec_declares_its_own_credential_literal() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/e2e");
    let (findings, scanned) = scan_tree(&root);
    for expected in [
        "fixtures.ts",
        "drafts.spec.ts",
        "draft-recovery.spec.ts",
        "shell.spec.ts",
        "retention.spec.ts",
        "public-url.spec.ts",
    ] {
        assert!(scanned.iter().any(|s| s == expected), "scanned {expected}");
    }
    assert!(
        findings.is_empty(),
        "e2e credentials come from `web/e2e/fixtures.ts` (`testPassword()`, \
         `wrongTestPassword()`, `freshTestPassword()`); these declare their own:\n{}",
        listing(&findings)
    );
}

#[test]
fn the_case_corpus_scans_as_written() {
    let cases = cases();
    assert!(cases.len() >= 30, "{} cases", cases.len());
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

#[cfg(unix)]
#[test]
fn the_walk_skips_links_and_data_and_names_bad_files() {
    let forbidden = "const PASSWORD = 'x';";
    let tree = tempfile::tempdir().expect("temp dir");
    let outside = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir(tree.path().join("nested")).expect("mkdir");
    std::fs::write(tree.path().join("nested/real.spec.ts"), forbidden).expect("write");
    std::fs::write(tree.path().join("data.txt"), forbidden).expect("write");
    std::fs::write(outside.path().join("linked.spec.ts"), forbidden).expect("write");
    std::os::unix::fs::symlink(outside.path(), tree.path().join("linked_dir")).expect("link");
    std::os::unix::fs::symlink(
        outside.path().join("linked.spec.ts"),
        tree.path().join("linked_file.spec.ts"),
    )
    .expect("link");
    let (findings, scanned) = scan_tree(tree.path());
    assert_eq!(scanned, ["nested/real.spec.ts"]);
    assert_eq!(
        listing(&findings),
        "nested/real.spec.ts:1: Binding `PASSWORD`"
    );

    std::fs::write(tree.path().join("broken.spec.ts"), [0xff, 0xfe, b'\n']).expect("write");
    let panic = std::panic::catch_unwind(|| scan_tree(tree.path())).expect_err("non-UTF-8 fails");
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("broken.spec.ts"), "{message}");
}
