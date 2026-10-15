#!/bin/sh
# Install or upgrade Consolebook on a Linux server with systemd.
#
#   curl -fsSL https://github.com/FieldmouseWorks/consolebook/releases/latest/download/install.sh | sudo sh
#
# The same command upgrades an existing installation and keeps its data.
# It downloads the release for this machine, checks its SHA-256, installs
# /usr/local/bin/consolebook, a `consolebook` service account, the data
# directory /var/lib/consolebook, and the `consolebook` systemd service, then
# starts it and prints the address and first-run setup code.
#
# Options (after `sh -s --` when piping):
#   --backup-dir DIR  also copy every backup to DIR, an already-mounted network
#                     share or another disk (strongly recommended; see docs/install.md)
#   --data-dir DIR    keep the database and local backups in DIR
#                     (default /var/lib/consolebook)
#   --version X.Y.Z   install that release instead of the latest
#   --lan             listen on every network interface (port 7770)
#   --listen ADDR     listen on ADDR, e.g. 0.0.0.0:8080 (default 127.0.0.1:7770)
#   --public-url URL  the https:// address people use through your TLS proxy;
#                     session cookies become Secure (see docs/install.md)
#   --from FILE       install from a downloaded release archive
#   --no-service      install files only; do not create or start the service
#
# Exit: 0 installed and running (or installed, with --no-service), 1 failure.
# See docs/install.md.

set -eu

REPO="FieldmouseWorks/consolebook"
# The release workflow stamps the installer it publishes with that release's
# version, so the installer fetched from releases/latest installs the
# archives it shipped with, even if a newer release appears in between
# (ADR 0026). Empty here: the latest release.
RELEASED_VERSION=""
BIN=/usr/local/bin/consolebook
DEFAULT_DATA=/var/lib/consolebook
ETC=/etc/consolebook
ENV_FILE="$ETC/consolebook.env"
UNIT=/etc/systemd/system/consolebook.service
DROPIN_DIR=/etc/systemd/system/consolebook.service.d
DROPIN="$DROPIN_DIR/paths.conf"
ACCOUNT=consolebook
DEFAULT_LISTEN=127.0.0.1:7770

version="$RELEASED_VERSION"
from=""
listen=""
public_url=""
data_dir=""
backup_dir=""
service=yes

say() { printf '%s\n' "$*"; }
fail() {
	printf 'consolebook install: %s\n' "$*" >&2
	exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }
warn_backups() {
	if [ -n "${COPY:-}" ]; then
		say "Backups are also copied to $COPY."
		return
	fi
	say ""
	say "!! WARNING: backups are kept only on this server's own disk."
	say "!! If that disk fails, the records and every backup are lost together."
	say "!! Mount another disk or a network share, then rerun this installer with:"
	say "!!   --backup-dir /path/to/that/mount"
}
usage() {
	cat <<'EOF'
Install or upgrade Consolebook (Linux with systemd). Run as root.

  --backup-dir DIR  also copy every backup to DIR, an already-mounted network
                    share or another disk (strongly recommended)
  --data-dir DIR    keep the database and local backups in DIR
                    (default /var/lib/consolebook)
  --version X.Y.Z   install that release instead of the latest
  --lan             listen on every network interface (port 7770)
  --listen ADDR     listen on ADDR, e.g. 0.0.0.0:8080 (default 127.0.0.1:7770)
  --public-url URL  the https:// address people use through your TLS proxy
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
	--public-url)
		[ $# -ge 2 ] || fail "--public-url needs an address such as https://consolebook.example.org"
		case "$2" in
		https://* | http://*) public_url=${2%/} ;;
		*) fail "--public-url must start with https:// (for example https://consolebook.example.org)" ;;
		esac
		shift 2
		;;
	--from)
		[ $# -ge 2 ] || fail "--from needs a release archive path"
		from=$2
		shift 2
		;;
	--data-dir)
		[ $# -ge 2 ] || fail "--data-dir needs a directory"
		data_dir=$2
		shift 2
		;;
	--backup-dir)
		[ $# -ge 2 ] || fail "--backup-dir needs a directory"
		backup_dir=$2
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
for path in "$data_dir" "$backup_dir"; do
	case "$path" in
	"" | /*) ;;
	*) fail "directories must be absolute paths: $path" ;;
	esac
done
# The data and copy directories are checked against each other once the
# effective settings (options plus the existing env file) are known.

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

# Check the effective directories before stopping or replacing anything.
env_value() {
	if [ -f "$ENV_FILE" ]; then
		sed -n "s/^$1=//p" "$ENV_FILE" | tail -n 1
	fi
}
current_data=$(env_value CONSOLEBOOK_DATA_DIR)
current_data=${current_data:-$DEFAULT_DATA}
# Pointing an installation that has records at another directory would start
# a new, empty one in its place; moving data is a deliberate, manual step.
if [ -n "$data_dir" ] && [ "${data_dir%/}" != "${current_data%/}" ] && [ -f "$current_data/consolebook.db" ]; then
	fail "this installation's records are in $current_data; --data-dir would start an empty installation in $data_dir instead. To move them: stop the service, move $current_data (with its backups) to $data_dir, then rerun with --data-dir $data_dir (see docs/install.md)"
fi
effective_data=${data_dir:-$current_data}
effective_copy=${backup_dir:-$(env_value CONSOLEBOOK_BACKUP_COPY_DIR)}
if [ -n "$effective_copy" ]; then
	case "$effective_copy/" in
	"$effective_data"/*)
		fail "the backup copy directory must be outside the data directory ($effective_data); a copy on the same disk protects nothing"
		;;
	esac
fi
# The copy directory must already be a mounted share or another disk. The
# installer never creates it: on an unmounted share that would put the
# "off-server" copies on this server's own disk.
existing_ancestor() {
	dir=$1
	while [ ! -d "$dir" ]; do dir=$(dirname "$dir"); done
	printf '%s\n' "$dir"
}
copy_problem() {
	if [ ! -d "$1" ]; then
		echo "does not exist; mount the share or disk there first"
	elif [ "$(stat -c %d "$1")" = "$(stat -c %d "$(existing_ancestor "$effective_data")")" ]; then
		echo "is on the same filesystem as the data directory; mount the share or disk there first"
	fi
}
if [ -n "$backup_dir" ]; then
	problem=$(copy_problem "$backup_dir")
	[ -z "$problem" ] || fail "--backup-dir $backup_dir $problem"
elif [ -n "$effective_copy" ]; then
	problem=$(copy_problem "$effective_copy")
	[ -z "$problem" ] || say "warning: backup copy directory $effective_copy $problem; backups stay local until it is"
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
# The server's own parser judges the address, before anything is stopped or
# written: a value it would refuse at start must not take a working service down.
if [ -n "$public_url" ]; then
	public_url=$("$release/consolebook" check-public-url "$public_url") ||
		fail "--public-url $public_url is not a usable address (scheme, host, and optional port only); nothing was changed"
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
		useradd --system --user-group --home-dir "${data_dir:-$DEFAULT_DATA}" --no-create-home --shell "$nologin" "$ACCOUNT"
	elif have adduser; then
		adduser -S -D -H -h "${data_dir:-$DEFAULT_DATA}" -s "$nologin" "$ACCOUNT"
	else
		fail "need useradd or adduser to create the $ACCOUNT account"
	fi
fi
install -d -m 0755 "$ETC"

# Settings persist across upgrades; options given now replace their lines.
set_setting() {
	if grep -q "^$1=" "$ENV_FILE"; then
		sed -i "s|^$1=.*|$1=$2|" "$ENV_FILE"
	else
		printf '%s=%s\n' "$1" "$2" >>"$ENV_FILE"
	fi
}
if [ ! -f "$ENV_FILE" ]; then
	cat >"$ENV_FILE" <<EOF
# Consolebook service settings; read by consolebook.service.
CONSOLEBOOK_DATA_DIR=${data_dir:-$DEFAULT_DATA}
CONSOLEBOOK_BIND=${listen:-$DEFAULT_LISTEN}
EOF
	chmod 0644 "$ENV_FILE"
fi
if [ -n "$listen" ]; then set_setting CONSOLEBOOK_BIND "$listen"; fi
if [ -n "$data_dir" ]; then set_setting CONSOLEBOOK_DATA_DIR "$data_dir"; fi
if [ -n "$backup_dir" ]; then set_setting CONSOLEBOOK_BACKUP_COPY_DIR "$backup_dir"; fi
if [ -n "$public_url" ]; then set_setting CONSOLEBOOK_PUBLIC_URL "$public_url"; fi
PUBLIC=$(sed -n 's/^CONSOLEBOOK_PUBLIC_URL=//p' "$ENV_FILE" | tail -n 1)
DATA=$(sed -n 's/^CONSOLEBOOK_DATA_DIR=//p' "$ENV_FILE" | tail -n 1)
DATA=${DATA:-$DEFAULT_DATA}
COPY=$(sed -n 's/^CONSOLEBOOK_BACKUP_COPY_DIR=//p' "$ENV_FILE" | tail -n 1)
install -d -m 0750 -o "$ACCOUNT" -g "$ACCOUNT" "$DATA"
if [ -n "$COPY" ] && [ -d "$COPY" ]; then
	# A mounted share may not accept ownership changes; it must be writable
	# by the account either way, which the service proves on its first copy.
	chown "$ACCOUNT:$ACCOUNT" "$COPY" 2>/dev/null || true
	chmod 0750 "$COPY" 2>/dev/null || true
fi
bind=$(sed -n 's/^CONSOLEBOOK_BIND=//p' "$ENV_FILE" | tail -n 1)
bind=${bind:-$DEFAULT_LISTEN}
port=${bind##*:}
host=${bind%:*}
# Where this machine can reach the service: the bound address itself, or
# loopback when it listens on every interface.
case "$host" in
0.0.0.0) local_host=127.0.0.1 ;;
"[::]") local_host="[::1]" ;;
*) local_host=$host ;;
esac

say "Installed $("$BIN" --version)."

if [ "$service" = no ]; then
	say ""
	say "Files installed; no service was created. Start it with:"
	say "  sudo -u $ACCOUNT CONSOLEBOOK_DATA_DIR=$DATA${COPY:+ CONSOLEBOOK_BACKUP_COPY_DIR=$COPY}${PUBLIC:+ CONSOLEBOOK_PUBLIC_URL=$PUBLIC} $BIN serve --bind $bind"
	warn_backups
	exit 0
fi

# --- Service -----------------------------------------------------------------
install -m 0644 "$release/consolebook.service" "$UNIT"
# The unit's sandbox writes only the default data directory; let it write
# the configured ones instead.
install -d -m 0755 "$DROPIN_DIR"
{
	echo "# Written by install.sh from $ENV_FILE; rerun the installer to change."
	echo "[Service]"
	echo "ReadWritePaths=$DATA"
	# Optional ("-"): a share missing at start must not stop the service;
	# backups stay local and health reports the copy location unreachable.
	if [ -n "$COPY" ]; then echo "ReadWritePaths=-$COPY"; fi
} >"$DROPIN"
systemctl daemon-reload
systemctl enable consolebook >/dev/null 2>&1
systemctl restart consolebook

i=0
until probe "http://$local_host:$port/api/health" >/dev/null; do
	i=$((i + 1))
	if [ "$i" -ge 30 ]; then
		systemctl --no-pager status consolebook >&2 || true
		fail "the service did not answer at $local_host:$port within 30 seconds; see: journalctl -u consolebook"
	fi
	sleep 1
done

case "$host" in
0.0.0.0 | "[::]" | ::)
	address=$(hostname -I 2>/dev/null | cut -d' ' -f1)
	address=${address:-$(hostname)}
	case "$address" in
	*:*) address="[$address]" ;; # an IPv6 literal needs brackets in a URL
	esac
	url="http://$address:$port/"
	;;
*) url="http://$host:$port/" ;;
esac
# The published address, when there is one, is where people should go.
if [ -n "$PUBLIC" ]; then url="$PUBLIC/"; fi

say ""
if probe "http://$local_host:$port/api/instance" | grep -q '"initialized":false'; then
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
case "$PUBLIC:$host" in
https://*)
	say ""
	say "People reach it through HTTPS at $PUBLIC; sign-in cookies are Secure."
	;;
*:127.* | *:localhost | *:::1 | *:"[::1]")
	say ""
	say "It listens on this machine only. To open it from other computers, put HTTPS in"
	say "front of it and rerun with --public-url, or use --lan for a plain-HTTP trial;"
	say "see docs/install.md."
	;;
*)
	say ""
	say "It speaks plain HTTP on your network. For anything beyond a trial, put HTTPS in"
	say "front of it; see docs/install.md."
	;;
esac
say "Data and automatic backups: $DATA   Logs: journalctl -u consolebook"
warn_backups
