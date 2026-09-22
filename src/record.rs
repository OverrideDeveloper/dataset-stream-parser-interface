use quick_xml::events::Event;
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use serde::de::DeserializeOwned;
use std::fmt;

#[derive(Debug, Clone)]
pub struct PreviewConfig {
    /// Target preview size in bytes. The child that reaches this target is included.
    pub max_bytes: usize,
    /// Optional child element name that ends the preview when encountered.
    pub stop_element: Option<String>,
    /// Maximum number of top-level child elements retained in the preview.
    pub max_children: usize,
}

impl PreviewConfig {
    pub fn new(max_bytes: usize, max_children: usize) -> Self {
        Self {
            max_bytes,
            stop_element: None,
            max_children,
        }
    }

    pub fn with_stop_element(mut self, element: impl Into<String>) -> Self {
        self.stop_element = Some(element.into());
        self
    }

    pub(crate) fn validate(&self) -> RecordResult<()> {
        if self.max_bytes == 0 {
            return Err(RecordError::InvalidConfiguration(
                "preview max_bytes must be greater than zero".into(),
            ));
        }
        if self.max_children == 0 {
            return Err(RecordError::InvalidConfiguration(
                "preview max_children must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self {
            max_bytes: 4096,
            stop_element: Some("title".into()),
            max_children: 10,
        }
    }
}

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

    /// Build a bounded XML preview from the record start and child elements.
    pub(crate) fn preview(&self, config: &PreviewConfig) -> RecordResult<LoadedRecord> {
        config.validate()?;

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
                Event::Start(event) if root_seen && child_count < config.max_children => {
                    let child_name = event.name().as_ref().to_vec();
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
                    let reached_target = preview.len() >= config.max_bytes;
                    let reached_stop_element = config
                        .stop_element
                        .as_deref()
                        .is_some_and(|name| name.as_bytes() == child_name.as_slice());

                    if reached_target || reached_stop_element || child_count >= config.max_children {
                        break;
                    }
                }
                Event::Empty(event) if root_seen && child_count < config.max_children => {
                    let child_name = event.name().as_ref().to_vec();
                    let mut writer = Writer::new(&mut preview);
                    writer.write_event(Event::Empty(event.into_owned()))?;
                    child_count += 1;

                    let reached_target = preview.len() >= config.max_bytes;
                    let reached_stop_element = config
                        .stop_element
                        .as_deref()
                        .is_some_and(|name| name.as_bytes() == child_name.as_slice());

                    if reached_target || reached_stop_element || child_count >= config.max_children {
                        break;
                    }
                }
                Event::Eof => break,
                _ => {}
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
