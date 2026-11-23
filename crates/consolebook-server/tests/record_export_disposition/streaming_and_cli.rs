//! Real-server streaming race tests (disposition between manifest preparation
//! and payload delivery) and file-only CLI `export verify` proofs (#215).

use std::io::{Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use consolebook_server::export_verify::{self, ArchiveContentStatus, Finding};
use consolebook_server::record_export::{self, Scope};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

#[derive(Debug)]
struct GateSink {
    buf: Vec<u8>,
    pos: u64,
    hold_at: u64,
    held: bool,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

impl GateSink {
    fn new(hold_at: u64) -> (Self, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, arrived_rx) = mpsc::channel();
        let (release_tx, release) = mpsc::channel();
        (
            Self {
                buf: Vec::new(),
                pos: 0,
                hold_at,
                held: false,
                entered,
                release,
            },
            arrived_rx,
            release_tx,
        )
    }
}

impl Write for GateSink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let end = usize::try_from(self.pos).expect("pos") + data.len();
        if end > self.buf.len() {
            self.buf.resize(end, 0);
        }
        let start = usize::try_from(self.pos).expect("pos");
        self.buf[start..end].copy_from_slice(data);
        self.pos += data.len() as u64;
        if !self.held && self.pos >= self.hold_at {
            self.held = true;
            let _ = self.entered.send(());
            let _ = self.release.recv_timeout(Duration::from_secs(30));
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for GateSink {
    fn seek(&mut self, style: SeekFrom) -> std::io::Result<u64> {
        let next: i128 = match style {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(n) => i128::from(self.buf.len() as u64) + i128::from(n),
            SeekFrom::Current(n) => i128::from(self.pos) + i128::from(n),
        };
        if next < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "negative seek",
            ));
        }
        self.pos = u64::try_from(next).expect("seek pos");
        Ok(self.pos)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn disposition_racing_between_manifest_and_payload_fails_transfer_visibly() {
    let fx = DispExportFixture::new().await;
    fx.set_destroy_policies().await;
    let (r1, fin1) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-01",
            "Invented first record payload",
            None,
        )
        .await;
    let (r2, fin2) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-02",
            "Invented second record payload disposed mid-stream",
            None,
        )
        .await;
    assert!(r1 < r2);
    let evaluated_at = fin1.max(fin2) + 31 * SECONDS_PER_DAY;

    // Pause the streaming producer right after manifest.json is written, before
    // record r2 is fetched by write_units_streaming.
    let (sink, arrived_rx, release_tx) = GateSink::new(128);
    let pool = fx.pool.clone();
    let actor = fx.admin_id;
    let export_task = tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async move {
            record_export::export_to(
                &pool,
                actor,
                Scope::Installation,
                EXPORTED_AT,
                sink,
                &|| false,
                |_| {},
            )
            .await
        })
    });

    arrived_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("producer reached manifest write");

    // Execute lawful disposition on r2 while the export is paused mid-flight.
    fx.dispose_records(vec![r2], evaluated_at, true).await;
    release_tx.send(()).expect("release export");

    let err = export_task
        .await
        .expect("join")
        .expect_err("export must fail when a manifested version is disposed mid-transfer");
    assert!(
        err.to_string().contains("no longer stored"),
        "expected missing-version race error, got: {err:#}"
    );
}

async fn live_login(addr: &SocketAddr, username: &str) -> String {
    let payload = serde_json::json!({
        "username": username,
        "password": support::password(),
    })
    .to_string();
    let req = format!(
        "POST /api/auth/login HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).await.expect("read");
    let text = String::from_utf8_lossy(&resp);
    let needle = format!("{}=", consolebook_server::http::SESSION_COOKIE);
    for line in text.lines() {
        if line.to_ascii_lowercase().starts_with("set-cookie:")
            && let Some(start) = line.find(&needle)
        {
            let after_cookie = &line[start + needle.len()..];
            return after_cookie
                .split(';')
                .next()
                .expect("cookie val")
                .to_owned();
        }
    }
    panic!("no session cookie in login response:\n{text}");
}

fn split_response(raw: &[u8]) -> (String, &[u8]) {
    let at = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http header separator");
    let headers = String::from_utf8_lossy(&raw[..at]).to_ascii_lowercase();
    (headers, &raw[at + 4..])
}

fn chunked_is_complete(mut body: &[u8]) -> bool {
    loop {
        let Some(crlf) = body.windows(2).position(|w| w == b"\r\n") else {
            return false;
        };
        let size_str = std::str::from_utf8(&body[..crlf])
            .ok()
            .and_then(|s| s.split(';').next())
            .map(str::trim);
        let Some(size) = size_str.and_then(|s| usize::from_str_radix(s, 16).ok()) else {
            return false;
        };
        body = &body[crlf + 2..];
        if size == 0 {
            return body.starts_with(b"\r\n");
        }
        if body.len() < size + 2 || &body[size..size + 2] != b"\r\n" {
            return false;
        }
        body = &body[size + 2..];
    }
}

fn dechunk_lossy(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(crlf) = body.windows(2).position(|w| w == b"\r\n") {
        let Some(size) = std::str::from_utf8(&body[..crlf])
            .ok()
            .and_then(|s| s.split(';').next())
            .and_then(|s| usize::from_str_radix(s.trim(), 16).ok())
        else {
            break;
        };
        body = &body[crlf + 2..];
        if size == 0 {
            break;
        }
        let take = size.min(body.len());
        out.extend_from_slice(&body[..take]);
        if body.len() < size + 2 {
            break;
        }
        body = &body[size + 2..];
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn real_server_http_stream_aborts_on_mid_transfer_disposition_and_serves_coherent_post_disposition_export()
 {
    let fx = DispExportFixture::new().await;
    fx.set_destroy_policies().await;

    // Seed enough large records (about 1 MiB each, 12 MiB in all) that the
    // response far exceeds everything that can hold it in flight before the
    // final record is read: the body channel (8 x 64 KiB), one entry buffer,
    // the server's socket send buffer (at most `net.core.wmem_max`, 4 MiB by
    // default) and the client's receive window, pinned small below so kernel
    // autotuning cannot grow it. The producer is therefore backpressured
    // before it reaches the victim, whatever the timing.
    let big_narrative = "Invented shift observation detail. ".repeat(30_000);
    let mut record_ids = Vec::new();
    let mut latest_fin = 0;
    for day in 1..=12 {
        let (id, fin) = fx
            .create_finalized_daily(
                fx.enrollment_id,
                &format!("2026-06-{day:02}"),
                &big_narrative,
                None,
            )
            .await;
        record_ids.push(id);
        latest_fin = latest_fin.max(fin);
    }
    let victim_id = *record_ids.last().expect("last record");
    let evaluated_at = latest_fin + 31 * SECONDS_PER_DAY;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app =
        consolebook_server::http::router(consolebook_server::http::AppState::new(fx.pool.clone()));
    let serving = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let cookie = live_login(&addr, "avery.admin").await;

    // 1. Start streaming GET /api/exports/records, read only the response
    //    headers (200 OK), then dispose the last record while the server is
    //    backpressured on the unread socket.
    let socket = tokio::net::TcpSocket::new_v4().expect("socket");
    socket
        .set_recv_buffer_size(16 * 1024)
        .expect("small receive window");
    let mut client = socket.connect(addr).await.expect("connect");
    let req = format!(
        "GET /api/exports/records HTTP/1.1\r\nHost: {addr}\r\nCookie: {}={cookie}\r\nConnection: close\r\n\r\n",
        consolebook_server::http::SESSION_COOKIE
    );
    client.write_all(req.as_bytes()).await.expect("write");

    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = client.read(&mut buf).await.expect("read headers");
        assert!(n > 0, "server closed before headers");
        raw.extend_from_slice(&buf[..n]);
        if raw.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let (headers_only, _) = split_response(&raw);
    assert!(
        headers_only.starts_with("http/1.1 200"),
        "expected 200 streaming start: {headers_only}"
    );
    assert!(headers_only.contains("transfer-encoding: chunked"));

    // Dispose the final record while the HTTP transfer is stalled mid-stream.
    // The stall must stay inside the server's stall limit, or the transfer
    // would end for the stall and this test would pass for the wrong reason.
    let stalled_since = std::time::Instant::now();
    fx.dispose_records(vec![victim_id], evaluated_at, true)
        .await;
    assert!(
        stalled_since.elapsed() < consolebook_server::exports_http::EXPORT_STALL_LIMIT / 2,
        "disposition took {:?}: too close to the export stall limit to prove the race",
        stalled_since.elapsed()
    );

    // Drain the rest of the socket: the server must abort the chunked transfer
    // cleanly without a terminal 0-length chunk, and any received bytes must
    // fail file-only archive verification.
    let mut rest = Vec::new();
    let _ = client.read_to_end(&mut rest).await;
    raw.extend_from_slice(&rest);
    let (_, body) = split_response(&raw);
    assert!(
        !chunked_is_complete(body),
        "raced streaming transfer must NOT end with a complete chunked terminator"
    );
    let partial_zip = dechunk_lossy(body);
    let partial_report = export_verify::verify_archive(&partial_zip);
    assert!(
        !partial_report.verified(),
        "truncated raced archive must never verify: {partial_report:?}"
    );
    assert!(
        partial_report
            .findings
            .iter()
            .any(|f| matches!(f, Finding::NotAnArchive { .. })),
        "expected unreadable incomplete ZIP finding, got: {partial_report:?}"
    );

    // 2. A fresh streaming request AFTER disposition completes cleanly with a
    //    coherent mixed retained + disposed v2 archive, and requesting the
    //    disposed record directly over HTTP delivers a coherent FullyDisposed
    //    archive.
    let mut client2 = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect2");
    client2.write_all(req.as_bytes()).await.expect("write2");
    let mut raw2 = Vec::new();
    client2.read_to_end(&mut raw2).await.expect("read2");
    let (headers2, body2) = split_response(&raw2);
    assert!(headers2.starts_with("http/1.1 200"), "{headers2}");
    assert!(chunked_is_complete(body2), "post-disposition stream failed");
    let zip2 = dechunk_lossy(body2);
    let report2 = export_verify::verify_archive(&zip2);
    assert!(report2.verified(), "{report2:?}");
    assert_eq!(
        report2.content_status(),
        ArchiveContentStatus::MixedRetainedAndDisposed
    );
    assert_eq!(report2.units.len(), 11);
    assert_eq!(
        report2
            .disposition
            .as_ref()
            .expect("disposition report")
            .dispositions
            .len(),
        1
    );

    let req_disposed = format!(
        "GET /api/drafts/{victim_id}/export HTTP/1.1\r\nHost: {addr}\r\nCookie: {}={cookie}\r\nConnection: close\r\n\r\n",
        consolebook_server::http::SESSION_COOKIE
    );
    let mut client3 = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect3");
    client3
        .write_all(req_disposed.as_bytes())
        .await
        .expect("write3");
    let mut raw3 = Vec::new();
    client3.read_to_end(&mut raw3).await.expect("read3");
    serving.abort();
    let (headers3, body3) = split_response(&raw3);
    assert!(headers3.starts_with("http/1.1 200"), "{headers3}");
    assert!(chunked_is_complete(body3));
    let zip3 = dechunk_lossy(body3);
    let report3 = export_verify::verify_archive(&zip3);
    assert!(report3.verified(), "{report3:?}");
    assert_eq!(
        report3.content_status(),
        ArchiveContentStatus::FullyDisposed
    );
    assert_eq!(report3.units.len(), 0);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn cli_export_verify_reports_honest_verdicts_without_opening_an_installation() {
    let fx = DispExportFixture::new().await;
    let (_, meta_policy_id) = fx.set_destroy_policies().await;
    let (r_retained, fin1) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-01",
            "Invented retained CLI record",
            None,
        )
        .await;
    let (r_hashed, fin2) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-02",
            "Invented hashed disposed CLI record",
            Some("Invented v2"),
        )
        .await;
    let (r_nohash, fin3) = fx
        .create_finalized_daily(
            fx.enrollment_id,
            "2026-06-03",
            "Invented no-hash disposed CLI record",
            None,
        )
        .await;
    let evaluated_at = fin1.max(fin2).max(fin3) + 31 * SECONDS_PER_DAY;
    fx.dispose_records(vec![r_hashed], evaluated_at, true).await;
    fx.dispose_records(vec![r_nohash], evaluated_at + 10, false)
        .await;
    fx.insert_boundary_checkpoint(meta_policy_id, evaluated_at + 20)
        .await;
    assert!(r_retained > 0);

    let mixed = record_export::export_at(&fx.pool, fx.admin_id, Scope::Installation, EXPORTED_AT)
        .await
        .expect("call")
        .expect("mixed export");
    let fully_disposed = record_export::export_at(
        &fx.pool,
        fx.executor_id,
        Scope::Record {
            record_id: r_hashed,
        },
        EXPORTED_AT,
    )
    .await
    .expect("call")
    .expect("disposed export");

    // Run the CLI verifier from a completely empty temp directory with no
    // database or CONSOLEBOOK_DATA_DIR to prove file-only operation.
    let isolated = tempfile::tempdir().expect("isolated dir");
    let mixed_path = isolated.path().join("mixed.zip");
    let disposed_path = isolated.path().join("disposed.zip");
    let tampered_path = isolated.path().join("tampered.zip");
    std::fs::write(&mixed_path, &mixed.bytes).expect("write mixed");
    std::fs::write(&disposed_path, &fully_disposed.bytes).expect("write disposed");

    let tampered = repack(&with_entry(
        &entries(&mixed.bytes),
        consolebook_server::record_export::DISPOSITION_EVIDENCE_PATH,
        |b| {
            let mut copy = b.to_vec();
            copy.push(b' ');
            Some(copy)
        },
    ));
    std::fs::write(&tampered_path, &tampered).expect("write tampered");

    let run_cli = |path: &std::path::Path| {
        Command::new(server_binary())
            .current_dir(isolated.path())
            .env_remove("CONSOLEBOOK_DATA_DIR")
            .args(["export", "verify"])
            .arg(path)
            .output()
            .expect("run consolebook-server export verify")
    };

    let out_mixed = run_cli(&mixed_path);
    let stdout_mixed = String::from_utf8_lossy(&out_mixed.stdout);
    assert!(
        out_mixed.status.success(),
        "mixed CLI failed: {stdout_mixed}"
    );
    assert!(
        stdout_mixed.contains("hash lineage continuity only; content unavailable for verification"),
        "{stdout_mixed}"
    );
    assert!(
        stdout_mixed.contains("content and version hashes unavailable"),
        "{stdout_mixed}"
    );
    assert!(
        stdout_mixed.contains("intentional policy boundary"),
        "{stdout_mixed}"
    );
    assert!(
        stdout_mixed.contains("self-consistency only"),
        "{stdout_mixed}"
    );

    let out_disposed = run_cli(&disposed_path);
    let stdout_disposed = String::from_utf8_lossy(&out_disposed.stdout);
    assert!(
        out_disposed.status.success(),
        "disposed CLI failed: {stdout_disposed}"
    );
    assert!(
        stdout_disposed.contains("fully disposed scope"),
        "{stdout_disposed}"
    );
    assert!(
        stdout_disposed.contains("0 retained units"),
        "{stdout_disposed}"
    );
    assert!(
        stdout_disposed.contains("content unavailable for verification"),
        "{stdout_disposed}"
    );

    let out_tampered = run_cli(&tampered_path);
    let stdout_tampered = String::from_utf8_lossy(&out_tampered.stdout);
    assert!(
        !out_tampered.status.success(),
        "tampered CLI must exit non-zero"
    );
    assert!(
        stdout_tampered.contains("NOT VERIFIED"),
        "{stdout_tampered}"
    );
    // Confirm the verifier created nothing: no database, no data directory,
    // nothing but the three archives it was handed.
    let mut left: Vec<_> = std::fs::read_dir(isolated.path())
        .expect("read isolated dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    left.sort();
    assert_eq!(left, ["disposed.zip", "mixed.zip", "tampered.zip"]);
}
