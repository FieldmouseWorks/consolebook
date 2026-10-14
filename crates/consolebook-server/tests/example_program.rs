//! The guided setup offers an example program (web/src/lib/setup/
//! example-program.json) as an editable starting draft. The template must
//! stay importable and publishable as the format and publishing rules evolve,
//! so this test fails the build when it would fail an administrator.
//! All content is invented.

use std::path::Path;

use consolebook_server::data_dir::DataDir;
use consolebook_server::program_export::{self, ImportRefusal, ImportTarget};
use consolebook_server::{programs, setup, storage};

const PASSWORD: &str = "invented-passphrase-1";

fn template() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/lib/setup/example-program.json");
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()))
}

async fn installation() -> (tempfile::TempDir, sqlx::SqlitePool, i64) {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let data_dir = DataDir::new(tmp.path().join("data"));
    data_dir.ensure_layout().expect("create layout");
    let pool = storage::open(&data_dir.database()).await.expect("open");
    let code = setup::issue_setup_code(&pool)
        .await
        .expect("issue")
        .expect("uninitialized")
        .0;
    let admin_id = setup::initialize(
        &pool,
        &code.raw,
        "Example County Communications",
        "avery.admin",
        "Avery Admin",
        PASSWORD,
    )
    .await
    .expect("initialize")
    .expect("accepted");
    (tmp, pool, admin_id)
}

#[tokio::test]
async fn the_example_program_imports_as_a_draft_and_publishes_unchanged() {
    let (_tmp, pool, admin) = installation().await;
    let version_id =
        program_export::import_version(&pool, admin, &template(), ImportTarget::NewProgram)
            .await
            .expect("import")
            .expect("the template is a valid program-version document");
    assert_eq!(
        programs::publish_version(&pool, admin, version_id)
            .await
            .expect("publish"),
        Ok(()),
        "the template publishes without edits"
    );
}

#[tokio::test]
async fn the_example_program_is_offered_once_per_name() {
    let (_tmp, pool, admin) = installation().await;
    program_export::import_version(&pool, admin, &template(), ImportTarget::NewProgram)
        .await
        .expect("import")
        .expect("first import");
    assert_eq!(
        program_export::import_version(&pool, admin, &template(), ImportTarget::NewProgram)
            .await
            .expect("import"),
        Err(ImportRefusal::ProgramNameTaken)
    );
}
