//! Streaming dataset record interfaces.

mod engine;
#[path = "../manip/mod.rs"]
pub mod manip;
mod record;
pub mod text;
pub mod xml;

pub use engine::{Checkpoint, DatasetEngine, RecordSource};
pub use record::{
    DatasetRecord, LoadedRecord, PreviewConfig, PreviewElement, PreviewRecord, RecordDecoder,
    RecordError, RecordResult,
};
pub use text::{TextRecordStream, TextStreamConfig};
pub use xml::{XmlRecordStream, XmlStreamConfig};

/// A source of bounded dataset records.
pub trait RecordStream {
    fn next_record(&mut self) -> RecordResult<Option<DatasetRecord>>;
    fn checkpoint_position(&self) -> Option<u64> { None }
}
