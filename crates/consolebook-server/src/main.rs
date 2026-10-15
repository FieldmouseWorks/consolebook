//! Command-line entry point for the Consolebook server.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use consolebook_server::backup_copy::{self, BackupSettings};
use consolebook_server::data_dir::DataDir;
use consolebook_server::doctor::Verdict;
use consolebook_server::public_url::PublicUrl;
use consolebook_server::serve_lock::ServeLock;
use consolebook_server::users::{IssueRefusal, ResetOrigin};
use consolebook_server::{
    BUILD_COMMIT, BUILD_IDENTITY, VERSION, backup, doctor, export_verify, http, restore, scheduler,
    sessions, setup, storage, users,
};

#[derive(Parser)]
#[command(name = "consolebook", version = BUILD_IDENTITY, about = "Training-record system for emergency communications centers (pre-alpha)")]
struct Cli {
    /// Installation data directory (one directory per agency installation).
    #[arg(
        long,
        global = true,
        env = "CONSOLEBOOK_DATA_DIR",
        default_value = "data"
    )]
    data_dir: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize the data directory if needed and serve the HTTP API.
    Serve {
        /// Address to bind. Local by default; put a reverse proxy in front
        /// for external TLS if you expose it.
        #[arg(long, env = "CONSOLEBOOK_BIND", default_value = "127.0.0.1:7770")]
        bind: SocketAddr,
        /// Hours between automatic validated backups.
        #[arg(
            long,
            env = "CONSOLEBOOK_BACKUP_INTERVAL_HOURS",
            default_value_t = scheduler::DEFAULT_INTERVAL_HOURS,
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        backup_interval_hours: u64,
        /// Snapshots retention keeps in backups/.
        #[arg(
            long,
            env = "CONSOLEBOOK_BACKUP_KEEP",
            default_value_t = backup::DEFAULT_KEEP as u64,
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        backup_keep: u64,
        /// Also copy each validated backup here: a second disk or a mounted
        /// network share, so backups survive the loss of this one (ADR 0024).
        #[arg(long, env = "CONSOLEBOOK_BACKUP_COPY_DIR")]
        backup_copy_dir: Option<PathBuf>,
        /// The address people use, such as `https://consolebook.example.org`
        /// behind a TLS proxy. An `https://` address marks session cookies
        /// Secure (ADR 0025).
        #[arg(long, env = "CONSOLEBOOK_PUBLIC_URL")]
        public_url: Option<PublicUrl>,
    },
    /// Diagnose an existing installation without modifying it.
    Doctor {
        /// The backup copy directory the service uses, to check that
        /// copies are current (ADR 0024). Read only.
        #[arg(long, env = "CONSOLEBOOK_BACKUP_COPY_DIR")]
        backup_copy_dir: Option<PathBuf>,
        /// Hours between the service's automatic backups, to judge whether
        /// the newest is overdue.
        #[arg(
            long,
            env = "CONSOLEBOOK_BACKUP_INTERVAL_HOURS",
            default_value_t = scheduler::DEFAULT_INTERVAL_HOURS,
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        backup_interval_hours: u64,
    },
    /// Take a validated, consistent snapshot of the database into backups/.
    Backup {
        /// Snapshots retention keeps in backups/.
        #[arg(
            long,
            env = "CONSOLEBOOK_BACKUP_KEEP",
            default_value_t = backup::DEFAULT_KEEP as u64,
            value_parser = clap::value_parser!(u64).range(1..)
        )]
        keep: u64,
        /// Also copy the new backup here (ADR 0024).
        #[arg(long, env = "CONSOLEBOOK_BACKUP_COPY_DIR")]
        backup_copy_dir: Option<PathBuf>,
    },
    /// Replace the database with a validated snapshot (server must be stopped).
    ///
    /// The current database is set aside as a pre-restore snapshot in
    /// backups/ before anything is replaced.
    Restore {
        /// Path to the snapshot to restore, usually in backups/.
        snapshot: PathBuf,
    },
    /// Work with record exports (docs/formats/record-export.md).
    Export {
        #[command(subcommand)]
        action: ExportAction,
    },
    /// Check a public URL as `serve` would and print its origin. The
    /// installer runs it before changing anything, so a value the server
    /// would refuse never stops a working service.
    #[command(hide = true)]
    CheckPublicUrl { url: PublicUrl },
    /// Print a fresh first-run setup code for an uninitialized installation.
    SetupCode,
    /// Issue a password reset code for a locked-out administrator.
    ///
    /// Requires operating-system access to the data directory and records
    /// an explicit recovery audit event.
    Recover {
        /// Username of the administrator account to recover.
        #[arg(long)]
        username: String,
    },
}

#[derive(Subcommand)]
enum ExportAction {
    /// Verify a record export archive from the file alone.
    ///
    /// Opens no data directory: the archive carries everything the
    /// checks need, and the verdict is consistency with its stated
    /// fingerprints.
    Verify {
        /// Path to the export archive (.zip).
        archive: PathBuf,
    },
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    // Structured logs, no record content: log events describe operations and
    // outcomes, never evaluation narratives or personal data.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}

#[tokio::main]
async fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Serve {
            bind,
            backup_interval_hours,
            backup_keep,
            backup_copy_dir,
            public_url,
        } => {
            let settings = BackupSettings {
                data_dir: DataDir::new(&cli.data_dir),
                interval_hours: backup_interval_hours,
                keep: usize::try_from(backup_keep).unwrap_or(usize::MAX),
                copy_dir: backup_copy_dir,
            };
            serve(bind, settings, public_url).await
        }
        Command::Doctor {
            backup_copy_dir,
            backup_interval_hours,
        } => {
            let options = doctor::Options {
                backup_copy_dir: backup_copy_dir.as_deref(),
                backup_interval_hours,
            };
            run_doctor(&cli.data_dir, &options).await
        }
        Command::Backup {
            keep,
            backup_copy_dir,
        } => run_backup(&cli.data_dir, keep, backup_copy_dir.as_deref()).await,
        Command::Restore { snapshot } => run_restore(&cli.data_dir, &snapshot).await,
        Command::Export {
            action: ExportAction::Verify { archive },
        } => run_export_verify(&archive),
        Command::SetupCode => run_setup_code(&cli.data_dir).await,
        Command::CheckPublicUrl { url } => {
            println!("{url}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Recover { username } => run_recover(&cli.data_dir, &username).await,
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            tracing::error!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(
    bind: SocketAddr,
    settings: BackupSettings,
    public_url: Option<PublicUrl>,
) -> Result<ExitCode> {
    let data_dir = settings.data_dir.clone();
    data_dir.ensure_layout()?;
    let lock = ServeLock::acquire(&data_dir)?;
    tracing::debug!(lock = %lock.path().display(), "holding installation lock");
    let pool = backup::open_migrated(
        &data_dir,
        Some(&lock),
        settings
            .copy_dir
            .as_deref()
            .map(|dir| backup::OffServerCopy {
                dir,
                keep: settings.keep,
            }),
    )
    .await?;
    let installation_id = storage::installation_id(&pool).await?;
    tracing::info!(
        version = VERSION,
        commit = BUILD_COMMIT,
        installation_id,
        data_dir = %data_dir.root().display(),
        "starting Consolebook (pre-alpha)"
    );

    if !consolebook_server::web_assets::embedded() {
        tracing::warn!(
            "this build does not embed the web interface; build web/ (npm ci && npm run build) before cargo for a complete executable"
        );
    }
    if let Some((code, expires_at)) = setup::issue_setup_code(&pool).await? {
        // The one deliberate secret in the log: the operator needs it to
        // complete first-run setup, and it is useless once setup completes
        // or the code expires. It names the address people use: setup over
        // plain HTTP would end at a sign-in whose Secure cookie never sticks.
        let address = public_url
            .as_ref()
            .map_or_else(|| format!("http://{bind}"), |url| url.origin().to_owned());
        tracing::warn!(
            setup_code = code.raw,
            expires_in_minutes = (expires_at - now_unix()) / 60,
            "installation is not initialized; open {address}/ in a web browser and enter this setup code"
        );
    }

    if let Some(dir) = &settings.copy_dir {
        tracing::info!(copy_dir = %dir.display(), "backups are copied off the server");
    } else {
        tracing::warn!(
            "no backup copy directory: every backup shares a disk with the database; \
             set CONSOLEBOOK_BACKUP_COPY_DIR (docs/install.md)"
        );
    }
    match &public_url {
        Some(url) if url.is_secure() => {
            tracing::info!(public_url = %url, "session cookies are marked Secure");
            let revoked = sessions::revoke_not_secure(&pool).await?;
            if revoked > 0 {
                tracing::info!(
                    revoked,
                    "signed out sessions whose cookies were issued without Secure; \
                     people sign in again at the public address"
                );
            }
        }
        _ if !bind.ip().is_loopback() => tracing::warn!(
            "serving plain HTTP on the network: session cookies are not Secure; put HTTPS \
             in front and set CONSOLEBOOK_PUBLIC_URL to its https:// address (docs/install.md)"
        ),
        _ => {}
    }
    let scheduler = tokio::spawn(scheduler::run(settings.clone(), pool.clone()));

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(addr = %listener.local_addr()?, "listening");
    let mut state = http::AppState::new(pool).with_backups(settings);
    if let Some(url) = public_url {
        state = state.with_public_url(url);
    }
    http::serve(listener, state).await?;
    scheduler.abort();
    drop(lock);
    tracing::info!("stopped");
    Ok(ExitCode::SUCCESS)
}

async fn run_doctor(data_dir: &std::path::Path, options: &doctor::Options<'_>) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    let findings = doctor::run(&data_dir, options).await;
    for finding in &findings {
        let mark = match finding.verdict {
            Verdict::Ok => "ok  ",
            Verdict::Warn => "warn",
            Verdict::Fail => "FAIL",
        };
        println!("{mark}  {:<22} {}", finding.check, finding.detail);
    }
    if doctor::has_failure(&findings) {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

async fn run_setup_code(data_dir: &std::path::Path) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    let pool = backup::open_migrated(&data_dir, None, None).await?;
    match setup::issue_setup_code(&pool).await? {
        None => {
            eprintln!("this installation is already initialized; setup is unavailable");
            Ok(ExitCode::FAILURE)
        }
        Some((code, expires_at)) => {
            println!("{}", code.raw);
            eprintln!(
                "setup code valid for {} minutes; enter it on the setup page in a web browser",
                (expires_at - now_unix()) / 60
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn run_recover(data_dir: &std::path::Path, username: &str) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    let pool = backup::open_migrated(&data_dir, None, None).await?;
    match users::issue_reset_code(&pool, username, ResetOrigin::Recovery).await? {
        Ok(issued) => {
            println!("{}", issued.code.raw);
            eprintln!(
                "reset code for {} valid for {} minutes; use POST /api/auth/reset with a new password. All sessions revoke when it is used.",
                issued.user.username,
                (issued.expires_at - now_unix()) / 60
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(IssueRefusal::NoSuchUser) => {
            eprintln!("no user named {username}");
            Ok(ExitCode::FAILURE)
        }
        Err(IssueRefusal::NotAnAdministrator) => {
            eprintln!(
                "{username} is not an administrator; ask an administrator to issue a reset code instead"
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

async fn run_backup(
    data_dir: &std::path::Path,
    keep: u64,
    copy_dir: Option<&std::path::Path>,
) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    data_dir.ensure_layout()?;
    let keep = usize::try_from(keep).unwrap_or(usize::MAX);
    let report = backup::run(&data_dir, keep).await?;
    tracing::info!(
        snapshot = %report.snapshot.display(),
        size_bytes = report.size_bytes,
        pruned = report.pruned.len(),
        "backup complete and validated"
    );
    println!("{}", report.snapshot.display());
    if let Some(copy_dir) = copy_dir {
        let copy = backup_copy::copy_snapshot(&report.snapshot, copy_dir, keep).await?;
        tracing::info!(copy = %copy.copy.display(), sha256 = copy.sha256, "backup copied and verified");
        println!("{}", copy.copy.display());
    }
    Ok(ExitCode::SUCCESS)
}

async fn run_restore(data_dir: &std::path::Path, snapshot: &std::path::Path) -> Result<ExitCode> {
    let data_dir = DataDir::new(data_dir);
    data_dir.ensure_layout()?;
    let report = restore::run(&data_dir, snapshot).await?;
    if let Some(pre) = &report.pre_restore_snapshot {
        eprintln!("previous database set aside as {}", pre.display());
    }
    eprintln!(
        "restored {} (installation {})",
        report.restored_from.display(),
        report.installation_id
    );
    if let Some(backup) = &report.backup {
        eprintln!(
            "backed up the restored database as {}; it is the copy the service sends off the server next",
            backup.display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn run_export_verify(archive: &std::path::Path) -> Result<ExitCode> {
    let bytes = std::fs::read(archive).with_context(|| format!("reading {}", archive.display()))?;
    let report = export_verify::verify_archive(&bytes);
    if let Some(kind) = report.kind {
        println!("kind          {kind}");
    }
    if let Some(id) = &report.installation_id {
        println!("installation  {id}");
    }
    if let Some(id) = report.enrollment_id {
        println!("enrollment    {id}");
    }
    if let Some(at) = report.exported_at_rfc3339() {
        println!("exported at   {at}");
    }
    if let Some(scope) = report.scope {
        println!("scope         {scope}");
    }
    for finding in &report.findings {
        println!("FAIL  archive  {finding}");
    }
    for unit in &report.units {
        let mark = if unit.verified() { "ok  " } else { "FAIL" };
        println!(
            "{mark}  {:<20} record {} version {} (schema {}); predecessor {}",
            unit.path, unit.record_id, unit.version_number, unit.record_schema, unit.predecessor
        );
        for finding in &unit.findings {
            println!("      {finding}");
        }
    }
    for document in &report.documents {
        let mark = if document.verified() { "ok  " } else { "FAIL" };
        println!(
            "{mark}  {:<28} {} document",
            document.path,
            document.kind.as_str()
        );
        for finding in &document.findings {
            println!("      {finding}");
        }
    }
    for table in &report.tables {
        let mark = if table.verified() { "ok  " } else { "FAIL" };
        let rows = table
            .rows
            .map_or_else(|| "unreadable".to_owned(), |rows| format!("{rows} row(s)"));
        println!("{mark}  {:<36} {} table, {rows}", table.path, table.table);
        for finding in &table.findings {
            println!("      {finding}");
        }
    }
    let consistent = report.units.iter().filter(|unit| unit.verified()).count();
    let documents = if report.documents.is_empty() && report.tables.is_empty() {
        String::new()
    } else {
        format!(
            " and {} of {} documents",
            report
                .documents
                .iter()
                .filter(|document| document.verified())
                .count()
                + report
                    .tables
                    .iter()
                    .filter(|table| table.verified())
                    .count(),
            report.documents.len() + report.tables.len()
        )
    };
    let what = match report.kind {
        Some(export_verify::ArchiveKind::TraineePacket) => "packet",
        Some(export_verify::ArchiveKind::InstallationExport) => "installation export",
        _ => "export",
    };
    if report.verified() {
        println!(
            "verified {consistent} of {} units{documents}: the {what} is consistent with its stated fingerprints",
            report.units.len()
        );
        Ok(ExitCode::SUCCESS)
    } else {
        println!(
            "NOT VERIFIED: {consistent} of {} units consistent{documents}, {} archive finding(s)",
            report.units.len(),
            report.findings.len()
        );
        Ok(ExitCode::FAILURE)
    }
}
