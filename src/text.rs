use crate::{DatasetRecord, RecordError, RecordResult, RecordStream};
use std::io::BufRead;

/// Configuration for a plain-text record stream.
///
/// Text is divided into paragraphs at blank lines. Each paragraph becomes a
/// record unless it is larger than the configured target size, in which case
/// it is split at whitespace boundaries rather than in the middle of a word.
#[derive(Debug, Clone)]
pub struct TextStreamConfig {
    /// Target UTF-8 byte size for a generated record.
    ///
    /// This is a target rather than an absolute maximum: an individual word
    /// longer than the target is kept intact.
    pub target_record_bytes: usize,
}

impl TextStreamConfig {
    pub fn new(target_record_bytes: usize) -> Self {
        Self { target_record_bytes }
    }
}

impl Default for TextStreamConfig {
    fn default() -> Self {
        Self {
            target_record_bytes: 4096,
        }
    }
}

/// Streaming plain-text record reader.
///
/// Blank lines delimit paragraphs. Each paragraph is a record boundary.
/// An oversized paragraph is divided at whitespace boundaries so generated
/// records remain useful text rather than arbitrary byte slices.
pub struct TextRecordStream<R> {
    reader: R,
    config: TextStreamConfig,
    record_index: u64,
    pending_records: Vec<String>,
    eof: bool,
}

impl<R: BufRead> TextRecordStream<R> {
    pub fn new(reader: R, config: TextStreamConfig) -> RecordResult<Self> {
        if config.target_record_bytes == 0 {
            return Err(RecordError::InvalidConfiguration(
                "text target_record_bytes must be greater than zero".into(),
            ));
        }

        Ok(Self {
            reader,
            config,
            record_index: 0,
            pending_records: Vec::new(),
            eof: false,
        })
    }

    pub fn with_record_index(mut self, record_index: u64) -> Self {
        self.record_index = record_index;
        self
    }

    fn next_paragraph(&mut self) -> RecordResult<Option<String>> {
        if self.eof {
            return Ok(None);
        }

        let mut paragraph = String::new();
        let mut line = String::new();

        loop {
            line.clear();
            let bytes_read = self.reader.read_line(&mut line)?;

            if bytes_read == 0 {
                self.eof = true;
                break;
            }

            if line.trim().is_empty() {
                if paragraph.is_empty() {
                    continue;
                }
                break;
            }

            paragraph.push_str(&line);
        }

        if paragraph.is_empty() {
            Ok(None)
        } else {
            Ok(Some(paragraph))
        }
    }

    fn split_paragraph(&self, paragraph: &str) -> Vec<String> {
        let target = self.config.target_record_bytes;

        if paragraph.len() <= target {
            return vec![paragraph.to_owned()];
        }

        let mut chunks = Vec::new();
        let mut start = 0usize;

        while start < paragraph.len() {
            let remaining = &paragraph[start..];

            if remaining.len() <= target {
                chunks.push(remaining.to_owned());
                break;
            }

            let target_end = start + target;
            let mut split_at = None;

            // Find the last whitespace boundary whose resulting chunk is
            // within the target. Iterate by char so target_end never becomes
            // an invalid UTF-8 slice boundary.
            for (offset, character) in paragraph[start..].char_indices() {
                let position = start + offset;
                let end = position + character.len_utf8();

                if end > target_end {
                    break;
                }

                if character.is_whitespace() {
                    split_at = Some(end);
                }
            }

            let split_at = match split_at {
                Some(position) if position > start => position,
                _ => {
                    // There is no whitespace before the target. Extend to the
                    // next whitespace so a long word is never split.
                    paragraph[start..]
                        .char_indices()
                        .find_map(|(offset, character)| {
                            character
                                .is_whitespace()
                                .then_some(start + offset + character.len_utf8())
                        })
                        .unwrap_or(paragraph.len())
                }
            };

            let chunk = paragraph[start..split_at].to_owned();
            if !chunk.is_empty() {
                chunks.push(chunk);
            }
            start = split_at;
        }

        chunks
    }

    fn next_chunk(&mut self) -> RecordResult<Option<String>> {
        if let Some(record) = self.pending_records.pop() {
            return Ok(Some(record));
        }

        let paragraph = match self.next_paragraph()? {
            Some(paragraph) => paragraph,
            None => return Ok(None),
        };

        let chunks = self.split_paragraph(&paragraph);
        let mut chunks = chunks.into_iter();

        let first = match chunks.next() {
            Some(chunk) => chunk,
            None => return Ok(None),
        };

        // An oversized paragraph may produce multiple records. Queue the
        // remainder in reverse so the next call returns them in source order.
        self.pending_records.extend(chunks.rev());

        Ok(Some(first))
    }

}

impl<R: BufRead> RecordStream for TextRecordStream<R> {
    fn next_record(&mut self) -> RecordResult<Option<DatasetRecord>> {
        let text = match self.next_chunk()? {
            Some(text) => text,
            None => return Ok(None),
        };

        let record = DatasetRecord::new(self.record_index, text.into_bytes());
        self.record_index += 1;
        Ok(Some(record))
    }
}

#[cfg(test)]
mod tests {
    use super::{TextRecordStream, TextStreamConfig};
    use crate::RecordStream;
    use std::io::Cursor;

    fn records(text: &str, target: usize) -> Vec<String> {
        let mut stream = TextRecordStream::new(
            Cursor::new(text.as_bytes()),
            TextStreamConfig::new(target),
        )
        .unwrap();

        let mut records = Vec::new();
        while let Some(record) = stream.next_record().unwrap() {
            records.push(String::from_utf8(record.as_bytes().to_vec()).unwrap());
        }
        records
    }

    #[test]
    fn blank_lines_delimit_paragraphs() {
        let result = records("First paragraph.\n\nSecond paragraph.\n", 4096);

        assert_eq!(
            result,
            vec![
                "First paragraph.\n".to_string(),
                "Second paragraph.\n".to_string()
            ]
        );
    }

    #[test]
    fn oversized_paragraphs_split_at_whitespace() {
        let result = records("one two three four five six", 12);

        assert_eq!(
            result,
            vec![
                "one two ".to_string(),
                "three four ".to_string(),
                "five six".to_string()
            ]
        );
    }

    #[test]
    fn long_words_are_not_split() {
        let result = records("supercalifragilistic", 8);

        assert_eq!(result, vec!["supercalifragilistic".to_string()]);
    }

    #[test]
    fn utf8_words_are_not_split_in_the_middle_of_a_character() {
        let result = records("alpha café delta", 8);

        for record in result {
            assert!(std::str::from_utf8(record.as_bytes()).is_ok());
        }
    }

    #[test]
    fn record_indexes_are_sequential() {
        let mut stream = TextRecordStream::new(
            Cursor::new("one\n\ntwo\n".as_bytes()),
            TextStreamConfig::default(),
        )
        .unwrap()
        .with_record_index(7);

        assert_eq!(stream.next_record().unwrap().unwrap().index(), 7);
        assert_eq!(stream.next_record().unwrap().unwrap().index(), 8);
        assert!(stream.next_record().unwrap().is_none());
    }
}
