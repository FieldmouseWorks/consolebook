use super::*;
use consolebook_server::program_export::{self, ImportRefusal, ImportTarget};
use programs::{AuthorRefusal, ProgramRefusal, PublishRefusal};

#[tokio::test]
async fn duplicate_program_name_has_one_winner() {
    let fx = Fixture::new().await;
    one_winner(
        contend(
            &fx.pool,
            programs::create_program(&fx.pool, ACTOR, "Invented Program"),
            programs::create_program(&fx.pool, ACTOR, "INVENTED PROGRAM"),
        )
        .await,
        &ProgramRefusal::NameTaken,
    );
    assert_eq!(
        programs::list_programs(&fx.pool)
            .await
            .expect("programs")
            .len(),
        1
    );
    fx.probe().await;
}

#[tokio::test]
async fn version_creation_assigns_distinct_monotonic_numbers() {
    let fx = Fixture::new().await;
    let (program, _) = fx.draft().await;
    let content = content();
    let (left, right) = contend(
        &fx.pool,
        programs::create_version(&fx.pool, ACTOR, program, &content),
        programs::create_version(&fx.pool, ACTOR, program, &content),
    )
    .await;
    assert_ne!(
        left.expect("left").expect("version"),
        right.expect("right").expect("version")
    );
    let numbers: Vec<_> = programs::list_versions(&fx.pool, program)
        .await
        .expect("versions")
        .into_iter()
        .map(|v| v.version_number)
        .collect();
    assert_eq!(numbers, vec![1, 2, 3]);
}

#[tokio::test]
async fn publication_has_one_winner_and_one_audit_event() {
    let fx = Fixture::new().await;
    let (_, version) = fx.draft().await;
    one_winner(
        contend(
            &fx.pool,
            programs::publish_version(&fx.pool, ACTOR, version),
            programs::publish_version(&fx.pool, ACTOR, version),
        )
        .await,
        &PublishRefusal::AlreadyPublished,
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event WHERE kind = 'program_version_published'",
    )
    .fetch_one(&fx.pool)
    .await
    .expect("audit");
    assert_eq!(count, 1);
    fx.probe().await;
}

#[tokio::test]
async fn draft_replacements_remain_whole_last_write_content() {
    let fx = Fixture::new().await;
    let (_, version) = fx.draft().await;
    let mut first = content();
    first.label = "first".into();
    first.phases[0].name = "First phase".into();
    let mut second = content();
    second.label = "second".into();
    second.phases[0].name = "Second phase".into();
    let (left, right) = contend(
        &fx.pool,
        programs::replace_draft(&fx.pool, ACTOR, version, &first),
        programs::replace_draft(&fx.pool, ACTOR, version, &second),
    )
    .await;
    left.expect("left").expect("replace");
    right.expect("right").expect("replace");
    let stored = programs::load_content(&fx.pool, version)
        .await
        .expect("load")
        .expect("content");
    assert!(stored == first || stored == second, "no mixed content");
}

#[tokio::test]
async fn draft_discard_has_one_winner() {
    let fx = Fixture::new().await;
    let (_, version) = fx.draft().await;
    one_winner(
        contend(
            &fx.pool,
            programs::discard_draft(&fx.pool, ACTOR, version),
            programs::discard_draft(&fx.pool, ACTOR, version),
        )
        .await,
        &AuthorRefusal::NoSuchVersion,
    );
    assert!(
        programs::load_content(&fx.pool, version)
            .await
            .expect("load")
            .is_none()
    );
    fx.probe().await;
}

#[tokio::test]
async fn import_serializes_names_and_version_numbers() {
    let fx = Fixture::new().await;
    let (_, version) = fx.draft().await;
    let export = program_export::export_version(&fx.pool, version)
        .await
        .expect("export")
        .expect("exists");
    let doc = export.replace("Invented County Program", "Imported Invented Program");
    let imported = one_winner(
        contend(
            &fx.pool,
            program_export::import_version(&fx.pool, ACTOR, &doc, ImportTarget::NewProgram),
            program_export::import_version(&fx.pool, ACTOR, &doc, ImportTarget::NewProgram),
        )
        .await,
        &ImportRefusal::ProgramNameTaken,
    );
    fx.probe().await;
    let program = programs::version_summary(&fx.pool, imported)
        .await
        .expect("summary")
        .expect("version")
        .program_id;
    let (left, right) = contend(
        &fx.pool,
        program_export::import_version(&fx.pool, ACTOR, &doc, ImportTarget::VersionOf(program)),
        program_export::import_version(&fx.pool, ACTOR, &doc, ImportTarget::VersionOf(program)),
    )
    .await;
    let ids = [
        left.expect("left").expect("import"),
        right.expect("right").expect("import"),
    ];
    assert_ne!(ids[0], ids[1]);
    for id in ids {
        assert_eq!(
            program_export::export_version(&fx.pool, id)
                .await
                .expect("export")
                .expect("exists"),
            doc
        );
    }
    let numbers: Vec<_> = programs::list_versions(&fx.pool, program)
        .await
        .expect("versions")
        .into_iter()
        .map(|v| v.version_number)
        .collect();
    assert_eq!(numbers, vec![1, 2, 3]);
}
