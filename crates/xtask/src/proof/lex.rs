//! Reading a gate line as a shell would, enough to tell whether it runs the
//! browser suite so that its failure counts.

use super::E2E_MARKER;

/// What a POSIX shell lexer makes of `line`: its words and operators, quotes
/// and escapes resolved, a `#` comment dropped to the end of its line. Operators are runs of
/// `();<>|&`, each run one token (`&&`, `||`, `)`). None for a line it can't
/// read: an open quote, or a trailing escape.
///
/// It reads the way the runner it replaced did (Python's `shlex` with
/// `posix=True`, `punctuation_chars=True` and `whitespace_split`), so a gate
/// line means the same to it.
#[must_use]
pub fn tokens(line: &str) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Gap,
        Word,
        Operator,
        Single,
        Double,
        /// After a backslash: in a word, or in double quotes.
        Escape {
            in_double: bool,
        },
    }
    const OPERATOR: &str = "();<>|&";
    const SPACE: &str = " \t\r\n";
    let mut out = Vec::new();
    let mut token = String::new();
    // A quoted empty word ('') is still a word.
    let mut quoted = false;
    let mut emit = |token: &mut String, quoted: &mut bool| {
        if !token.is_empty() || *quoted {
            out.push(std::mem::take(token));
        }
        *quoted = false;
    };
    let mut state = State::Gap;
    let mut chars = line.chars();
    // A character an operator ended on, read again as what follows it.
    let mut again = None;
    while let Some(c) = again.take().or_else(|| chars.next()) {
        state = match state {
            // A comment, to the end of its line.
            State::Gap | State::Word | State::Operator if c == '#' => {
                chars.by_ref().find(|&c| c == '\n');
                emit(&mut token, &mut quoted);
                State::Gap
            }
            State::Gap | State::Word | State::Operator if SPACE.contains(c) => {
                emit(&mut token, &mut quoted);
                State::Gap
            }
            State::Operator if OPERATOR.contains(c) => {
                token.push(c);
                State::Operator
            }
            State::Operator => {
                emit(&mut token, &mut quoted);
                again = Some(c);
                State::Gap
            }
            State::Gap | State::Word if OPERATOR.contains(c) => {
                emit(&mut token, &mut quoted);
                token.push(c);
                State::Operator
            }
            State::Gap | State::Word => match c {
                '\'' | '"' => {
                    quoted = true;
                    if c == '"' {
                        State::Double
                    } else {
                        State::Single
                    }
                }
                '\\' => State::Escape { in_double: false },
                _ => {
                    token.push(c);
                    State::Word
                }
            },
            State::Single if c == '\'' => State::Word,
            State::Double if c == '"' => State::Word,
            State::Double if c == '\\' => State::Escape { in_double: true },
            State::Single | State::Double => {
                token.push(c);
                state
            }
            State::Escape { in_double } => {
                // In double quotes only the quote and the escape itself are
                // escaped; any other backslash stays.
                if in_double && c != '"' && c != '\\' {
                    token.push('\\');
                }
                token.push(c);
                if in_double {
                    State::Double
                } else {
                    State::Word
                }
            }
        };
    }
    match state {
        State::Single | State::Double | State::Escape { .. } => None,
        State::Gap | State::Word | State::Operator => {
            emit(&mut token, &mut quoted);
            Some(out)
        }
    }
}

/// Whether a gate line invokes the browser suite so that its failure counts.
///
/// The line is split into commands only at `&&`, the one operator that
/// passes a failure through. Any other operator (`;`, `||`, `|`, `&`, which
/// can each hide a failed suite) stays inside its command, so that command
/// can never equal the marker. One command, after grouping parentheses and
/// leading `NAME=value` assignments, must be exactly `npm run e2e`. A line
/// the lexer can't read is refused, never accepted.
#[must_use]
pub fn runs_e2e(line: &str) -> bool {
    let Some(tokens) = tokens(line) else {
        return false;
    };
    let marker: Vec<&str> = E2E_MARKER.split(' ').collect();
    let mut command: Vec<String> = Vec::new();
    for token in tokens.into_iter().chain(std::iter::once("&&".to_owned())) {
        if token == "(" || token == ")" {
            continue;
        }
        if token != "&&" {
            command.push(token);
            continue;
        }
        let words = command.iter().skip_while(|w| assignment(w));
        if words.map(String::as_str).eq(marker.iter().copied()) {
            return true;
        }
        command.clear();
    }
    false
}

/// `NAME=value`, a shell variable's assignment.
fn assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `word` as one shell word: as it is if nothing in it is special, in single
/// quotes otherwise (Python's `shlex.quote`, as the receipt's commands were
/// written).
#[must_use]
pub fn quote(word: &str) -> String {
    if word.is_empty() {
        return "''".to_owned();
    }
    let plain = word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r#"'"'"'"#))
    }
}

/// Words joined into one shell line.
#[must_use]
pub fn join<S: AsRef<str>>(words: &[S]) -> String {
    words
        .iter()
        .map(|w| quote(w.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}
