//! Process-level invented-data restore drill, clean-installation recovery,
//! stale/missing/corrupt evidence refusal, unreachable copy storage,
//! catch-up non-republication, and ADR 0027 externally encrypted copy
//! recovery tests (#213).

use std::path::Path;

use consolebook_server::backup_copy;
use consolebook_server::data_dir::DataDir;
use consolebook_server::disposition_recovery::{
    self, DISPOSITION_LEDGER_FILENAME, DispositionLedger, DispositionLedgerEntry,
    RestoreLedgerReport,
};
use consolebook_server::restore::{self, RestoreOptions};
use consolebook_server::{backup, doctor, storage};

use crate::{DrillInstallation, other_filesystem, run_cli};

fn list_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Simulates storage-layer / operator-managed external encryption around a
/// plain SQLite backup copy (ADR 0027: Consolebook copies are plain SQLite
/// files; confidentiality at rest is provided by agency storage or external
/// tooling, and copies must be decrypted before `consolebook restore`).
fn simulate_external_encrypt(plain: &[u8]) -> Vec<u8> {
    let mut out = b"INVENTED-EXTERNAL-ENC-V1:".to_vec();
    out.extend(plain.iter().map(|b| b ^ 0x5a));
    out
}

fn simulate_external_decrypt(cipher: &[u8]) -> Vec<u8> {
    let payload = cipher
        .strip_prefix(b"INVENTED-EXTERNAL-ENC-V1:")
        .expect("header");
    payload.iter().map(|b| b ^ 0x5a).collect()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn restore_drill_blocks_pre_disposition_resurrection_and_preserves_evidence_on_clean_host() {
    let fx = DrillInstallation::new().await;

    // 1. CLI restore of a pre-disposition snapshot onto the live host FAILS
    // CLOSED, leaves the live DB untouched, and writes no pre-restore snapshot.
    let backups_before = list_names(&fx.data_dir.backups());
    let db_before = std::fs::read(fx.data_dir.database()).expect("db before");
    let out = run_cli(
        &fx.data_dir,
        &[
            "restore",
            fx.pre_disposition_snapshot.to_str().expect("utf8"),
        ],
    );
    assert!(
        !out.status.success(),
        "pre-disposition snapshot must be refused"
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("lawfully disposed")
            && combined.contains(&fx.disposed_record_id.to_string()),
        "{combined}"
    );
    assert_eq!(list_names(&fx.data_dir.backups()), backups_before);
    assert_eq!(
        std::fs::read(fx.data_dir.database()).expect("db after"),
        db_before
    );

    // 2. Clean-installation recovery: brand-new host with empty data dir.
    let clean_tmp = tempfile::tempdir().expect("clean host");
    let clean_data = DataDir::new(clean_tmp.path().join("data"));
    clean_data.ensure_layout().expect("layout");
    let exported_ledger = clean_tmp.path().join("offline-disposition-ledger.json");
    std::fs::copy(fx.data_dir.disposition_ledger(), &exported_ledger).expect("export ledger");

    // 2a. Clean host + --backup-copy-dir mirror refuses pre-disposition snapshot.
    let out = run_cli(
        &clean_data,
        &[
            "restore",
            "--backup-copy-dir",
            fx.copy_dir.to_str().expect("utf8"),
            fx.pre_disposition_snapshot.to_str().expect("utf8"),
        ],
    );
    assert!(
        !out.status.success(),
        "clean host with copy-dir mirror must refuse pre-disposition snapshot"
    );
    assert!(!clean_data.database().exists());

    // 2b. Clean host + --disposition-ledger refuses pre-disposition snapshot.
    let out = run_cli(
        &clean_data,
        &[
            "restore",
            "--disposition-ledger",
            exported_ledger.to_str().expect("utf8"),
            fx.pre_disposition_snapshot.to_str().expect("utf8"),
        ],
    );
    assert!(
        !out.status.success(),
        "clean host with explicit ledger must refuse pre-disposition snapshot"
    );
    assert!(!clean_data.database().exists());

    // 2c. Clean host + --disposition-ledger accepts post-disposition snapshot
    // and persists data/instance/disposition-ledger.json on the recovered host.
    let out = run_cli(
        &clean_data,
        &[
            "restore",
            "--disposition-ledger",
            exported_ledger.to_str().expect("utf8"),
            fx.post_disposition_snapshot.to_str().expect("utf8"),
        ],
    );
    assert!(
        out.status.success(),
        "post-disposition snapshot must restore cleanly: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("disposition recovery ledger checked"),
        "{stderr}"
    );
    assert!(clean_data.disposition_ledger().is_file());

    // Verify retained record survived and disposed record stayed absent.
    let pool = storage::open_existing(&clean_data.database())
        .await
        .expect("open restored");
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM evaluation_record ORDER BY id")
        .fetch_all(&pool)
        .await
        .expect("ids");
    assert_eq!(ids, vec![fx.retained_record_id]);
    pool.close().await;

    // 2d. Because disposition evidence survived onto the clean host, a later
    // restore of the pre-disposition snapshot WITHOUT passing --disposition-ledger
    // still fails closed!
    let out = run_cli(
        &clean_data,
        &[
            "restore",
            fx.pre_disposition_snapshot.to_str().expect("utf8"),
        ],
    );
    assert!(
        !out.status.success(),
        "recovered host must remember prior dispositions locally"
    );

    // 3. Clean host with NO ledger anywhere restores honestly with an explicit
    // warning (never claiming resurrection protection it could not check).
    let no_ledger_tmp = tempfile::tempdir().expect("no ledger host");
    let no_ledger_data = DataDir::new(no_ledger_tmp.path().join("data"));
    no_ledger_data.ensure_layout().expect("layout");
    let out = run_cli(
        &no_ledger_data,
        &[
            "restore",
            fx.post_disposition_snapshot.to_str().expect("utf8"),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: no disposition recovery ledger was available"),
        "{stderr}"
    );
    // Restoring a post-disposition snapshot whose DB holds disposition_event
    // rows ALSO rebuilds data/instance/disposition-ledger.json from the DB!
    assert!(no_ledger_data.disposition_ledger().is_file());
    let rebuilt = disposition_recovery::read_required_ledger(&no_ledger_data.disposition_ledger())
        .expect("rebuilt ledger");
    assert_eq!(rebuilt.entries.len(), 1);
    assert_eq!(
        rebuilt.entries[0].evaluation_record_id,
        fx.disposed_record_id
    );
}

#[tokio::test]
async fn content_hash_only_ledger_match_also_blocks_resurrection() {
    let fx = DrillInstallation::new().await;
    let clean_tmp = tempfile::tempdir().expect("clean host");
    let clean_data = DataDir::new(clean_tmp.path().join("data"));
    clean_data.ensure_layout().expect("layout");

    // Ledger references a different record_id (9999) but carries the real
    // content_hash of disposed_record_id; restore must STILL refuse!
    let hash_only_ledger = DispositionLedger::new(
        &fx.installation_id,
        fx.disposed_at,
        vec![DispositionLedgerEntry {
            disposition_run_id: 1,
            evaluation_record_id: 9999,
            content_hashes: vec![fx.disposed_content_hash.clone()],
            disposed_at: fx.disposed_at,
            policy_authority: "INVENTED-CEDAR-DAILY-2026".into(),
        }],
    );
    let ledger_path = clean_tmp.path().join("hash-only-ledger.json");
    disposition_recovery::write_ledger_atomic(&ledger_path, &hash_only_ledger).expect("write");

    let err = restore::run_with_options(
        &clean_data,
        &fx.pre_disposition_snapshot,
        &RestoreOptions {
            backup_copy_dir: None,
            disposition_ledger: Some(&ledger_path),
        },
    )
    .await
    .expect_err("content hash match must refuse");
    assert!(
        format!("{err:#}").contains("matched content hashes: 1"),
        "{err:#}"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn missing_stale_corrupt_and_unreachable_recovery_evidence_fail_closed() {
    let fx = DrillInstallation::new().await;

    // 1. Missing explicit --disposition-ledger fails closed.
    let missing_ledger = fx.tmp.path().join("does-not-exist.json");
    let err = restore::run_with_options(
        &fx.data_dir,
        &fx.post_disposition_snapshot,
        &RestoreOptions {
            backup_copy_dir: None,
            disposition_ledger: Some(&missing_ledger),
        },
    )
    .await
    .expect_err("missing explicit ledger");
    assert!(
        format!("{err:#}").contains("reading disposition ledger"),
        "{err:#}"
    );

    // 2. Unreachable copy directory (missing or unmounted same-fs mount point)
    // fails closed during restore and warns read-only in doctor.
    let missing_share = fx.other_fs.path().join("unmounted-missing");
    let err = restore::run_with_options(
        &fx.data_dir,
        &fx.post_disposition_snapshot,
        &RestoreOptions {
            backup_copy_dir: Some(&missing_share),
            disposition_ledger: None,
        },
    )
    .await
    .expect_err("missing share must fail closed");
    assert!(format!("{err:#}").contains("unavailable"), "{err:#}");
    assert!(!missing_share.exists());

    let same_fs_mount = fx.tmp.path().join("unmounted-local-mountpoint");
    std::fs::create_dir_all(&same_fs_mount).expect("mountpoint");
    let err = restore::run_with_options(
        &fx.data_dir,
        &fx.post_disposition_snapshot,
        &RestoreOptions {
            backup_copy_dir: Some(&same_fs_mount),
            disposition_ledger: None,
        },
    )
    .await
    .expect_err("same-fs unmounted share must fail closed");
    assert!(format!("{err:#}").contains("same filesystem"), "{err:#}");

    let findings = doctor::run(
        &fx.data_dir,
        &doctor::Options {
            backup_copy_dir: Some(&missing_share),
            ..doctor::Options::default()
        },
    )
    .await;
    let rec = findings
        .iter()
        .find(|f| f.check == "disposition recovery")
        .expect("finding");
    assert_eq!(rec.verdict, doctor::Verdict::Warn, "{rec:?}");
    assert!(!missing_share.exists(), "doctor must not create copy dir");

    // 3. Stale/missing local ledger when live DB has disposition_event rows
    // fails closed in restore, catch_up, and doctor.
    let saved_ledger = std::fs::read(fx.data_dir.disposition_ledger()).expect("save");
    std::fs::remove_file(fx.data_dir.disposition_ledger()).expect("remove local ledger");

    let err = restore::run(&fx.data_dir, &fx.post_disposition_snapshot)
        .await
        .expect_err("missing local ledger with live DB dispositions must fail closed");
    assert!(format!("{err:#}").contains("missing or stale"), "{err:#}");

    let err = backup_copy::catch_up(&fx.data_dir, &fx.copy_dir, 14)
        .await
        .expect_err("catch_up must fail closed on missing local ledger");
    assert!(format!("{err:#}").contains("missing or stale"), "{err:#}");

    let findings = doctor::run(&fx.data_dir, &doctor::Options::default()).await;
    assert!(doctor::has_failure(&findings), "{findings:?}");

    // 4. Corrupt local ledger fails closed in restore, backup, catch_up, and doctor.
    std::fs::write(fx.data_dir.disposition_ledger(), b"{corrupt").expect("corrupt");
    assert!(
        restore::run(&fx.data_dir, &fx.post_disposition_snapshot)
            .await
            .is_err()
    );
    assert!(backup::run(&fx.data_dir, 14).await.is_err());
    assert!(
        backup_copy::catch_up(&fx.data_dir, &fx.copy_dir, 14)
            .await
            .is_err()
    );
    let findings = doctor::run(&fx.data_dir, &doctor::Options::default()).await;
    assert!(doctor::has_failure(&findings), "{findings:?}");

    // 5. Wrong-installation ledger fails closed.
    let foreign_ledger =
        DispositionLedger::new("different-installation-id", fx.disposed_at, vec![]);
    disposition_recovery::write_ledger_atomic(&fx.data_dir.disposition_ledger(), &foreign_ledger)
        .expect("write foreign");
    let err = restore::run(&fx.data_dir, &fx.post_disposition_snapshot)
        .await
        .expect_err("wrong installation must fail closed");
    assert!(
        format!("{err:#}").contains("belongs to installation"),
        "{err:#}"
    );

    // Restore valid local ledger and verify recovery returns to healthy.
    std::fs::write(fx.data_dir.disposition_ledger(), saved_ledger).expect("restore ledger");
    let report = restore::run_with_options(
        &fx.data_dir,
        &fx.post_disposition_snapshot,
        &RestoreOptions {
            backup_copy_dir: Some(&fx.copy_dir),
            disposition_ledger: None,
        },
    )
    .await
    .expect("valid post-disposition restore");
    assert!(matches!(
        report.ledger_check,
        RestoreLedgerReport::Checked {
            entries_checked: 1,
            ..
        }
    ));
}

#[tokio::test]
async fn catch_up_and_copy_snapshot_cannot_republish_forbidden_disposed_content() {
    let fx = DrillInstallation::new().await;

    // Place a pre-disposition snapshot with a future timestamp in backups/
    // so it appears as the newest local snapshot (e.g., interrupted cleanup or
    // mistaken file placement).
    let forbidden_local = fx
        .data_dir
        .backups()
        .join("consolebook-20990101T000000Z.db");
    std::fs::copy(&fx.pre_disposition_snapshot, &forbidden_local).expect("plant pre-disp snap");

    let share_before = list_names(&fx.copy_dir);

    // Neither catch_up nor copy_snapshot nor copy_snapshot_with_data_dir may
    // publish the forbidden snapshot off the server.
    let err = backup_copy::catch_up(&fx.data_dir, &fx.copy_dir, 14)
        .await
        .expect_err("catch_up must refuse pre-disposition snapshot");
    assert!(format!("{err:#}").contains("lawfully disposed"), "{err:#}");

    let err = backup_copy::copy_snapshot(&forbidden_local, &fx.copy_dir, 14)
        .await
        .expect_err("copy_snapshot must refuse pre-disposition snapshot");
    assert!(format!("{err:#}").contains("lawfully disposed"), "{err:#}");

    let err =
        backup_copy::copy_snapshot_with_data_dir(&fx.data_dir, &forbidden_local, &fx.copy_dir, 14)
            .await
            .expect_err("copy_snapshot_with_data_dir must refuse pre-disposition snapshot");
    assert!(format!("{err:#}").contains("lawfully disposed"), "{err:#}");

    // Nothing was written into the copy directory.
    assert_eq!(list_names(&fx.copy_dir), share_before);

    // Pre-migration snapshot carrying disposed content is equally refused.
    let forbidden_premig = fx
        .data_dir
        .backups()
        .join("consolebook-premigration-20990102T000000Z.db");
    std::fs::copy(&fx.pre_disposition_snapshot, &forbidden_premig).expect("plant premig");
    assert!(
        backup_copy::copy_snapshot_with_data_dir(&fx.data_dir, &forbidden_premig, &fx.copy_dir, 14)
            .await
            .is_err()
    );
    assert_eq!(list_names(&fx.copy_dir), share_before);

    // Remove the planted forbidden snapshots, remove the copy mirror, and
    // verify catch_up heals the off-server ledger mirror when the newest
    // snapshot is clean.
    std::fs::remove_file(&forbidden_local).expect("clean");
    std::fs::remove_file(&forbidden_premig).expect("clean");
    std::fs::remove_file(fx.copy_dir.join(DISPOSITION_LEDGER_FILENAME)).expect("remove mirror");

    assert!(
        backup_copy::catch_up(&fx.data_dir, &fx.copy_dir, 14)
            .await
            .expect("catch_up heals mirror")
            .is_none()
    );
    assert!(fx.copy_dir.join(DISPOSITION_LEDGER_FILENAME).is_file());

    // A copy directory holding ONLY another installation's recovery ledger is
    // still owned by that installation and refuses a replacement server's copy.
    let foreign_tmp = tempfile::tempdir().expect("foreign");
    let foreign_data = DataDir::new(foreign_tmp.path().join("data"));
    foreign_data.ensure_layout().expect("layout");
    storage::open(&foreign_data.database())
        .await
        .expect("open")
        .close()
        .await;
    let foreign_snap = backup::run(&foreign_data, 14).await.expect("snap").snapshot;
    let ledger_only_fs = other_filesystem(foreign_tmp.path());
    let ledger_only_dir = ledger_only_fs.path().join("ledger-only-share");
    std::fs::create_dir_all(&ledger_only_dir).expect("dir");
    std::fs::copy(
        fx.data_dir.disposition_ledger(),
        ledger_only_dir.join(DISPOSITION_LEDGER_FILENAME),
    )
    .expect("seed ledger");

    let err = backup_copy::copy_snapshot_with_data_dir(
        &foreign_data,
        &foreign_snap,
        &ledger_only_dir,
        14,
    )
    .await
    .expect_err("foreign ledger-only copy dir must be refused");
    assert!(format!("{err:#}").contains(&fx.installation_id), "{err:#}");
}

#[tokio::test]
async fn externally_encrypted_copy_recovery_follows_adr_0027_and_enforces_disposition_ledger() {
    let fx = DrillInstallation::new().await;
    let rec_tmp = tempfile::tempdir().expect("encrypted recovery");
    let clean_data = DataDir::new(rec_tmp.path().join("data"));
    clean_data.ensure_layout().expect("layout");

    let ledger_copy = rec_tmp.path().join("disposition-ledger.json");
    std::fs::copy(fx.data_dir.disposition_ledger(), &ledger_copy).expect("ledger");

    // 1. Externally encrypted pre-disposition and post-disposition copies.
    let enc_pre = rec_tmp.path().join("consolebook-pre.db.enc");
    let enc_post = rec_tmp.path().join("consolebook-post.db.enc");
    std::fs::write(
        &enc_pre,
        simulate_external_encrypt(&std::fs::read(&fx.pre_disposition_snapshot).expect("pre")),
    )
    .expect("write enc pre");
    std::fs::write(
        &enc_post,
        simulate_external_encrypt(&std::fs::read(&fx.post_disposition_snapshot).expect("post")),
    )
    .expect("write enc post");

    // 2. Passing the still-encrypted copy directly to `consolebook restore`
    // fails closed with ADR 0027 guidance and changes nothing.
    let out = run_cli(
        &clean_data,
        &[
            "restore",
            "--disposition-ledger",
            ledger_copy.to_str().expect("utf8"),
            enc_post.to_str().expect("utf8"),
        ],
    );
    assert!(!out.status.success());
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("external encryption") && combined.contains("ADR 0027"),
        "{combined}"
    );
    assert!(!clean_data.database().exists());

    // 3. Decrypt the pre-disposition copy at the storage/operator layer:
    // restore STILL refuses it because the ledger records disposed_record_id.
    let dec_pre = rec_tmp.path().join("consolebook-pre-decrypted.db");
    std::fs::write(
        &dec_pre,
        simulate_external_decrypt(&std::fs::read(&enc_pre).expect("read enc pre")),
    )
    .expect("write dec pre");
    let out = run_cli(
        &clean_data,
        &[
            "restore",
            "--disposition-ledger",
            ledger_copy.to_str().expect("utf8"),
            dec_pre.to_str().expect("utf8"),
        ],
    );
    assert!(
        !out.status.success(),
        "decrypted pre-disposition copy must still be blocked by recovery ledger"
    );
    assert!(!clean_data.database().exists());

    // 4. Decrypt the post-disposition copy at the storage/operator layer:
    // restore succeeds, verifies integrity, preserves the local recovery ledger,
    // and creates a validated restore-created backup.
    let dec_post = rec_tmp.path().join("consolebook-post-decrypted.db");
    std::fs::write(
        &dec_post,
        simulate_external_decrypt(&std::fs::read(&enc_post).expect("read enc post")),
    )
    .expect("write dec post");
    let out = run_cli(
        &clean_data,
        &[
            "restore",
            "--disposition-ledger",
            ledger_copy.to_str().expect("utf8"),
            dec_post.to_str().expect("utf8"),
        ],
    );
    assert!(
        out.status.success(),
        "decrypted post-disposition copy must restore cleanly: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(clean_data.disposition_ledger().is_file());
    assert_eq!(
        backup::own_backups_in(&clean_data.backups())
            .expect("list")
            .len(),
        1
    );
}
