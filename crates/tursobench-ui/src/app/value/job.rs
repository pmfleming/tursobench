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
use tursobench_blob::tree::{self, Limits, Tree};

/// What a job decodes.
pub(in crate::app) enum Work {
    /// A whole JSONB value.
    Jsonb(Arc<[u8]>),
    /// The first bytes of a longer JSONB value `len` bytes long.
    JsonbPrefix(Arc<[u8]>, u64),
}
impl Work {
    fn run(&self, cancel: &AtomicBool) -> Output {
        match self {
            Self::Jsonb(bytes) => Output::Tree(
                tree::jsonb(bytes, &Limits::default(), cancel)
                    .map(Arc::new)
                    .map_err(|e| match e {
                        tree::Error::Invalid(reason) => reason.to_owned(),
                        tree::Error::Cancelled => "Cancelled".to_owned(),
                    }),
            ),
            Self::JsonbPrefix(head, len) => Output::Tree(
                tree::jsonb_prefix(head, *len)
                    .map(Arc::new)
                    .ok_or_else(|| "Not valid JSONB".to_owned()),
            ),
        }
    }
}

/// What a job produced.
#[derive(Clone)]
pub(in crate::app) enum Output {
    Tree(Result<Arc<Tree>, String>),
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
        Work::Jsonb(_) | Work::JsonbPrefix(..) => Output::Tree(Err(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::{Decoder, Output, Work};
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
        let stale = decoder.submit(&ctx, Work::Jsonb(Arc::from(&b"\x00"[..])));
        let id = decoder.submit(&ctx, Work::Jsonb(object));
        assert!(stale < id);
        let Output::Tree(Ok(tree)) = wait(&mut decoder, id) else {
            panic!("expected a tree")
        };
        assert_eq!(tree.json(), r#"{"a":1}"#);
        assert!(decoder.poll(stale).is_none());
        let id = decoder.submit(&ctx, Work::Jsonb(Arc::from(&b"\x1f"[..])));
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
