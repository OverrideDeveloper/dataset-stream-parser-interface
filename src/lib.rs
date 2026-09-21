//! Streaming dataset record interfaces.
//!
//! The core abstraction is intentionally independent of a particular dataset:
//! a source is streamed, a meaningful record boundary is recognized, and one
//! bounded record is exposed to the caller at a time.

mod engine;
mod record;
pub mod xml;

pub use engine::{DatasetEngine, RecordSource};
pub use record::{DatasetRecord, RecordDecoder, RecordError, RecordResult};
pub use xml::{XmlRecordStream, XmlStreamConfig};

/// A source of bounded dataset records.
pub trait RecordStream {
    /// Return the next record, or None at end of input.
    fn next_record(&mut self) -> RecordResult<Option<DatasetRecord>>;
}
