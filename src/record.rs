use quick_xml::events::Event;
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use serde::de::DeserializeOwned;
use std::fmt;

#[derive(Debug, Clone)]
pub struct DatasetRecord {
    index: u64,
    bytes: Vec<u8>,
}

impl DatasetRecord {
    pub(crate) fn new(index: u64, bytes: Vec<u8>) -> Self {
        Self { index, bytes }
    }

    pub fn index(&self) -> u64 { self.index }
    pub fn len(&self) -> usize { self.bytes.len() }
    pub fn is_empty(&self) -> bool { self.bytes.is_empty() }
    pub fn as_bytes(&self) -> &[u8] { &self.bytes }

    /// Build a lightweight XML preview containing the record start and first three child elements.
    pub(crate) fn preview(&self) -> RecordResult<LoadedRecord> {
        let mut reader = Reader::from_reader(self.bytes.as_slice());
        reader.config_mut().trim_text(false);

        let mut buffer = Vec::new();
        let mut preview = Vec::new();
        let mut root_seen = false;
        let mut child_count = 0usize;

        loop {
            buffer.clear();
            match reader.read_event_into(&mut buffer)? {
                Event::Start(event) if !root_seen => {
                    let mut writer = Writer::new(&mut preview);
                    writer.write_event(Event::Start(event.into_owned()))?;
                    root_seen = true;
                }
                Event::Start(_) if root_seen && child_count < 3 => {
                    let mut depth = 1usize;
                    let mut writer = Writer::new(&mut preview);

                    loop {
                        buffer.clear();
                        let event = reader.read_event_into(&mut buffer)?;
                        match event {
                            Event::Start(_) => {
                                depth += 1;
                                writer.write_event(event)?;
                            }
                            Event::End(_) => {
                                writer.write_event(event)?;
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            Event::Eof => {
                                return Err(RecordError::InvalidConfiguration(
                                    "unexpected end of input while building record preview".into(),
                                ));
                            }
                            _ => writer.write_event(event)?,
                        }
                    }

                    child_count += 1;
                }
                Event::Empty(event) if root_seen && child_count < 3 => {
                    let mut writer = Writer::new(&mut preview);
                    writer.write_event(Event::Empty(event.into_owned()))?;
                    child_count += 1;
                }
                Event::Eof => break,
                _ => {}
            }

            if child_count == 3 {
                break;
            }
        }

        Ok(LoadedRecord::new(self.index, preview))
    }

    pub fn decode<T: DeserializeOwned>(&self) -> RecordResult<T> {
        quick_xml::de::from_reader(self.bytes.as_slice()).map_err(RecordError::decode)
    }
}

#[derive(Debug, Clone)]
pub struct LoadedRecord {
    index: u64,
    content: Vec<u8>,
}

impl LoadedRecord {
    fn new(index: u64, content: Vec<u8>) -> Self {
        Self { index, content }
    }

    pub fn index(&self) -> u64 { self.index }
    pub fn as_bytes(&self) -> &[u8] { &self.content }
    pub fn len(&self) -> usize { self.content.len() }
    pub fn is_empty(&self) -> bool { self.content.is_empty() }
}

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
