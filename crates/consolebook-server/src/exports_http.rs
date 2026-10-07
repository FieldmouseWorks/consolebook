//! Record export HTTP handlers (Milestone 5 slice 1; ADR 0014).
//!
//! `http.rs` remains the hub; this module owns the export downloads and
//! the installation-export summary. Scope rules live in
//! `record_export`; handlers translate refusals into stable error codes
//! and deliver the documented archive bytes as attachments.

use std::io::Write as _;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use http_body::Frame;
use time::OffsetDateTime;
use tokio::sync::{Semaphore, mpsc, oneshot};

use crate::export_stream::EntryBuffer;
use crate::http::{ApiError, AppState, CurrentUser};
use crate::record_export::{self, ExportRefusal, Scope};
use crate::trainee_packet::{self, PacketRefusal};

/// How many chunks may wait between the archive's producer and the
/// response body. The bound is the backpressure: a client that stops
/// reading blocks the producer at this many chunks rather than letting it
/// run ahead, and a client that disconnects closes the channel, which
/// stops the producer. Each chunk holds at most [`BODY_CHUNK_BYTES`], so
/// the queue holds at most 8 × 64 KiB = 512 KiB however large a record is.
const BODY_CHUNKS: usize = 8;
/// The most bytes one queued chunk may hold. The container writer hands
/// the destination a whole entry at a time, and a record can be arbitrarily
/// large, so an entry is split into pieces of this size: the queue's bound
/// is then a bound on bytes, not only on items.
const BODY_CHUNK_BYTES: usize = 64 * 1024;
/// How long one chunk may wait for room in the response queue, and how long
/// the failure detail may wait for room of its own. The queue is full for
/// this long only when the client has stopped reading altogether, so
/// reaching it ends the export: the transfer is ended without its final
/// chunk, which the client reads as an incomplete download rather than as a
/// complete export. A client that keeps making progress — even slowly —
/// never reaches it.
///
/// Public because a transport-level test has to outlast both windows to
/// prove that losing the failure detail still loses the transfer; the unit
/// tests inject their own, much shorter, deadlines instead.
pub const EXPORT_STALL_LIMIT: Duration = Duration::from_secs(10);
/// How long the preflight may take before the request fails. This covers
/// authorization, the metadata pass, and the audit: the time before the
/// response can still carry a typed answer. It deliberately excludes the
/// payload pass, which has no deadline of its own beyond the per-chunk
/// stall guard.
const PREFLIGHT_LIMIT: Duration = Duration::from_secs(30);
/// How many exports one process produces at a time. Each producer is a
/// blocking thread for the length of its download, and a client that reads
/// one chunk per stall window keeps it for as long as it likes, so without
/// a bound an authorized caller's parallel requests grow the blocking pool
/// to its own limit and queue closures behind it. Beyond this many, a
/// request is refused with a typed `export_busy` answer before anything is
/// authorized, audited, or spawned; the refusal is the client's cue to
/// retry, not a failure of the export.
///
/// Public so the transport test can fill every slot.
pub const EXPORT_SLOTS: usize = 4;
/// The slots themselves, one per process: an export holds one from before
/// its producer is spawned until the producer returns.
static SLOTS: Semaphore = Semaphore::const_new(EXPORT_SLOTS);

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/drafts/{id}/export", get(export_record))
        .route(
            "/api/drafts/{id}/versions/{number}/export",
            get(export_version),
        )
        .route("/api/enrollments/{id}/export", get(export_enrollment))
        .route("/api/exports/records", get(export_installation))
        .route("/api/exports/summary", get(export_summary))
        .route("/api/enrollments/{id}/packet", get(export_packet))
        .route("/api/my/enrollments", get(my_enrollments))
}

fn export_refusal(refusal: ExportRefusal) -> ApiError {
    match refusal {
        ExportRefusal::NoSuchRecord => {
            ApiError::new(StatusCode::NOT_FOUND, "no_such_record", "no such record")
        }
        ExportRefusal::NoSuchVersion => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_version",
            "this record has no finalized version with that number",
        ),
        ExportRefusal::NoSuchEnrollment => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_enrollment",
            "no such enrollment",
        ),
        ExportRefusal::CapabilityRequired => ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "exporting takes the scope's read authority; the whole installation takes export_records",
        ),
        ExportRefusal::NothingToExport => ApiError::new(
            StatusCode::CONFLICT,
            "nothing_to_export",
            "this scope holds no finalized version; an export never claims completeness it lacks",
        ),
        ExportRefusal::ExportFailed => ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "export_failed",
            "the export could not be produced or delivered; nothing complete was sent",
        ),
    }
}

/// The answer when every export slot is taken: nothing was authorized,
/// audited, or started, and the client may simply try again.
fn export_busy() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "export_busy",
        format!("{EXPORT_SLOTS} exports are already in flight; retry shortly"),
    )
}

/// The archive as a download, streamed: the documented bytes are written
/// to the response as they are produced, so the whole corpus of stored
/// payloads is never held (#47; ADR 0014 costs). What still scales with
/// the installation is its unit count, not its bytes: the entry metadata
/// the archive manifest lists, the serialized manifest itself, and the
/// container's central directory are each O(units), and the database
/// driver buffers a bounded number of rows per query. The browser's own
/// download helper buffers the response it saves, separately.
///
/// A typed refusal is answered before the response starts. After it
/// starts, the body carries the archive and nothing else: a failure has
/// no status line left to change, so it ends the transfer without its
/// final chunk. The client reads that as an incomplete download, never as
/// a complete export — the bytes of a complete archive are exactly the
/// bytes the container writer produced, and a truncated stream cannot be
/// mistaken for them.
async fn deliver(state: &AppState, actor_user_id: i64, scope: Scope) -> Result<Response, ApiError> {
    // Admission comes first: a slot is taken before any work is done, held
    // by the producer for the whole download, and released when it ends,
    // so at most `EXPORT_SLOTS` producers exist and none ever queues.
    let Ok(slot) = SLOTS.try_acquire() else {
        return Err(export_busy());
    };
    let exported_at = OffsetDateTime::now_utc().unix_timestamp();
    let (started_tx, started_rx) = oneshot::channel();
    let (body_tx, body_rx) = mpsc::channel::<StreamItem>(BODY_CHUNKS);
    let failure_tx = body_tx.clone();
    let completion = Completion::new();
    let producer_completion = completion.clone();
    let pool = state.pool.clone();
    // The archive is produced on a blocking thread: it streams SQLite rows
    // and writes ZIP bytes, and a slow client must not stall the runtime.
    // The handle is dropped: the response body drives the producer. A
    // `spawn_blocking` task cannot be cancelled from outside once it runs,
    // so the producer asks for itself whether the response's receiver is
    // gone — dropped with this handler when the preflight fails or times
    // out, or when the client disconnects before the response starts — and
    // stops before the audit and the metadata pass rather than after them.
    let _work = tokio::task::spawn_blocking(move || {
        // The slot lives exactly as long as the producer.
        let _slot = slot;
        // The probe's sender lives in this closure, so it never keeps the
        // response body open after the producer ends.
        let abandoned = body_tx.clone();
        let cancelled = move || abandoned.is_closed();
        // The container writer patches each entry's local header after its
        // payload, so it writes through the sink that holds the entry and
        // hands the destination an append-only stream (#47).
        let sink = EntryBuffer::new(ChunkSink::new(body_tx));
        let runtime = tokio::runtime::Handle::current();
        let mut started = Some(started_tx);
        // A producer that goes away without finishing — a panic, or a
        // return this code forgot — must not leave the body to end
        // cleanly, which would look like a complete download.
        let mut guard = FailureGuard::new(failure_tx.clone());
        let produced = runtime.block_on(record_export::export_to(
            &pool,
            actor_user_id,
            scope,
            exported_at,
            sink,
            &cancelled,
            |produced| {
                // The scope is authorized, audited, and read: the response
                // may start, and only a failure before this point can still
                // reach the client as a typed answer.
                if let Some(started) = started.take() {
                    drop(started.send(Ok(produced)));
                }
                guard.arm();
            },
        ));
        // The destination's tail is handed on here, on the thread that owns
        // it: the archive is complete only once every byte is out.
        let (outcome, failure) = match produced {
            Ok((outcome, mut sink)) => {
                let flushed = sink.flush();
                drop(sink);
                (outcome, flushed.err().map(|err| err.to_string()))
            }
            Err(err) => {
                // A client that left is not a production failure: nobody is
                // left to read a signal, and the body simply closes. Any
                // other failure — including a client that stopped reading
                // altogether — ends the transfer incomplete, so it is never
                // saved as a whole export (#47).
                let client_gone = err
                    .chain()
                    .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
                    .any(|io| {
                        matches!(
                            io.kind(),
                            std::io::ErrorKind::BrokenPipe
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                        )
                    });
                if !client_gone {
                    tracing::error!(error = %err, "producing a record export failed");
                }
                (
                    Err(ExportRefusal::ExportFailed),
                    (!client_gone).then(|| err.to_string()),
                )
            }
        };
        // A typed refusal that never started a response is the caller's to
        // answer. A failure after the response started has no status line
        // left, so it goes to the body as the signal that ends the transfer
        // without its final chunk: a send that is awaited rather than
        // dropped, because a dropped future would never travel and the
        // download would end cleanly.
        if let (Err(refusal), Some(started)) = (&outcome, started.take()) {
            drop(started.send(Err(*refusal)));
        }
        // Success is a fact only when the archive was produced and its tail
        // flushed. Everything else leaves the completion unachieved, so the
        // body fails closed when the channel closes, whatever the queue had
        // room for.
        let complete = outcome.is_ok() && failure.is_none();
        if let Some(detail) = failure {
            signal_failure(&failure_tx, detail);
        }
        if complete {
            producer_completion.achieved();
        }
        guard.disarm();
        // The receiver is gone when the client left before the response
        // started; the export is over either way.
        drop(started);
    });
    let produced = match tokio::time::timeout(PREFLIGHT_LIMIT, started_rx).await {
        Ok(Ok(Ok(produced))) => produced,
        Ok(Ok(Err(refusal))) => return Err(export_refusal(refusal)),
        Ok(Err(_)) | Err(_) => {
            // The response body's receiver is dropped with this return, which
            // the producer's probe sees: it stops at its next check — before
            // the audit or the metadata pass if it has not reached them, at
            // the next metadata row if it is reading — and a producer already
            // past `ready` stops at its first send.
            return Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "export_failed",
                "the export could not be produced",
            ));
        }
    };
    let disposition = format!("attachment; filename=\"{}\"", produced.file_name);
    Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::new(ExportBody {
            body: body_rx,
            completion,
        }))
        .map_err(|err| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "export_failed",
                format!("the export response could not be built: {err}"),
            )
        })
}

/// One message of a streamed export body: bytes, or the failure that ends
/// the download short of a complete archive.
enum StreamItem {
    Chunk(Bytes),
    /// The failure that ended the archive short, when the queue had room
    /// for it. Only the detail: whether the transfer was complete is
    /// [`Completion`]'s answer, which needs no room at all.
    Failed(String),
}

/// Whether the producer finished a complete archive.
///
/// This is deliberately not the channel: a failure that cannot be queued —
/// because the client stopped reading and the queue is full — would leave
/// channel closure indistinguishable from success, and a client that
/// resumes reading would be handed a truncated prefix that looks complete.
/// Only an explicit success permits a clean end of body; anything else
/// fails closed.
#[derive(Clone)]
struct Completion(Arc<AtomicBool>);

impl Completion {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// The archive was produced and its tail flushed.
    fn achieved(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn is_achieved(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Hands the failure to the response body, so the transfer ends without
/// its final chunk rather than looking complete. This is a blocking send
/// with a bound: the producer is already on a blocking thread, and the
/// alternative — an unpolled `send` future — would drop the signal and end
/// the download cleanly.
fn signal_failure(sender: &mpsc::Sender<StreamItem>, detail: String) {
    let deadline = std::time::Instant::now() + EXPORT_STALL_LIMIT;
    if send_before(sender, StreamItem::Failed(detail), deadline).is_err() {
        tracing::error!("the export's failure could not reach the response body");
    }
}

/// Why one item never reached the response body.
enum SendOutcome {
    /// The receiver is gone: the client left.
    ClientGone,
    /// The queue stayed full until the deadline: the client stopped
    /// reading, so the transfer is over either way.
    Stalled,
}

/// Hands one item to the response body, waiting for room until `deadline`.
/// A full queue means a client that is not reading; the wait is a sleep on
/// a blocking thread, so no runtime worker is stalled and no queue grows.
fn send_before(
    sender: &mpsc::Sender<StreamItem>,
    mut item: StreamItem,
    deadline: std::time::Instant,
) -> std::result::Result<(), SendOutcome> {
    loop {
        item = match sender.try_send(item) {
            Ok(()) => return Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(SendOutcome::ClientGone),
            Err(mpsc::error::TrySendError::Full(item)) => item,
        };
        if std::time::Instant::now() >= deadline {
            return Err(SendOutcome::Stalled);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Ends a started download incomplete when the producer goes away without
/// completing it — the path a panic takes, where dropping the channel
/// alone would look like a clean end of body.
struct FailureGuard {
    sender: mpsc::Sender<StreamItem>,
    armed: bool,
}

impl FailureGuard {
    fn new(sender: mpsc::Sender<StreamItem>) -> Self {
        Self {
            sender,
            armed: false,
        }
    }

    /// Arms the guard once the response has started, so only a failure the
    /// client can still see is signalled.
    fn arm(&mut self) {
        self.armed = true;
    }

    /// The producer reached a known end: complete, refused, or a failure
    /// it signalled itself.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for FailureGuard {
    fn drop(&mut self) {
        if self.armed {
            signal_failure(
                &self.sender,
                "the export ended before it was complete".to_owned(),
            );
        }
    }
}

/// The response body of a streamed export. A failure ends the stream
/// without a clean end-of-body, so the transfer is incomplete rather than
/// an apparently successful partial file (issue #47).
struct ExportBody {
    body: mpsc::Receiver<StreamItem>,
    completion: Completion,
}

impl http_body::Body for ExportBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.body.poll_recv(cx) {
            Poll::Ready(Some(StreamItem::Chunk(bytes))) => {
                Poll::Ready(Some(Ok(Frame::data(bytes))))
            }
            Poll::Ready(Some(StreamItem::Failed(detail))) => Poll::Ready(Some(Err(
                std::io::Error::other(format!("the export could not be completed: {detail}")),
            ))),
            // The stream ended. Unless the producer said it finished a
            // complete archive, that end is a failure, however clean the
            // channel closure looked.
            Poll::Ready(None) => Poll::Ready(if self.completion.is_achieved() {
                None
            } else {
                Some(Err(std::io::Error::other(
                    "the export ended before it was complete",
                )))
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// The archive's destination: an append-only stream of numbered pieces
/// over the response body. The container writer hands it finished entries,
/// so it never rewrites a byte; the channel's bound is the backpressure,
/// and a closed channel ends the export instead of buffering it for nobody.
struct ChunkSink {
    sender: mpsc::Sender<StreamItem>,
    /// How many bytes have been handed to the response: the position the
    /// destination has reached.
    sent: u64,
    /// How long one chunk may wait for room before the transfer is ended.
    stall_limit: Duration,
}

impl ChunkSink {
    fn new(sender: mpsc::Sender<StreamItem>) -> Self {
        Self::with_stall_limit(sender, EXPORT_STALL_LIMIT)
    }

    /// A sink with its own stall bound, so the unit tests can exercise a
    /// stalled client without waiting out the production one per chunk.
    fn with_stall_limit(sender: mpsc::Sender<StreamItem>, stall_limit: Duration) -> Self {
        Self {
            sender,
            sent: 0,
            stall_limit,
        }
    }

    /// Hands one chunk to the response body, waiting while the queue is
    /// full so the producer stays inside the channel's bound rather than
    /// growing a queue or stalling a runtime worker. The wait is bounded:
    /// a client that stops reading altogether loses the transfer, which it
    /// reads as an incomplete download, and a client that keeps making
    /// progress — however slowly — never notices the bound.
    fn send_bytes(&mut self, bytes: Vec<u8>) -> std::io::Result<()> {
        let deadline = std::time::Instant::now() + self.stall_limit;
        self.send_within(bytes, deadline)
    }

    /// The same hand-off with the caller's deadline, so the stall bound is
    /// the caller's to choose.
    fn send_within(&mut self, bytes: Vec<u8>, deadline: std::time::Instant) -> std::io::Result<()> {
        let size = bytes.len() as u64;
        match send_before(
            &self.sender,
            StreamItem::Chunk(Bytes::from(bytes)),
            deadline,
        ) {
            Ok(()) => {
                self.sent += size;
                Ok(())
            }
            Err(SendOutcome::ClientGone) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the client left",
            )),
            // Not the client's departure: the transfer ends incomplete and
            // the client can see that it did.
            Err(SendOutcome::Stalled) => Err(std::io::Error::other("the client stopped reading")),
        }
    }
}

impl std::io::Write for ChunkSink {
    /// Queues at most [`BODY_CHUNK_BYTES`] of `buf` and reports how much it
    /// took; `write_all` comes back for the rest, so a large entry becomes
    /// many bounded chunks rather than one item the size of the record.
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let take = buf.len().min(BODY_CHUNK_BYTES);
        self.send_bytes(buf[..take].to_vec())?;
        Ok(take)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl std::io::Seek for ChunkSink {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        use std::io::SeekFrom;
        match pos {
            SeekFrom::Current(0) | SeekFrom::End(0) => Ok(self.sent),
            SeekFrom::Start(at) if at == self.sent => Ok(at),
            SeekFrom::Start(at) => Err(std::io::Error::other(format!(
                "a streamed export cannot rewrite position {at}"
            ))),
            other => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!("a streamed export is append-only ({other:?})"),
            )),
        }
    }
}

async fn export_version(
    State(state): State<AppState>,
    current: CurrentUser,
    Path((record_id, version_number)): Path<(i64, i64)>,
) -> Result<Response, ApiError> {
    deliver(
        &state,
        current.user.id,
        Scope::Version {
            record_id,
            version_number,
        },
    )
    .await
}

async fn export_record(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(record_id): Path<i64>,
) -> Result<Response, ApiError> {
    deliver(&state, current.user.id, Scope::Record { record_id }).await
}

async fn export_enrollment(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(enrollment_id): Path<i64>,
) -> Result<Response, ApiError> {
    deliver(&state, current.user.id, Scope::Enrollment { enrollment_id }).await
}

async fn export_installation(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Response, ApiError> {
    deliver(&state, current.user.id, Scope::Installation).await
}

async fn export_summary(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Response, ApiError> {
    match record_export::summary(&state.pool, current.user.id).await? {
        Ok(summary) => Ok(Json(summary).into_response()),
        Err(refusal) => Err(export_refusal(refusal)),
    }
}

// ------------------------------------------------------- trainee packets

fn packet_refusal(refusal: PacketRefusal) -> ApiError {
    match refusal {
        PacketRefusal::NoSuchEnrollment => ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_enrollment",
            "no such enrollment",
        ),
        PacketRefusal::CapabilityRequired => ApiError::new(
            StatusCode::FORBIDDEN,
            "capability_required",
            "a packet takes the enrollment's training-history read authority, the trainee's own view_own_records, or export_records",
        ),
    }
}

async fn export_packet(
    State(state): State<AppState>,
    current: CurrentUser,
    Path(enrollment_id): Path<i64>,
) -> Result<Response, ApiError> {
    match trainee_packet::export(&state.pool, current.user.id, enrollment_id).await? {
        Ok(packet) => {
            let disposition = format!("attachment; filename=\"{}\"", packet.file_name);
            Ok((
                [
                    (header::CONTENT_TYPE, "application/zip".to_owned()),
                    (header::CONTENT_DISPOSITION, disposition),
                ],
                packet.bytes,
            )
                .into_response())
        }
        Err(refusal) => Err(packet_refusal(refusal)),
    }
}

#[derive(serde::Serialize)]
struct MyEnrollmentsBody {
    enrollments: Vec<trainee_packet::OwnEnrollment>,
}

async fn my_enrollments(
    State(state): State<AppState>,
    current: CurrentUser,
) -> Result<Response, ApiError> {
    match trainee_packet::own_enrollments(&state.pool, current.user.id).await? {
        Ok(enrollments) => Ok(Json(MyEnrollmentsBody { enrollments }).into_response()),
        Err(refusal) => Err(packet_refusal(refusal)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body::Body as _;

    /// Polls one frame of a body, so the terminal cases can be asserted.
    async fn frame(body: &mut ExportBody) -> Option<Result<Frame<Bytes>, std::io::Error>> {
        std::future::poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await
    }

    /// A body whose producer finished a complete archive ends cleanly.
    #[tokio::test]
    async fn an_achieved_completion_ends_the_body() {
        let (sender, receiver) = mpsc::channel::<StreamItem>(BODY_CHUNKS);
        let completion = Completion::new();
        let mut body = ExportBody {
            body: receiver,
            completion: completion.clone(),
        };
        sender
            .send(StreamItem::Chunk(Bytes::from_static(b"PK")))
            .await
            .expect("queue a chunk");
        completion.achieved();
        drop(sender);
        assert!(matches!(frame(&mut body).await, Some(Ok(_))));
        assert!(frame(&mut body).await.is_none());
    }

    /// A body whose producer went away without finishing fails closed, even
    /// though the channel closed exactly as a successful run's would: this
    /// is what a client that stopped reading long enough to lose the
    /// failure detail must still be told.
    #[tokio::test]
    async fn a_closed_channel_without_completion_fails_the_body() {
        let (sender, receiver) = mpsc::channel::<StreamItem>(BODY_CHUNKS);
        let mut body = ExportBody {
            body: receiver,
            completion: Completion::new(),
        };
        sender
            .send(StreamItem::Chunk(Bytes::from_static(b"PK")))
            .await
            .expect("queue a chunk");
        drop(sender);
        assert!(matches!(frame(&mut body).await, Some(Ok(_))));
        let end = frame(&mut body).await;
        assert!(matches!(end, Some(Err(_))), "{end:?}");
    }

    /// A failure after the response started must reach the body as an
    /// error: a body that simply ended would look to every client like a
    /// complete download of a short file (issue #47).
    #[tokio::test]
    async fn a_failure_item_ends_the_body_with_an_error() {
        let (sender, receiver) = mpsc::channel::<StreamItem>(BODY_CHUNKS);
        let mut body = ExportBody {
            body: receiver,
            completion: Completion::new(),
        };
        sender
            .send(StreamItem::Chunk(Bytes::from_static(b"PK\x03\x04")))
            .await
            .expect("queue the first chunk");
        signal_failure(&sender, "an invented production failure".to_owned());
        drop(sender);

        assert!(matches!(frame(&mut body).await, Some(Ok(_))));
        let failure = frame(&mut body).await;
        assert!(matches!(failure, Some(Err(_))), "{failure:?}");
    }

    /// The guard is what a panic relies on: without it, a producer that
    /// goes away mid-download would drop the channel and the body would end
    /// cleanly.
    #[tokio::test]
    async fn a_producer_that_goes_away_after_starting_signals_a_failure() {
        let (sender, mut receiver) = mpsc::channel::<StreamItem>(BODY_CHUNKS);
        let mut guard = FailureGuard::new(sender.clone());
        guard.arm();
        drop(guard);
        drop(sender);
        assert!(matches!(receiver.recv().await, Some(StreamItem::Failed(_))));
    }

    /// A guard that was disarmed — a complete export, a typed refusal, or a
    /// failure the producer signalled itself — sends nothing.
    #[tokio::test]
    async fn a_disarmed_producer_sends_nothing() {
        let (sender, mut receiver) = mpsc::channel::<StreamItem>(BODY_CHUNKS);
        let mut guard = FailureGuard::new(sender.clone());
        guard.arm();
        guard.disarm();
        drop(guard);
        drop(sender);
        assert!(receiver.recv().await.is_none());
    }

    /// A stall is not the client's departure: it must be classified as a
    /// failure, because the client is still there to see the difference.
    #[tokio::test]
    async fn a_stalled_send_is_not_a_client_departure() {
        let (sender, mut receiver) = mpsc::channel::<StreamItem>(1);
        sender
            .send(StreamItem::Chunk(Bytes::from_static(b"first")))
            .await
            .expect("fill the queue");
        let mut sink = ChunkSink::new(sender);
        let started = std::time::Instant::now();
        let err = sink
            .send_within(b"second".to_vec(), started + Duration::from_millis(50))
            .expect_err("the queue is full and nobody reads it");
        assert_ne!(err.kind(), std::io::ErrorKind::BrokenPipe, "{err:?}");
        assert!(
            started.elapsed() >= Duration::from_millis(50),
            "the send gave up after {:?}",
            started.elapsed()
        );
        assert!(receiver.try_recv().is_ok());
    }

    /// The response queue is bounded in bytes, not only in items: a record
    /// of any size reaches it in pieces of at most `BODY_CHUNK_BYTES`, so a
    /// client that stops reading holds at most `BODY_CHUNKS` of them. A
    /// whole entry queued as one item would let a single large record sit
    /// in memory for a client that is not reading (#47).
    #[tokio::test]
    async fn a_large_entry_is_queued_in_bounded_pieces() {
        let (sender, mut receiver) = mpsc::channel::<StreamItem>(BODY_CHUNKS);
        let mut sink = ChunkSink::with_stall_limit(sender, Duration::from_millis(50));
        let err = sink
            .write_all(&vec![7u8; 1024 * 1024])
            .expect_err("nobody reads, so the queue fills before the entry is out");
        assert_ne!(err.kind(), std::io::ErrorKind::BrokenPipe, "{err:?}");
        let mut queued = 0;
        while let Ok(item) = receiver.try_recv() {
            match item {
                StreamItem::Chunk(bytes) => {
                    assert!(
                        bytes.len() <= BODY_CHUNK_BYTES,
                        "one queued item holds {} bytes",
                        bytes.len()
                    );
                    queued += bytes.len();
                }
                StreamItem::Failed(detail) => panic!("unexpected failure item: {detail}"),
            }
        }
        assert_eq!(
            queued,
            BODY_CHUNKS * BODY_CHUNK_BYTES,
            "the queue held {queued} bytes for a client that read nothing"
        );
    }

    /// A client that left is reported as exactly that, so the producer can
    /// tell "nobody is listening" from "this failed".
    #[tokio::test]
    async fn a_departed_client_is_a_broken_pipe() {
        let (sender, receiver) = mpsc::channel::<StreamItem>(1);
        drop(receiver);
        let mut sink = ChunkSink::new(sender);
        let err = sink
            .send_within(
                b"bytes".to_vec(),
                std::time::Instant::now() + Duration::from_secs(1),
            )
            .expect_err("the receiver is gone");
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe, "{err:?}");
    }
}
