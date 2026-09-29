use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewElement {
    pub name: String,
    pub text: String,
}

impl PreviewElement {
    fn retained_bytes(&self) -> usize {
        self.name.len() + self.text.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewRecord {
    pub index: u64,
    pub elements: Vec<PreviewElement>,
}

impl PreviewRecord {
    fn new(index: u64, elements: Vec<PreviewElement>) -> Self {
        Self { index, elements }
    }

    pub fn index(&self) -> u64 {
        self.index
    }

    pub fn elements(&self) -> &[PreviewElement] {
        &self.elements
    }

    pub fn len(&self) -> usize {
        self.elements
            .iter()
            .map(PreviewElement::retained_bytes)
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn contains_text(&self, query: &str) -> bool {
        self.elements
            .iter()
            .any(|element| element.text.contains(query))
    }
}

pub type LoadedRecord = PreviewRecord;

#[derive(Debug, Clone)]
pub struct PreviewConfig {
    /// Target retained UTF-8 bytes across element names and text.
    /// The element that reaches this target is included.
    pub target_bytes: usize,
    /// Optional child element name that ends the preview when encountered.
    pub stop_element: Option<String>,
    /// Maximum number of top-level child elements retained in the preview.
    pub max_children: usize,
}

impl PreviewConfig {
    pub fn new(target_bytes: usize, max_children: usize) -> Self {
        Self {
            target_bytes,
            stop_element: None,
            max_children,
        }
    }

    pub fn with_stop_element(mut self, element: impl Into<String>) -> Self {
        self.stop_element = Some(element.into());
        self
    }

    pub(crate) fn validate(&self) -> RecordResult<()> {
        if self.target_bytes == 0 {
            return Err(RecordError::InvalidConfiguration(
                "preview target_bytes must be greater than zero".into(),
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
            target_bytes: 4096,
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

    /// Build a bounded, schema-neutral text projection from the record.
    ///
    /// Each retained top-level child becomes one named text element. Nested
    /// descendant text is flattened into the containing child, so the preview
    /// remains useful for search without retaining XML syntax.
    pub fn preview_xml(&self, config: &PreviewConfig) -> RecordResult<PreviewRecord> {
        config.validate()?;

        let mut reader = Reader::from_reader(self.bytes.as_slice());
        reader.config_mut().trim_text(false);

        let mut buffer = Vec::new();
        let mut root_seen = false;
        let mut elements = Vec::new();

        loop {
            buffer.clear();
            match reader.read_event_into(&mut buffer)? {
                Event::Start(_) if !root_seen => {
                    root_seen = true;
                }
                Event::Start(event) if root_seen && elements.len() < config.max_children => {
                    let child_name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                    let text = collect_element_text(&mut reader, &mut buffer)?;
                    let element = PreviewElement {
                        name: child_name.clone(),
                        text,
                    };
                    let reached_target =
                        elements.iter().map(PreviewElement::retained_bytes).sum::<usize>()
                            + element.retained_bytes()
                            >= config.target_bytes;
                    let reached_stop_element = config
                        .stop_element
                        .as_deref()
                        .is_some_and(|name| name == child_name);

                    elements.push(element);

                    if reached_target
                        || reached_stop_element
                        || elements.len() >= config.max_children
                    {
                        break;
                    }
                }
                Event::Empty(event) if root_seen && elements.len() < config.max_children => {
                    let child_name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                    let element = PreviewElement {
                        name: child_name.clone(),
                        text: String::new(),
                    };
                    let reached_target =
                        elements.iter().map(PreviewElement::retained_bytes).sum::<usize>()
                            + element.retained_bytes()
                            >= config.target_bytes;
                    let reached_stop_element = config
                        .stop_element
                        .as_deref()
                        .is_some_and(|name| name == child_name);

                    elements.push(element);

                    if reached_target
                        || reached_stop_element
                        || elements.len() >= config.max_children
                    {
                        break;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }

        Ok(PreviewRecord::new(self.index, elements))
    }

    /// Build a bounded preview for a plain-text record.
    ///
    /// Plain text has no child-element structure, so the preview is represented
    /// as one named "text" element. The bounded text window starts at the
    /// beginning of the record and uses the same UTF-8 boundary machinery as
    /// bounded evidence extraction.
    pub fn preview_text(&self, config: &PreviewConfig) -> RecordResult<PreviewRecord> {
        config.validate()?;

        let text = String::from_utf8_lossy(&self.bytes);
        let bounded = bounded_text_window_from_start(&text, config.target_bytes);

        Ok(PreviewRecord::new(
            self.index,
            vec![PreviewElement {
                name: "text".into(),
                text: bounded.to_owned(),
            }],
        ))
    }

    pub fn decode<T: DeserializeOwned>(&self) -> RecordResult<T> {
        quick_xml::de::from_reader(self.bytes.as_slice()).map_err(RecordError::decode)
    }
}

fn collect_element_text(
    reader: &mut Reader<&[u8]>,
    buffer: &mut Vec<u8>,
) -> RecordResult<String> {
    let mut depth = 1usize;
    let mut text = String::new();

    loop {
        buffer.clear();
        match reader.read_event_into(buffer)? {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Text(event) => {
                text.push_str(&String::from_utf8_lossy(event.as_ref()));
            }
            Event::CData(event) => {
                text.push_str(&String::from_utf8_lossy(event.as_ref()));
            }
            Event::Empty(_) => {}
            Event::Eof => {
                return Err(RecordError::InvalidConfiguration(
                    "unexpected end of input while building record preview".into(),
                ));
            }
            _ => {}
        }
    }

    Ok(text)
}

/// Return a bounded UTF-8-safe window around a match.
///
/// This is the boundary algorithm used by bounded evidence extraction. The
/// preview path uses the same mechanics with its anchor shifted to the start
/// of the text rather than around a query match.
pub(crate) fn bounded_text_window_around_match(
    text: &str,
    match_start: usize,
    match_end: usize,
    budget: usize,
) -> &str {
    if budget == 0 {
        return "";
    }

    let match_bytes = match_end - match_start;
    if match_bytes >= budget {
        return &text[match_start..match_end];
    }

    let remaining = budget - match_bytes;
    let left_budget = remaining / 2;
    let right_budget = remaining - left_budget;

    let mut start = floor_char_boundary(text, match_start.saturating_sub(left_budget));
    let mut end = ceil_char_boundary(
        text,
        match_end.saturating_add(right_budget).min(text.len()),
    );

    while end - start > budget {
        if end > match_end {
            end = floor_char_boundary(text, end - 1);
        } else if start < match_start {
            start = ceil_char_boundary(text, start + 1);
        } else {
            break;
        }
    }

    &text[start..end]
}

pub(crate) fn bounded_text_window_from_start(text: &str, budget: usize) -> &str {
    if budget == 0 || text.is_empty() {
        return "";
    }

    let end = ceil_char_boundary(text, budget.min(text.len()));
    &text[..end]
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
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

#[cfg(test)]
mod tests {
    use super::{DatasetRecord, PreviewConfig};

    fn preview(xml: &str, config: PreviewConfig) -> super::PreviewRecord {
        let record = DatasetRecord::new(0, xml.as_bytes().to_vec());
        record.preview_xml(&config).unwrap()
    }

    #[test]
    fn preview_projects_named_child_text() {
        let config = PreviewConfig::new(4096, 10);
        let result = preview(
            "<article><author><name>Alice</name></author><title>Important title</title></article>",
            config,
        );

        assert_eq!(result.elements[0].name, "author");
        assert_eq!(result.elements[0].text, "Alice");
        assert_eq!(result.elements[1].name, "title");
        assert_eq!(result.elements[1].text, "Important title");
    }

    #[test]
    fn preview_stops_at_target_size() {
        let config = PreviewConfig::new(8, 10);
        let result = preview(
            "<article><a>one</a><b>two</b><c>three</c></article>",
            config,
        );

        assert_eq!(result.elements.len(), 2);
        assert_eq!(result.elements[0].text, "one");
        assert_eq!(result.elements[1].text, "two");
    }

    #[test]
    fn preview_includes_stop_element() {
        let config = PreviewConfig::new(4096, 10).with_stop_element("title");
        let result = preview(
            "<article><author>one</author><author>two</author><title>Important title</title><year>2026</year></article>",
            config,
        );

        assert_eq!(result.elements.last().unwrap().name, "title");
        assert_eq!(result.elements.last().unwrap().text, "Important title");
        assert!(!result.elements.iter().any(|element| element.name == "year"));
    }

    #[test]
    fn preview_stops_at_child_limit_when_stop_element_is_missing() {
        let config = PreviewConfig::new(4096, 2);
        let result = preview(
            "<article><a>one</a><b>two</b><c>three</c></article>",
            config,
        );

        assert_eq!(result.elements.len(), 2);
        assert_eq!(result.elements[0].text, "one");
        assert_eq!(result.elements[1].text, "two");
    }

    #[test]
    fn preview_flattens_nested_text() {
        let config = PreviewConfig::new(4096, 1);
        let result = preview(
            "<article><author><name>Alice</name><orcid>123</orcid></author><title>Title</title></article>",
            config,
        );

        assert_eq!(result.elements.len(), 1);
        assert_eq!(result.elements[0].name, "author");
        assert_eq!(result.elements[0].text, "Alice123");
    }

    #[test]
    fn preview_handles_empty_child() {
        let config = PreviewConfig::new(4096, 2);
        let result = preview("<article><author/><title>Title</title></article>", config);

        assert_eq!(result.elements[0].name, "author");
        assert!(result.elements[0].text.is_empty());
    }
}
