use crate::{DatasetRecord, RecordError, RecordResult, RecordStream};
use quick_xml::events::{BytesStart, Event};
use quick_xml::writer::Writer;
use quick_xml::Reader;
use std::io::BufRead;

/// Configuration for an XML record stream.
#[derive(Debug, Clone)]
pub struct XmlStreamConfig {
    /// Element name that represents one meaningful dataset record.
    pub record_element: Vec<u8>,
    /// Maximum serialized size allowed for one bounded record.
    pub max_record_bytes: usize,
}

impl XmlStreamConfig {
    pub fn new(record_element: impl Into<Vec<u8>>) -> Self {
        Self {
            record_element: record_element.into(),
            max_record_bytes: 16 * 1024 * 1024,
        }
    }

    pub fn with_max_record_bytes(mut self, max_record_bytes: usize) -> Self {
        self.max_record_bytes = max_record_bytes;
        self
    }
}

/// Streaming XML record reader.
///
/// The reader walks the document until it finds the configured record
/// element, then materializes only that bounded subtree.
pub struct XmlRecordStream<R> {
    reader: Reader<R>,
    config: XmlStreamConfig,
    buffer: Vec<u8>,
    record_index: u64,
}

impl<R: BufRead> XmlRecordStream<R> {
    pub fn new(source: R, config: XmlStreamConfig) -> RecordResult<Self> {
        if config.record_element.is_empty() {
            return Err(RecordError::InvalidConfiguration(
                "record_element cannot be empty".into(),
            ));
        }
        if config.max_record_bytes == 0 {
            return Err(RecordError::InvalidConfiguration(
                "max_record_bytes must be greater than zero".into(),
            ));
        }

        let mut reader = Reader::from_reader(source);
        reader.config_mut().trim_text(false);

        Ok(Self {
            reader,
            config,
            buffer: Vec::new(),
            record_index: 0,
        })
    }

    fn append_event(record: &mut Vec<u8>, event: Event<'_>, max: usize) -> RecordResult<()> {
        Writer::new(&mut *record).write_event(event)?;

        if record.len() > max {
            return Err(RecordError::InvalidConfiguration(format!(
                "record exceeded max_record_bytes ({max})"
            )));
        }

        Ok(())
    }

    fn collect_record(&mut self, start: BytesStart<'static>) -> RecordResult<Vec<u8>> {
        let mut record = Vec::new();
        let max = self.config.max_record_bytes;
        let mut depth = 1usize;

        Self::append_event(&mut record, Event::Start(start), max)?;

        loop {
            self.buffer.clear();

            let event = self.reader.read_event_into(&mut self.buffer)?;

            match event {
                Event::Start(_) => {
                    depth += 1;
                    Self::append_event(&mut record, event, max)?;
                }
                Event::Empty(_) => {
                    Self::append_event(&mut record, event, max)?;
                }
                Event::End(_) => {
                    Self::append_event(&mut record, event, max)?;
                    depth -= 1;
                    if depth == 0 {
                        return Ok(record);
                    }
                }
                Event::Eof => {
                    return Err(RecordError::InvalidConfiguration(
                        "unexpected end of input inside record".into(),
                    ));
                }
                _ => {
                    Self::append_event(&mut record, event, max)?;
                }
            }
        }
    }
}

impl<R: BufRead> RecordStream for XmlRecordStream<R> {
    fn checkpoint_position(&self) -> Option<u64> {
        Some(self.reader.buffer_position() as u64)
    }

    fn next_record(&mut self) -> RecordResult<Option<DatasetRecord>> {
        loop {
            self.buffer.clear();

            let start = {
                let event = self.reader.read_event_into(&mut self.buffer)?;

                match event {
                    Event::Start(start)
                        if start.name().as_ref() == self.config.record_element.as_slice() =>
                    {
                        Some(start.into_owned())
                    }
                    Event::Eof => return Ok(None),
                    _ => None,
                }
            };

            if let Some(start) = start {
                let bytes = self.collect_record(start)?;
                let record = DatasetRecord::new(self.record_index, bytes);
                self.record_index += 1;
                return Ok(Some(record));
            }
        }
    }
}
