use crate::{DatasetRecord, RecordError, RecordResult, RecordStream};
use quick_xml::events::Event;
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
pub struct XmlRecordStream<R> {
    reader: Reader<R>,
    config: XmlStreamConfig,
    buffer: Vec<u8>,
    record_index: u64,
}

impl<R: BufRead> XmlRecordStream<R> {
    pub fn new(source: R, config: XmlStreamConfig) -> RecordResult<Self> {
        if config.record_element.is_empty() {
            return Err(RecordError::InvalidConfiguration("record_element cannot be empty".into()));
        }
        if config.max_record_bytes == 0 {
            return Err(RecordError::InvalidConfiguration("max_record_bytes must be greater than zero".into()));
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

    fn collect_record(&mut self, start: quick_xml::events::BytesStart<'_>) -> RecordResult<Vec<u8>> {
        let mut record = Vec::new();
        record.extend_from_slice(start.as_ref());

        self.reader.read_to_end_into(start.name(), &mut self.buffer)?;

        record.extend_from_slice(&self.buffer);

        if record.len() > self.config.max_record_bytes {
            return Err(RecordError::InvalidConfiguration(format!(
                "record {} exceeded max_record_bytes ({})",
                self.record_index, self.config.max_record_bytes
            )));
        }

        self.buffer.clear();
        Ok(record)
    }
}

impl<R: BufRead> RecordStream for XmlRecordStream<R> {
    fn next_record(&mut self) -> RecordResult<Option<DatasetRecord>> {
        loop {
            self.buffer.clear();

            match self.reader.read_event_into(&mut self.buffer)? {
                Event::Start(start)
                    if start.name().as_ref() == self.config.record_element.as_slice() =>
                {
                    let bytes = self.collect_record(start)?;
                    let record = DatasetRecord::new(self.record_index, bytes);
                    self.record_index += 1;
                    return Ok(Some(record));
                }
                Event::Eof => return Ok(None),
                _ => {}
            }
        }
    }
}
