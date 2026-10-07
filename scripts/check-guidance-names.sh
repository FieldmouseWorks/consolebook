#!/bin/sh
# Guard: tracked agent guidance must name roles, not specific models,
# model families, or vendor/agent products (issue #78, rung 2 for #76).
#
# Usage: scripts/check-guidance-names.sh   (from anywhere in the repository)
# Exit:  0 clean, 1 residual hits (printed as path:line: text), 2 error.
#
# Depends only on git and a POSIX awk. Deliberately no system grep: its
# alternation behavior differs between implementations.

set -eu

# --- Scanned set -----------------------------------------------------------
# Git pathspecs, one per line (files, directories, or :(glob) magic for
# entrypoints at any depth). To extend: add a line. Every entry must match at
# least one tracked file, or the guard fails closed (exit 2), so a renamed
# file cannot drop out. Root entrypoints are listed literally as well as by
# glob, so removing a root file fails closed even when a nested copy exists;
# overlapping entries scan each file once.
SCANNED='
AGENTS.md
:(glob)**/AGENTS.md
CONTRIBUTING.md
CLAUDE.md
:(glob)**/CLAUDE.md
docs/workflow.md
docs/development.md
.agents
.github/ISSUE_TEMPLATE
.github/PULL_REQUEST_TEMPLATE.md
.github/copilot-instructions.md
'

# --- Pattern list ----------------------------------------------------------
# Forbidden names, whitespace-separated, matched case-insensitively as fixed
# strings. A match counts when the character before it is not a letter or
# digit and the character after it is not a letter. So separators such as
# _ - . / and start of line may precede a name, and digits, _ - . may follow
# it (version and ID suffixes still match), while plurals and longer words
# (an "s" or other letter suffix, a letter or digit prefix) do not.
# To extend: add a lowercase word. The forbidden words appear here only, as
# data; the allowlist below holds entrypoint file names, not forbidden words.
PATTERNS='
gpt chatgpt openai codex claude anthropic opus sonnet haiku fable gemini
copilot deepseek llama mistral grok qwen
'

# --- Allowlist -------------------------------------------------------------
# Tool entrypoint file names: they say where a tool reads instructions, not
# which model runs. Exact, case-sensitive tokens, one per line, no whitespace.
# A token is removed from a reported line only where the pattern rule would
# also treat it as standing alone (no letter or digit before it, no letter
# after it); the rest of the line is still tested, and no file is ever
# skipped. To extend: add a line with the exact token.
ALLOWLIST='
CLAUDE.md
.github/copilot-instructions.md
copilot-instructions.md
'

# ---------------------------------------------------------------------------

me='scripts/check-guidance-names.sh'

top=$(git rev-parse --show-toplevel 2>/dev/null) || {
	echo "$me: not inside a git repository" >&2
	exit 2
}
cd "$top" || exit 2

# Pathspecs are passed through unquoted word splitting; disable globbing so
# git, not the shell, interprets them.
set -f

# Fail closed: every scanned entry must match a tracked file.
for entry in $SCANNED; do
	if ! git ls-files --error-unmatch -- "$entry" >/dev/null 2>&1; then
		echo "$me: scanned-set entry matched no tracked file: $entry" >&2
		exit 2
	fi
done

# Count the scanned files, and refuse paths containing ':' because hit lines
# are parsed as path:line:text.
set +e
# shellcheck disable=SC2086 # word splitting of the pathspec list is intended
nfiles=$(git ls-files -- $SCANNED | awk '
	index($0, ":") { print "unsupported tracked path (contains a colon): " $0; bad = 1 }
	END { if (bad) exit 2; print NR }
')
st=$?
set -e
if [ "$st" -ne 0 ]; then
	echo "$me: listing tracked files failed (status $st)" >&2
	[ -z "$nfiles" ] || printf '%s\n' "$nfiles" >&2
	exit 2
fi

# Build the git grep arguments: one -e per pattern.
set --
for p in $PATTERNS; do
	set -- "$@" -e "$p"
done

# Prefilter with git grep: any line containing a pattern as a substring is a
# candidate; the boundary rule is applied in awk below. --text scans files git
# would treat as binary (attributes or NUL bytes) instead of skipping them.
# Explicit flags override user config that would change the output shape
# (color, column numbers, relative paths).
set +e
# shellcheck disable=SC2086 # word splitting of the pathspec list is intended
hits=$(git -c grep.column=false -c grep.fullName=true \
	grep --no-color -n --text -i -F "$@" -- $SCANNED)
gs=$?
set -e
case $gs in
0) ;;
1)
	echo "guidance names: ok ($nfiles files scanned)"
	exit 0
	;;
*)
	echo "$me: git grep failed (status $gs)" >&2
	exit 2
	;;
esac

# Re-test each reported line after stripping allowlisted tokens. Residual
# hits (and any parse error) go to stderr.
set +e
printf '%s\n' "$hits" | GUARD_PATTERNS=$PATTERNS GUARD_ALLOW=$ALLOWLIST awk '
function isalpha(c) {
	return c != "" && index("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ", c) > 0
}
function isalnum(c) {
	return isalpha(c) || (c != "" && index("0123456789", c) > 0)
}
# Replace every occurrence of tok in s that has no letter or digit before it
# and no letter after it (the same boundary as the hit test) with a space.
function strip(s, tok,    out, i, n, pre, post) {
	out = ""
	n = length(tok)
	while ((i = index(s, tok)) > 0) {
		pre = (i > 1) ? substr(s, i - 1, 1) : ""
		post = substr(s, i + n, 1)
		if (!isalnum(pre) && !isalpha(post)) {
			out = out substr(s, 1, i - 1) " "
			s = substr(s, i + n)
		} else {
			out = out substr(s, 1, i)
			s = substr(s, i + 1)
		}
	}
	return out s
}
# True if pattern p (lowercase) occurs in s (lowercased) with no letter or
# digit before it and no letter after it.
function hasword(s, p,    i, n, off, pre, post) {
	n = length(p)
	off = 0
	while ((i = index(substr(s, off + 1), p)) > 0) {
		i += off
		pre = (i > 1) ? substr(s, i - 1, 1) : ""
		post = substr(s, i + n, 1)
		if (!isalnum(pre) && !isalpha(post)) return 1
		off = i
	}
	return 0
}
BEGIN {
	npat = split(ENVIRON["GUARD_PATTERNS"], pat)
	nallow = split(ENVIRON["GUARD_ALLOW"], allow)
	# Strip longer tokens first so a token that contains another wins.
	for (a = 1; a <= nallow; a++)
		for (b = a + 1; b <= nallow; b++)
			if (length(allow[b]) > length(allow[a])) { t = allow[a]; allow[a] = allow[b]; allow[b] = t }
	bad = 0
	parse_err = 0
}
$0 == "" { next }
{
	i = index($0, ":")
	rest = substr($0, i + 1)
	j = index(rest, ":")
	if (i == 0 || j == 0) {
		print "unparseable git grep line: " $0
		parse_err = 1
		exit 3
	}
	path = substr($0, 1, i - 1)
	line = substr(rest, 1, j - 1)
	text = substr(rest, j + 1)
	t = text
	for (a = 1; a <= nallow; a++) t = strip(t, allow[a])
	t = tolower(t)
	for (k = 1; k <= npat; k++) {
		if (hasword(t, tolower(pat[k]))) {
			print path ":" line ": " text
			bad = 1
			break
		}
	}
}
END { if (parse_err) exit 3; if (bad) exit 4 }
' >&2
as=$?
set -e
# 4 is the only residual-hit status: awk implementations use 1 and 2 for
# their own errors, which must not read as a clean or a hit result.
case $as in
0)
	echo "guidance names: ok ($nfiles files scanned)"
	exit 0
	;;
4)
	echo "$me: tracked guidance names a model or vendor (see lines above); use role names, or extend the allowlist at the top of $me for a tool entrypoint file name" >&2
	exit 1
	;;
*)
	echo "$me: line re-test failed (awk status $as)" >&2
	exit 2
	;;
esac
