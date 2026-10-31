//! The gate sequence, as CONTRIBUTING.md writes it.

use super::lex::{quote, runs_e2e};
use super::{BROWSER_PLACEHOLDER, BROWSER_VAR, E2E_MARKER, SECTION, SetupError, setup};

/// The gate steps, and the system-browser line, from CONTRIBUTING.md's text.
pub fn parse(text: &str) -> Result<(Vec<String>, String), SetupError> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| *l == SECTION)
        .ok_or_else(|| setup(format!("CONTRIBUTING.md has no '{SECTION}' heading")))?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with("## "))
        .map_or(lines.len(), |i| start + 1 + i);

    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in &lines[start + 1..end] {
        let stripped = line.trim();
        match current.as_mut() {
            None if stripped == "```sh" => current = Some(Vec::new()),
            None => {}
            Some(_) if stripped == "```" => blocks.extend(current.take()),
            Some(block) => block.push(stripped),
        }
    }
    if current.is_some() {
        return Err(setup(format!(
            "CONTRIBUTING.md: unterminated ```sh block under '{SECTION}'"
        )));
    }
    let Some((gate, rest)) = blocks.split_first() else {
        return Err(setup(format!(
            "CONTRIBUTING.md: no ```sh block under '{SECTION}'"
        )));
    };
    let steps: Vec<String> = gate
        .iter()
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .map(|s| (*s).to_owned())
        .collect();
    if steps.is_empty() {
        return Err(setup("CONTRIBUTING.md: the gate block has no commands"));
    }
    // A receipt that says "passed" must include the browser suite exactly once.
    let e2e = steps.iter().filter(|s| runs_e2e(s)).count();
    if e2e != 1 {
        return Err(setup(format!(
            "CONTRIBUTING.md: expected one gate step running '{E2E_MARKER}', found {e2e}"
        )));
    }
    let assigns = format!("{BROWSER_VAR}=");
    let browser: Vec<&str> = rest
        .iter()
        .flatten()
        .copied()
        .filter(|s| s.contains(&assigns))
        .collect();
    let [browser] = browser[..] else {
        return Err(setup(format!(
            "CONTRIBUTING.md: expected one {BROWSER_VAR}= line, found {}",
            browser.len()
        )));
    };
    if !runs_e2e(browser) {
        return Err(setup(format!(
            "CONTRIBUTING.md: the {BROWSER_VAR}= line must run '{E2E_MARKER}'"
        )));
    }
    Ok((steps, browser.to_owned()))
}

/// The steps with the browser step swapped for CONTRIBUTING.md's
/// system-browser line, pointed at `chromium`.
pub fn with_system_browser(
    steps: &[String],
    browser_line: &str,
    chromium: &str,
) -> Result<Vec<String>, SetupError> {
    if browser_line.matches(BROWSER_PLACEHOLDER).count() != 1 {
        return Err(setup(format!(
            "CONTRIBUTING.md: the {BROWSER_VAR}= line must contain '{BROWSER_PLACEHOLDER}' once"
        )));
    }
    let hits: Vec<usize> = (0..steps.len()).filter(|&i| runs_e2e(&steps[i])).collect();
    let [hit] = hits[..] else {
        return Err(setup(format!(
            "CONTRIBUTING.md: expected one gate step running '{E2E_MARKER}', found {}",
            hits.len()
        )));
    };
    let mut out = steps.to_vec();
    out[hit] = browser_line.replace(BROWSER_PLACEHOLDER, &quote(chromium));
    Ok(out)
}
