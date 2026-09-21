use crate::{DatasetRecord, RecordResult, RecordStream};

/// Re-openable source of dataset record streams.
///
/// Dataset operations may need to start from the beginning of a dataset.
/// A factory keeps that lifecycle concern outside the record-stream abstraction.
pub trait RecordSource {
    fn open(&self) -> RecordResult<Box<dyn RecordStream>>;
}

impl<F> RecordSource for F
where
    F: Fn() -> RecordResult<Box<dyn RecordStream>>,
{
    fn open(&self) -> RecordResult<Box<dyn RecordStream>> {
        self()
    }
}

/// Dataset access operations built on top of a re-openable record source.
///
/// The engine deliberately exposes mechanical operations rather than semantic
/// search behavior:
///
/// - `get` retrieves one record by zero-based index;
/// - `find` returns record indexes whose serialized records contain a text match;
/// - `list` enumerates records in a range, or all records when no range is given.
pub struct DatasetEngine<S> {
    source: S,
}

impl<S: RecordSource> DatasetEngine<S> {
    pub fn new(source: S) -> Self {
        Self { source }
    }

    /// Get one record by its zero-based record index.
    ///
    /// This operation opens a fresh stream and walks forward until the requested
    /// index is reached. Random-access indexing can be added behind this API later
    /// without changing its caller-facing contract.
    pub fn get(&self, index: u64) -> RecordResult<Option<DatasetRecord>> {
        let mut stream = self.source.open()?;

        while let Some(record) = stream.next_record()? {
            if record.index() == index {
                return Ok(Some(record));
            }
        }

        Ok(None)
    }

    /// Find record indexes whose serialized bytes contain `query`.
    ///
    /// Matching is intentionally simple and case-sensitive. An optional limit
    /// stops the scan after the requested number of matches.
    pub fn find(&self, query: &str, limit: Option<usize>) -> RecordResult<Vec<u64>> {
        if query.is_empty() {
            return Err(crate::RecordError::InvalidConfiguration(
                "find query cannot be empty".into(),
            ));
        }

        let needle = query.as_bytes();
        let mut stream = self.source.open()?;
        let mut matches = Vec::new();

        while let Some(record) = stream.next_record()? {
            if record.as_bytes().windows(needle.len()).any(|window| window == needle) {
                matches.push(record.index());

                if let Some(limit) = limit {
                    if matches.len() >= limit {
                        break;
                    }
                }
            }
        }

        Ok(matches)
    }

    /// List records in `range`, or all records when `range` is `None`.
    ///
    /// Ranges follow normal Rust half-open semantics, so `0..10` returns
    /// records 0 through 9.
    pub fn list(&self, range: Option<std::ops::Range<u64>>) -> RecordResult<Vec<DatasetRecord>> {
        let mut stream = self.source.open()?;
        let (start, end) = match range {
            Some(range) => (range.start, Some(range.end)),
            None => (0, None),
        };

        let mut records = Vec::new();

        while let Some(record) = stream.next_record()? {
            if record.index() < start {
                continue;
            }

            if let Some(end) = end {
                if record.index() >= end {
                    break;
                }
            }

            records.push(record);
        }

        Ok(records)
    }
}
