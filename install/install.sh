#!/bin/sh
# Install or upgrade Consolebook on a Linux server with systemd.
#
#   curl -fsSL https://raw.githubusercontent.com/FieldmouseWorks/consolebook/main/install/install.sh | sudo sh
#
# The same command upgrades an existing installation and keeps its data.
# It downloads the release for this machine, checks its SHA-256, installs
# /usr/local/bin/consolebook, a `consolebook` service account, the data
# directory /var/lib/consolebook, and the `consolebook` systemd service, then
# starts it and prints the address and first-run setup code.
#
# Options (after `sh -s --` when piping):
#   --version X.Y.Z   install that release instead of the latest
#   --lan             listen on every network interface (port 7770)
#   --listen ADDR     listen on ADDR, e.g. 0.0.0.0:8080 (default 127.0.0.1:7770)
#   --from FILE       install from a downloaded release archive
#   --no-service      install files only; do not create or start the service
#
# Exit: 0 installed and running (or installed, with --no-service), 1 failure.
# See docs/install.md.

set -eu

REPO="FieldmouseWorks/consolebook"
BIN=/usr/local/bin/consolebook
DATA=/var/lib/consolebook
ETC=/etc/consolebook
ENV_FILE="$ETC/consolebook.env"
UNIT=/etc/systemd/system/consolebook.service
ACCOUNT=consolebook
DEFAULT_LISTEN=127.0.0.1:7770

version=""
from=""
listen=""
service=yes

say() { printf '%s\n' "$*"; }
fail() {
	printf 'consolebook install: %s\n' "$*" >&2
	exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }
usage() {
	cat <<'EOF'
Install or upgrade Consolebook (Linux with systemd). Run as root.

  --version X.Y.Z   install that release instead of the latest
  --lan             listen on every network interface (port 7770)
  --listen ADDR     listen on ADDR, e.g. 0.0.0.0:8080 (default 127.0.0.1:7770)
  --from FILE       install from a downloaded release archive
  --no-service      install files only; do not create or start the service

See https://github.com/FieldmouseWorks/consolebook/blob/main/docs/install.md
EOF
}

while [ $# -gt 0 ]; do
	case "$1" in
	--version)
		[ $# -ge 2 ] || fail "--version needs a value such as 0.1.0"
		version=${2#v}
		shift 2
		;;
	--lan)
		listen=0.0.0.0:7770
		shift
		;;
	--listen)
		[ $# -ge 2 ] || fail "--listen needs an address such as 0.0.0.0:7770"
		listen=$2
		shift 2
		;;
	--from)
		[ $# -ge 2 ] || fail "--from needs a release archive path"
		from=$2
		shift 2
		;;
	--no-service)
		service=no
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	*) fail "unknown option: $1 (try --help)" ;;
	esac
done

[ "$(id -u)" -eq 0 ] || fail "run as root, for example: curl -fsSL ... | sudo sh"
[ "$(uname -s)" = Linux ] || fail "this installer supports Linux only"

case "$(uname -m)" in
x86_64 | amd64) arch=x86_64 ;;
aarch64 | arm64) arch=aarch64 ;;
*) fail "no release is built for $(uname -m) yet" ;;
esac

if have sha256sum; then
	sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif have shasum; then
	sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
	fail "need sha256sum or shasum to verify the download"
fi

if have curl; then
	fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
	probe() { curl -fsS --max-time 2 "$1" 2>/dev/null; }
elif have wget; then
	fetch() { wget -q -O "$2" "$1"; }
	probe() { wget -q -T 2 -O - "$1" 2>/dev/null; }
else
	fail "need curl or wget"
fi

if [ "$service" = yes ]; then
	have systemctl || fail "systemd is required for the service; use --no-service to install files only"
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

# --- Obtain and verify the release archive --------------------------------
asset="consolebook-linux-$arch.tar.gz"
if [ -n "$from" ]; then
	[ -f "$from" ] || fail "no such archive: $from"
	cp "$from" "$work/$asset"
	if [ -f "$from.sha256" ]; then
		cp "$from.sha256" "$work/$asset.sha256"
	else
		say "warning: no $from.sha256 beside the archive; skipping the checksum"
	fi
else
	if [ -n "$version" ]; then
		base="https://github.com/$REPO/releases/download/v$version"
	else
		base="https://github.com/$REPO/releases/latest/download"
	fi
	say "Downloading $asset${version:+ ($version)}..."
	fetch "$base/$asset" "$work/$asset" || fail "download failed: $base/$asset"
	fetch "$base/$asset.sha256" "$work/$asset.sha256" || fail "download failed: $base/$asset.sha256"
fi
if [ -f "$work/$asset.sha256" ]; then
	expected=$(cut -d' ' -f1 <"$work/$asset.sha256")
	actual=$(sha256 "$work/$asset")
	[ "$expected" = "$actual" ] || fail "checksum mismatch for $asset (expected $expected, got $actual)"
	say "Checksum verified."
fi

tar -xzf "$work/$asset" -C "$work"
release=$(find "$work" -mindepth 1 -maxdepth 1 -type d -name 'consolebook-*' | head -n 1)
if [ -z "$release" ] || [ ! -x "$release/consolebook" ]; then
	fail "the archive does not contain a consolebook executable"
fi

# --- Install files -----------------------------------------------------------
upgrading=no
[ -x "$BIN" ] && upgrading=yes
if [ "$service" = yes ] && [ "$upgrading" = yes ]; then
	systemctl stop consolebook 2>/dev/null || true
fi
install -m 0755 "$release/consolebook" "$BIN"

if ! id "$ACCOUNT" >/dev/null 2>&1; then
	nologin=$(command -v nologin || echo /bin/false)
	if have useradd; then
		useradd --system --user-group --home-dir "$DATA" --no-create-home --shell "$nologin" "$ACCOUNT"
	elif have adduser; then
		adduser -S -D -H -h "$DATA" -s "$nologin" "$ACCOUNT"
	else
		fail "need useradd or adduser to create the $ACCOUNT account"
	fi
fi
install -d -m 0750 -o "$ACCOUNT" -g "$ACCOUNT" "$DATA"
install -d -m 0755 "$ETC"

if [ ! -f "$ENV_FILE" ]; then
	cat >"$ENV_FILE" <<EOF
# Consolebook service settings; read by consolebook.service.
CONSOLEBOOK_DATA_DIR=$DATA
CONSOLEBOOK_BIND=${listen:-$DEFAULT_LISTEN}
EOF
	chmod 0644 "$ENV_FILE"
elif [ -n "$listen" ]; then
	sed -i "s|^CONSOLEBOOK_BIND=.*|CONSOLEBOOK_BIND=$listen|" "$ENV_FILE"
fi
bind=$(sed -n 's/^CONSOLEBOOK_BIND=//p' "$ENV_FILE" | tail -n 1)
bind=${bind:-$DEFAULT_LISTEN}
port=${bind##*:}
host=${bind%:*}

say "Installed $("$BIN" --version)."

if [ "$service" = no ]; then
	say ""
	say "Files installed; no service was created. Start it with:"
	say "  sudo -u $ACCOUNT CONSOLEBOOK_DATA_DIR=$DATA $BIN serve --bind $bind"
	exit 0
fi

# --- Service -----------------------------------------------------------------
install -m 0644 "$release/consolebook.service" "$UNIT"
systemctl daemon-reload
systemctl enable consolebook >/dev/null 2>&1
systemctl restart consolebook

i=0
until probe "http://127.0.0.1:$port/api/health" >/dev/null; do
	i=$((i + 1))
	if [ "$i" -ge 30 ]; then
		systemctl --no-pager status consolebook >&2 || true
		fail "the service did not answer on port $port within 30 seconds; see: journalctl -u consolebook"
	fi
	sleep 1
done

case "$host" in
0.0.0.0 | "[::]" | ::)
	address=$(hostname -I 2>/dev/null | cut -d' ' -f1)
	address=${address:-$(hostname)}
	url="http://$address:$port/"
	;;
*) url="http://$host:$port/" ;;
esac

say ""
if probe "http://127.0.0.1:$port/api/instance" | grep -q '"initialized":false'; then
	code=$(runuser -u "$ACCOUNT" -- env CONSOLEBOOK_DATA_DIR="$DATA" "$BIN" setup-code 2>/dev/null) ||
		fail "could not create a setup code; run: sudo -u $ACCOUNT CONSOLEBOOK_DATA_DIR=$DATA $BIN setup-code"
	say "Consolebook is running. Finish setup in a web browser:"
	say ""
	say "  1. Open        $url"
	say "  2. Setup code  $code"
	say "  3. Name your agency and create the first administrator."
	say ""
	say "The code expires; for a new one run:"
	say "  sudo -u $ACCOUNT CONSOLEBOOK_DATA_DIR=$DATA $BIN setup-code"
else
	if [ "$upgrading" = yes ]; then
		say "Consolebook was upgraded and is running at $url"
	else
		say "Consolebook is running at $url"
	fi
fi
case "$host" in
127.* | localhost | ::1 | "[::1]")
	say ""
	say "It listens on this machine only. To open it from other computers, rerun with --lan"
	say "(plain HTTP) or put HTTPS in front of it; see docs/install.md."
	;;
*)
	say ""
	say "It speaks plain HTTP on your network. For anything beyond a trial, put HTTPS in"
	say "front of it; see docs/install.md."
	;;
esac
say "Data and automatic backups: $DATA   Logs: journalctl -u consolebook"
