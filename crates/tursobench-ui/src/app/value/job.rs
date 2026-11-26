//! Decoding off the UI thread: one job at a time on one thread, each bounded,
//! caught if it panics, and cancelled when the next is submitted.
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};
use tursobench_blob::{
    Kind,
    formats::{self, Format},
    image::{self, Picture},
    inflate,
    sniff::{Compression, ImageFormat},
    tree::{self, Limits, Tree},
    vector::{self, Vector},
};

/// What a job decodes.
pub(in crate::app) enum Work {
    /// A whole value in a structured format.
    Tree(Arc<[u8]>, Format),
    /// The first bytes of a longer JSONB value `len` bytes long.
    JsonbPrefix(Arc<[u8]>, u64),
    /// An image, scaled to fit a square this many pixels a side.
    Image(Arc<[u8]>, ImageFormat, u32),
    /// A vector; plain float32 when no type byte names another layout.
    Vector(Arc<[u8]>),
    /// Compressed contents, inflated and read again.
    Contents(Arc<[u8]>, Compression),
    /// Formats without a signature, tried against the whole value.
    Probe(Arc<[u8]>),
}

/// Inflated contents, and what they are.
pub(in crate::app) struct Contents {
    pub compression: Compression,
    pub kind: Kind,
    pub len: usize,
    pub truncated: bool,
    /// The contents as text, when they are text: at most 1 MiB.
    pub text: Option<String>,
    /// The contents as a tree, when they are a structured format.
    pub tree: Option<Arc<Tree>>,
    pub bytes: Arc<[u8]>,
}
impl Work {
    fn run(&self, cancel: &AtomicBool) -> Output {
        match self {
            Self::Tree(bytes, format) => Output::Tree(
                formats::decode(*format, bytes, &Limits::default(), cancel)
                    .map(Arc::new)
                    .map_err(reason),
            ),
            Self::JsonbPrefix(head, len) => Output::Tree(
                tree::jsonb_prefix(head, *len)
                    .map(Arc::new)
                    .ok_or_else(|| "Not valid JSONB".to_owned()),
            ),
            Self::Image(bytes, format, fit) => {
                Output::Image(image::decode(bytes, *format, *fit, cancel).map(Arc::new))
            }
            Self::Vector(bytes) => Output::Vector(vector::read(bytes, true).map(Arc::new)),
            Self::Contents(bytes, compression) => {
                Output::Contents(contents(bytes, *compression, cancel).map(Arc::new))
            }
            Self::Probe(bytes) => Output::Probe(formats::probe(bytes, cancel)),
        }
    }
}

fn reason(error: tree::Error) -> String {
    match error {
        tree::Error::Invalid(reason) => reason.to_owned(),
        tree::Error::Cancelled => "Cancelled".to_owned(),
    }
}

/// Inflate one level, then read the contents again: as text, a tree, or bytes.
fn contents(
    bytes: &[u8],
    compression: Compression,
    cancel: &AtomicBool,
) -> Result<Contents, String> {
    let inflated = inflate::inflate(bytes, compression, inflate::OUT, cancel)?;
    let bytes: Arc<[u8]> = Arc::from(inflated.bytes);
    let len = bytes.len() as u64;
    let kind =
        tursobench_blob::sniff::sniff_tail(&bytes, &bytes[bytes.len().saturating_sub(8)..], len);
    let format = match kind {
        Kind::Jsonb => Some(Format::Jsonb),
        Kind::Structured(format) => Some(format),
        _ => None,
    };
    let tree = format
        .and_then(|format| formats::decode(format, &bytes, &Limits::default(), cancel).ok())
        .map(Arc::new);
    let text = match kind {
        Kind::Text(encoding) => Some(tursobench_blob::text::decode(
            &bytes[..bytes.len().min(1 << 20)],
            encoding,
        )),
        _ => None,
    };
    Ok(Contents {
        compression,
        kind,
        len: bytes.len(),
        truncated: inflated.truncated,
        text,
        tree,
        bytes,
    })
}

/// What a job produced.
#[derive(Clone)]
pub(in crate::app) enum Output {
    Tree(Result<Arc<Tree>, String>),
    Image(Result<Arc<Picture>, String>),
    Vector(Result<Arc<Vector>, String>),
    Contents(Result<Arc<Contents>, String>),
    Probe(Option<Format>),
}

struct Job {
    id: u64,
    work: Work,
    cancel: Arc<AtomicBool>,
}

/// The decode thread of one document, started when first needed.
/// Jobs to the thread, and its answers by job id.
type Channel = (Sender<Job>, Receiver<(u64, Output)>);

#[derive(Default)]
pub(in crate::app) struct Decoder {
    channel: Option<Channel>,
    cancel: Arc<AtomicBool>,
    current: u64,
}
impl Decoder {
    /// Start `work`, cancelling the job before it; returns the new job's id.
    pub fn submit(&mut self, ctx: &eframe::egui::Context, work: Work) -> u64 {
        self.cancel();
        self.current += 1;
        self.cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            id: self.current,
            work,
            cancel: self.cancel.clone(),
        };
        let (jobs, _) = self.channel.get_or_insert_with(|| spawn(ctx.clone()));
        if let Err(mpsc::SendError(job)) = jobs.send(job) {
            // The thread is gone: start another for this job.
            self.channel = None;
            let (jobs, _) = self.channel.get_or_insert_with(|| spawn(ctx.clone()));
            let _ = jobs.send(job);
        }
        self.current
    }
    /// Add `work` behind the jobs already waiting; returns its id. Jobs
    /// queued together are cancelled together.
    pub fn queue(&mut self, ctx: &eframe::egui::Context, work: Work) -> u64 {
        self.current += 1;
        let job = Job {
            id: self.current,
            work,
            cancel: self.cancel.clone(),
        };
        let (jobs, _) = self.channel.get_or_insert_with(|| spawn(ctx.clone()));
        let _ = jobs.send(job);
        self.current
    }
    /// Every result delivered since the last poll.
    pub fn poll_all(&mut self) -> Vec<(u64, Output)> {
        let Some((_, results)) = self.channel.as_ref() else {
            return Vec::new();
        };
        results.try_iter().collect()
    }
    /// Cancel every queued job; later jobs are not affected.
    pub fn cancel_queued(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
    }
    /// Stop the job under way, if any; its result is never delivered.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    /// The result of the latest job, once it is ready.
    pub fn poll(&mut self, id: u64) -> Option<Output> {
        let (_, results) = self.channel.as_ref()?;
        let mut latest = None;
        while let Ok((done, output)) = results.try_recv() {
            if done == id {
                latest = Some(output);
            }
        }
        latest
    }
}

fn spawn(ctx: eframe::egui::Context) -> Channel {
    let (jobs, inbox) = mpsc::channel::<Job>();
    let (outbox, results) = mpsc::channel();
    std::thread::Builder::new()
        .name("value-decoder".into())
        .spawn(move || {
            while let Ok(job) = inbox.recv() {
                if job.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                let output = catch_unwind(AssertUnwindSafe(|| job.work.run(&job.cancel)))
                    .unwrap_or_else(|_| failed(&job.work));
                if job.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                if outbox.send((job.id, output)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        })
        .map_or_else(
            // Without a thread, jobs are never answered; the views say so.
            |_| (mpsc::channel().0, mpsc::channel().1),
            |_| (jobs, results),
        )
}

/// The answer for a job whose decoder panicked.
fn failed(work: &Work) -> Output {
    let reason = "The decoder stopped on malformed input".to_owned();
    match work {
        Work::Tree(..) | Work::JsonbPrefix(..) => Output::Tree(Err(reason)),
        Work::Image(..) => Output::Image(Err(reason)),
        Work::Vector(_) => Output::Vector(Err(reason)),
        Work::Contents(..) => Output::Contents(Err(reason)),
        Work::Probe(_) => Output::Probe(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{Decoder, Format, Output, Work};
    use std::sync::Arc;

    fn wait(decoder: &mut Decoder, id: u64) -> Output {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(output) = decoder.poll(id) {
                return output;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "decoder never answered"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn only_the_latest_job_is_answered() {
        let ctx = eframe::egui::Context::default();
        let mut decoder = Decoder::default();
        let object: Arc<[u8]> = Arc::from(&b"\x4c\x17a\x131"[..]);
        let stale = decoder.submit(&ctx, Work::Tree(Arc::from(&b"\x00"[..]), Format::Jsonb));
        let id = decoder.submit(&ctx, Work::Tree(object, Format::Jsonb));
        assert!(stale < id);
        let Output::Tree(Ok(tree)) = wait(&mut decoder, id) else {
            panic!("expected a tree")
        };
        assert_eq!(tree.json(), r#"{"a":1}"#);
        assert!(decoder.poll(stale).is_none());
        let id = decoder.submit(&ctx, Work::Tree(Arc::from(&b"\x1f"[..]), Format::Jsonb));
        assert!(matches!(wait(&mut decoder, id), Output::Tree(Err(_))));
        let id = decoder.submit(
            &ctx,
            Work::JsonbPrefix(Arc::from(&b"\xcc\x40\x17a"[..]), 66),
        );
        let Output::Tree(Ok(tree)) = wait(&mut decoder, id) else {
            panic!("expected a partial tree")
        };
        assert!(!tree.complete);
    }
}
