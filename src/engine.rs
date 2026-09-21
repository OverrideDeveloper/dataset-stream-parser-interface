use crate::{DatasetRecord, LoadedRecord, RecordError, RecordResult, RecordStream};

/// Re-openable source of dataset record streams.
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
pub struct DatasetEngine<S> {
    source: S,
    loaded: Option<Vec<LoadedRecord>>,
}

impl<S: RecordSource> DatasetEngine<S> {
    pub fn new(source: S) -> Self {
        Self { source, loaded: None }
    }

    /// Load lightweight previews into memory for fast repeated searches.
    ///
    /// Only the first three XML child elements of each record are retained.
    pub fn load(&mut self) -> RecordResult<usize> {
        self.load_with_progress(|_| {})
    }

    /// Load lightweight previews while reporting the number of records loaded.
    pub fn load_with_progress<F>(&mut self, mut progress: F) -> RecordResult<usize>
    where
        F: FnMut(usize),
    {
        let mut stream = self.source.open()?;
        let mut loaded = Vec::new();

        while let Some(record) = stream.next_record()? {
            loaded.push(record.preview()?);
            progress(loaded.len());
        }

        let count = loaded.len();
        self.loaded = Some(loaded);
        Ok(count)
    }

    /// Get one complete record by its zero-based record index.
    pub fn get(&self, index: u64) -> RecordResult<Option<DatasetRecord>> {
        let mut stream = self.source.open()?;

        while let Some(record) = stream.next_record()? {
            if record.index() == index {
                return Ok(Some(record));
            }
        }

        Ok(None)
    }

    /// Find against loaded previews when available; otherwise scan the stream.
    pub fn find(&self, query: &str, limit: Option<usize>) -> RecordResult<Vec<u64>> {
        if query.is_empty() {
            return Err(RecordError::InvalidConfiguration(
                "find query cannot be empty".into(),
            ));
        }

        let needle = query.as_bytes();
        let mut matches = Vec::new();

        if let Some(loaded) = &self.loaded {
            for record in loaded {
                if record.as_bytes().windows(needle.len()).any(|window| window == needle) {
                    matches.push(record.index());
                    if let Some(limit) = limit {
                        if matches.len() >= limit {
                            break;
                        }
                    }
                }
            }
            return Ok(matches);
        }

        let mut stream = self.source.open()?;
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

    /// List complete records in range, or all records when range is None.
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
