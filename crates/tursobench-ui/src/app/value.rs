//! Blob and deferred values: what they are, in the words every view shares.
mod summary;

#[cfg(test)]
pub(super) use summary::MADE as SUMMARIES_MADE;
pub(super) use summary::Summary;
