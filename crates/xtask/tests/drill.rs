//! The drill's HTTP side against a live server on a loopback port: what
//! `seed` sets up, `collect` reads back and `compare` finds equal. The API
//! calls the drill makes run here, so a change to the API that breaks the
//! drill fails `cargo test` rather than the next drill.

use std::path::Path;

use consolebook_server::data_dir::DataDir;
use consolebook_server::{http, setup, storage};
use serde_json::{Value, json};
use xtask::drill::account::Account;
use xtask::drill::client::Client;
use xtask::drill::{collect, compare, seed};
use xtask::proof::host::sha256_file;

/// A fresh installation served on a loopback port, with its setup code.
struct Live {
    base: String,
    setup_code: String,
    _runtime: tokio::runtime::Runtime,
    _data: tempfile::TempDir,
}

fn live() -> Live {
    let data = tempfile::tempdir().expect("tempdir");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (base, setup_code) = runtime.block_on(async {
        let dir = DataDir::new(data.path().join("data"));
        dir.ensure_layout().expect("layout");
        let pool = storage::open(&dir.database()).await.expect("open");
        let (code, _) = setup::issue_setup_code(&pool)
            .await
            .expect("setup code")
            .expect("a fresh installation issues one");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(http::serve(listener, http::AppState::new(pool)));
        (format!("http://{address}"), code.raw)
    });
    Live {
        base,
        setup_code,
        _runtime: runtime,
        _data: data,
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read")).expect("json")
}

#[test]
fn a_seeded_installation_is_collected_and_compares_equal() {
    let server = live();
    let work = tempfile::tempdir().expect("tempdir");
    let account = work.path().join("account.json");
    let seeded = seed::seed(&server.base, &server.setup_code, &account).expect("seed");
    assert_eq!((seeded.records, seeded.enrollment), (3, 1));
    let admin = Account::read(&account).expect("account");
    assert_eq!(admin, seeded.admin);
    assert_eq!(admin.username, "avery.admin");

    let before = work.path().join("before");
    let collected = collect::collect(&server.base, &admin, &before).expect("collect");
    assert_eq!(collected.records, 3);
    let manifest = read(&collected.manifest);
    let records = manifest["records"].as_array().expect("records");
    let ids: Vec<&Value> = records.iter().map(|r| &r["record_id"]).collect();
    assert_eq!(ids, [&json!(1), &json!(2), &json!(3)]);
    let kinds: Vec<&Value> = records
        .iter()
        .map(|r| &r["acknowledgment"]["acknowledgment"]["kind"])
        .collect();
    assert_eq!(
        kinds,
        [
            &json!("acknowledged"),
            &json!("acknowledged_with_response"),
            &Value::Null
        ]
    );
    for record in records {
        let meta = &record["meta"];
        assert_eq!(meta["version_number"], json!(1), "{meta}");
        assert_eq!(
            meta["content_hash"].as_str().map(str::len),
            Some(64),
            "{meta}"
        );
        assert_eq!(
            meta["chain_hash"].as_str().map(str::len),
            Some(64),
            "{meta}"
        );
        assert_eq!(record["export"]["units"].as_array().map(Vec::len), Some(1));
        let id = record["record_id"].as_i64().expect("id");
        // Each record holds its own invented content, so records that came
        // back swapped would differ.
        let content = &recorded(&before.join(format!("record-{id}.zip")), id)["content"];
        assert_eq!(content["ratings"][0]["value"], json!(4 + (id - 1) % 2));
        assert_eq!(
            content["narratives"][0]["text"],
            json!(format!(
                "Invented observation {id}: most acceptable performance."
            ))
        );
    }
    assert_eq!(
        manifest["installation_export"]["scope"],
        json!({ "kind": "installation" })
    );
    assert_eq!(
        manifest["installation_export"]["units"]
            .as_array()
            .map(Vec::len),
        Some(3)
    );
    assert!(before.join("installation.zip").is_file());
    let instance = manifest["instance"].as_object().expect("instance");
    assert_eq!(
        instance.get("agency"),
        Some(&json!("Example County Communications"))
    );
    assert!(!instance.contains_key("commit") && !instance.contains_key("version"));
    // The manifest is written with its keys in order.
    let text = std::fs::read_to_string(&collected.manifest).expect("read");
    let keys: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("  \"")?.split_once('"').map(|(k, _)| k))
        .collect();
    assert_eq!(keys, ["installation_export", "instance", "records"]);

    // Read again, the same installation holds the same.
    let again = work.path().join("again");
    collect::collect(&server.base, &admin, &again).expect("collect again");
    let seen = compare::compare(&manifest, &read(&again.join("manifest.json"))).expect("compare");
    assert!(seen.same, "{:?}", seen.report);
}

#[test]
fn a_refused_call_carries_the_servers_answer() {
    let server = live();
    let stranger = Account::invented("nobody.here").expect("account");
    let Err(why) = Client::sign_in(&server.base, &stranger) else {
        panic!("signed in as nobody");
    };
    assert!(why.0.starts_with("POST /api/auth/login: 401 "), "{why}");
    assert!(!why.0.contains(&stranger.passphrase), "{why}");
    // Collect needs an installation set up and an account in it.
    let work = tempfile::tempdir().expect("tempdir");
    assert!(collect::collect(&server.base, &stranger, work.path()).is_err());
    // A second seed of the same installation is refused, and leaves the
    // first one's account file as it was.
    let account = work.path().join("account.json");
    seed::seed(&server.base, &server.setup_code, &account).expect("seed");
    let first = Account::read(&account).expect("account");
    assert!(seed::seed(&server.base, &server.setup_code, &account).is_err());
    assert_eq!(Account::read(&account).expect("account"), first);
    let elsewhere = work.path().join("elsewhere.json");
    let Err(why) = seed::seed(&server.base, &server.setup_code, &elsewhere) else {
        panic!("set up twice");
    };
    assert!(why.0.starts_with("POST /api/setup: "), "{why}");
}

/// `cargo xtask drill …`, as `run.sh` runs it.
fn xtask(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("drill")
        .args(args)
        .output()
        .expect("xtask runs")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn the_commands_run_the_drill_and_exit_by_its_verdict() {
    let server = live();
    let work = tempfile::tempdir().expect("tempdir");
    let path = |name: &str| work.path().join(name).to_string_lossy().into_owned();
    let (account, before, after) = (path("account.json"), path("before"), path("after"));

    let seeded = xtask(&["seed", &server.base, &server.setup_code, &account, &before]);
    assert_eq!(seeded.status.code(), Some(0), "{seeded:?}");
    let text = stdout(&seeded);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "seeded 3 finalized records for enrollment 1");
    let digest = lines[1]
        .strip_prefix("collected 3 records; manifest sha256 ")
        .expect("collect's line");
    let manifest = Path::new(&before).join("manifest.json");
    assert_eq!(digest, sha256_file(&manifest).expect("manifest"));

    let collected = xtask(&["collect", &server.base, &account, &after]);
    assert_eq!(collected.status.code(), Some(0), "{collected:?}");
    let (before_manifest, after_manifest) = (
        format!("{before}/manifest.json"),
        format!("{after}/manifest.json"),
    );
    let same = xtask(&["compare", &before_manifest, &after_manifest]);
    assert_eq!(same.status.code(), Some(0), "{same:?}");
    assert_eq!(stdout(&same).lines().last(), Some("MANIFESTS EQUAL"));

    let mut changed = read(Path::new(&after_manifest));
    changed["records"][2]["meta"]["chain_hash"] = json!("changed");
    std::fs::write(&after_manifest, changed.to_string()).expect("write");
    let differ = xtask(&["compare", &before_manifest, &after_manifest]);
    assert_eq!(differ.status.code(), Some(1), "{differ:?}");
    assert_eq!(stdout(&differ).lines().last(), Some("MANIFESTS DIFFER"));

    // A manifest it can't read, a sign-in it can't find, and a call it
    // doesn't know.
    let missing = path("missing.json");
    assert_eq!(
        xtask(&["compare", &before_manifest, &missing])
            .status
            .code(),
        Some(2)
    );
    let unread = xtask(&["collect", &server.base, &missing, &path("again")]);
    assert_eq!(unread.status.code(), Some(1), "{unread:?}");
    // Too few words, or an unknown command: a usage error, read before any
    // file or server is.
    for args in [
        &[][..],
        &["seed", &server.base, "account.json"][..],
        &["collect", &server.base][..],
        &["compare", &before_manifest][..],
        &["verify"][..],
    ] {
        assert_eq!(xtask(args).status.code(), Some(2), "{args:?}");
    }
    assert_eq!(xtask(&["--help"]).status.code(), Some(0));
}

/// The `record.json` of record `id`'s first version, from its export.
fn recorded(archive: &Path, id: i64) -> Value {
    let file = std::fs::File::open(archive).expect("export");
    let mut zip = zip::ZipArchive::new(file).expect("zip");
    let entry = zip
        .by_name(&format!("records/{id}/v1/record.json"))
        .expect("record.json");
    serde_json::from_reader(entry).expect("json")
}
