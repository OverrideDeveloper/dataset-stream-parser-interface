use serde::de::DeserializeOwned;
use std::fmt;

/// A bounded record extracted from a dataset stream.
#[derive(Debug, Clone)]
pub struct DatasetRecord {
    index: u64,
    bytes: Vec<u8>,
}

impl DatasetRecord {
    pub(crate) fn new(index: u64, bytes: Vec<u8>) -> Self {
        Self { index, bytes }
    }

    /// Zero-based position of this record in the stream.
    pub fn index(&self) -> u64 { self.index }

    /// Size of this record in bytes.
    pub fn len(&self) -> usize { self.bytes.len() }

    /// Whether the record contains no bytes.
    pub fn is_empty(&self) -> bool { self.bytes.is_empty() }

    /// Access the serialized record without copying it.
    pub fn as_bytes(&self) -> &[u8] { &self.bytes }

    /// Decode this bounded record into an application-owned type.
    pub fn decode<T: DeserializeOwned>(&self) -> RecordResult<T> {
        quick_xml::de::from_reader(self.bytes.as_slice()).map_err(RecordError::decode)
    }
}

/// Optional schema-specific decoder for callers that want a reusable transformation.
pub trait RecordDecoder<T> {
    fn decode(&self, record: &DatasetRecord) -> RecordResult<T>;
}

#[derive(Debug)]
pub enum RecordError {
    Io(std::io::Error),
    Xml(quick_xml::Error),
    Decode(quick_xml::DeError),
    InvalidConfiguration(String),
}

impl RecordError {
    pub(crate) fn decode(error: quick_xml::DeError) -> Self { Self::Decode(error) }
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Xml(error) => write!(f, "XML error: {error}"),
            Self::Decode(error) => write!(f, "record decode error: {error}"),
            Self::InvalidConfiguration(message) => write!(f, "invalid configuration: {message}"),
        }
    }
}

impl std::error::Error for RecordError {}

impl From<std::io::Error> for RecordError {
    fn from(error: std::io::Error) -> Self { Self::Io(error) }
}

impl From<quick_xml::Error> for RecordError {
    fn from(error: quick_xml::Error) -> Self { Self::Xml(error) }
}

pub type RecordResult<T> = Result<T, RecordError>;
