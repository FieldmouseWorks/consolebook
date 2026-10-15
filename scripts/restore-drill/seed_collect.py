#!/usr/bin/env python3
"""Seed an invented installation, or read back what it holds, for a restore drill.

    seed_collect.py seed    BASE_URL SETUP_CODE OUT_DIR
    seed_collect.py collect BASE_URL OUT_DIR

`seed` initializes a fresh installation over its HTTP API with invented
people and records:
- three daily evaluations, finalized;
- two of them acknowledged by the trainee, one with a response.
`collect` signs in as the invented administrator and writes `manifest.json`:
every finalized record's version metadata (content and chain hashes),
acknowledgment, and the SHA-256 of a fresh record export. It saves the
exports beside it. Run `collect` before and after a restore and compare
the manifests.

Everything here is invented: agency, people, dates, and narratives.
"""

import hashlib
import http.cookiejar
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path

AGENCY = "Example County Communications"
ADMIN = ("avery.admin", "Avery Admin", "invented-passphrase-1")
TRAINEE = ("taylor.trainee", "Taylor Trainee", "trainee-passphrase-2")
TRAINER = ("jordan.trainer", "Jordan Trainer", "trainer-passphrase-3", "trainer")
COORDINATOR = ("casey.coord", "Casey Coordinator", "coordinator-passphrase-4", "coordinator")
DATES = ["2026-06-02", "2026-06-03", "2026-06-04"]

PROGRAM = {
    "name": "Example County CTO Program",
    "label": "2026 rev A",
    "description": "Invented program for a restore drill.",
    "phases": [{"name": "Phase One", "description": "Observation.", "presentation_number": 1}],
    "phase_transitions": [],
    "competencies": [
        {
            "category": "Call processing",
            "name": "Emergency Call Interrogation",
            "description": "Obtains and verifies location, callback, and nature.",
            "tasks": [{"prompt": "Processes an invented structure-fire call.", "citations": []}],
            "citations": [],
        }
    ],
    "rating_scales": [
        {
            "name": "Standard 1-7",
            "kind": "anchored_numeric",
            "min_value": 1,
            "max_value": 7,
            "anchors": [
                {"value": 1, "label": "Unacceptable", "definition": "Contrary to training."},
                {"value": 4, "label": "Meets standards", "definition": "To the invented standard."},
            ],
        }
    ],
    "rating_modifiers": [],
    "evaluation_forms": [
        {
            "record_type": "daily_report",
            "name": "Daily Observation Report",
            "instructions": "Rate observed performance.",
            "competencies": [{"competency": "Emergency Call Interrogation", "rating_scale": "Standard 1-7"}],
            "narratives": [
                {"prompt": "Most acceptable performance.", "required": False},
                {"prompt": "Least acceptable performance.", "required": False},
            ],
        }
    ],
    "citations": [],
    "finalization_policy": {"review_approved": False, "required_narratives": False, "ratings_complete": False},
}


class Client:
    """One signed-in browser, more or less: a cookie jar and JSON calls."""

    def __init__(self, base: str):
        self.base = base.rstrip("/")
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())
        )

    def call(self, method: str, path: str, body=None, raw: bool = False):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.base + path, data=data, method=method)
        if data is not None:
            request.add_header("Content-Type", "application/json")
        try:
            with self.opener.open(request, timeout=60) as response:
                payload = response.read()
        except urllib.error.HTTPError as err:
            raise SystemExit(f"{method} {path}: {err.code} {err.read().decode(errors='replace')}")
        if raw:
            return payload
        return json.loads(payload) if payload else None

    def sign_in(self, username: str, password: str) -> "Client":
        self.call("POST", "/api/auth/login", {"username": username, "password": password})
        return self


def user(admin: Client, base: str, who) -> tuple[int, Client]:
    username, display, password, *role = who
    body = {"username": username, "display_name": display}
    if role:
        body["role"] = role[0]
    created = admin.call("POST", "/api/users", body)
    Client(base).call(
        "POST",
        "/api/auth/reset",
        {"username": username, "reset_code": created["reset_code"], "new_password": password},
    )
    return created["id"], Client(base).sign_in(username, password)


def seed(base: str, setup_code: str) -> None:
    username, display, password = ADMIN
    Client(base).call(
        "POST",
        "/api/setup",
        {
            "setup_code": setup_code,
            "agency_name": AGENCY,
            "username": username,
            "display_name": display,
            "password": password,
        },
    )
    admin = Client(base).sign_in(username, password)
    program = admin.call("POST", "/api/programs", {"name": PROGRAM["name"]})
    version = admin.call("POST", f"/api/programs/{program['id']}/versions", PROGRAM)
    admin.call("POST", f"/api/program-versions/{version['id']}/publish", {})
    trainee_id, trainee = user(admin, base, TRAINEE)
    trainer_id, trainer = user(admin, base, TRAINER)
    _, coordinator = user(admin, base, COORDINATOR)
    enrollment = admin.call(
        "POST", f"/api/program-versions/{version['id']}/enrollments", {"user_id": trainee_id}
    )
    admin.call(
        "POST", f"/api/enrollments/{enrollment['id']}/assignments", {"trainer_user_id": trainer_id}
    )
    for n, date in enumerate(DATES):
        session = admin.call(
            "POST",
            f"/api/enrollments/{enrollment['id']}/sessions",
            {
                "business_date": date,
                "timezone": "America/Chicago",
                "local_start": f"{date}T07:00",
                "trainer_user_ids": [trainer_id],
            },
        )
        trainer.call(
            "POST",
            f"/api/sessions/{session['id']}/close",
            {"disposition": "completed", "local_end": f"{date}T17:00"},
        )
        draft = admin.call("POST", f"/api/sessions/{session['id']}/draft", {})
        view = trainer.call("GET", f"/api/drafts/{draft['id']}")
        content = {
            "revision": view["revision"],
            "ratings": [
                {"form_competency_id": c["form_competency_id"], "value": 4 + n % 2}
                for c in view["form"]["competencies"]
            ],
            "narratives": [
                {
                    "form_narrative_id": r["form_narrative_id"],
                    "text": f"Invented observation {n + 1}: {r['prompt'].lower()}",
                }
                for r in view["form"]["narratives"]
            ],
        }
        saved = trainer.call("PUT", f"/api/drafts/{draft['id']}/content", content)
        revision = (saved or {}).get("revision") or trainer.call("GET", f"/api/drafts/{draft['id']}")["revision"]
        coordinator.call("POST", f"/api/drafts/{draft['id']}/finalize", {"revision": revision})
        if n == 0:
            trainee.call("POST", f"/api/drafts/{draft['id']}/acknowledge", {"kind": "acknowledged"})
        elif n == 1:
            trainee.call(
                "POST",
                f"/api/drafts/{draft['id']}/acknowledge",
                {"kind": "acknowledged_with_response", "response": "Invented trainee response."},
            )
    print(f"seeded {len(DATES)} finalized records for enrollment {enrollment['id']}")


def collect(base: str, out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    username, _, password = ADMIN
    admin = Client(base).sign_in(username, password)
    manifest = {"instance": admin.call("GET", "/api/instance"), "records": []}
    manifest["instance"].pop("commit", None)
    manifest["instance"].pop("version", None)
    record_id = 0
    misses = 0
    # Record ids are small integers; walk them until a run of misses.
    while misses < 5:
        record_id += 1
        request = urllib.request.Request(f"{admin.base}/api/drafts/{record_id}/version")
        try:
            with admin.opener.open(request, timeout=60) as response:
                version = json.loads(response.read())
        except urllib.error.HTTPError:
            misses += 1
            continue
        misses = 0
        ack = None
        try:
            ack = admin.call("GET", f"/api/drafts/{record_id}/acknowledgment")
        except SystemExit:
            pass
        archive = admin.call("GET", f"/api/drafts/{record_id}/export", raw=True)
        path = out / f"record-{record_id}.zip"
        path.write_bytes(archive)
        manifest["records"].append(
            {
                "record_id": record_id,
                "meta": version["meta"],
                "acknowledgment": ack,
                "export": path.name,
            }
        )
    everything = admin.call("GET", "/api/exports/records", raw=True)
    (out / "installation.zip").write_bytes(everything)
    manifest["installation_export_bytes"] = len(everything)
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    digest = hashlib.sha256((out / "manifest.json").read_bytes()).hexdigest()
    print(f"collected {len(manifest['records'])} records; manifest sha256 {digest}")


def main() -> None:
    if len(sys.argv) == 5 and sys.argv[1] == "seed":
        seed(sys.argv[2], sys.argv[3])
        collect(sys.argv[2], Path(sys.argv[4]))
    elif len(sys.argv) == 4 and sys.argv[1] == "collect":
        collect(sys.argv[2], Path(sys.argv[3]))
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
