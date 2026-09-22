use crate::{DatasetRecord, LoadedRecord, RecordError, RecordResult, RecordStream};

/// Re-openable source of dataset record streams.
pub trait RecordSource {
    fn open(&self) -> RecordResult<Box<dyn RecordStream>>;

    fn open_from(
        &self,
        position: u64,
        record_index: u64,
    ) -> RecordResult<Box<dyn RecordStream>> {
        let _ = (position, record_index);
        self.open()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint {
    pub index: u64,
    pub position: u64,
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
    checkpoints: Vec<Checkpoint>,
    checkpoint_interval: u64,
}

impl<S: RecordSource> DatasetEngine<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            loaded: None,
            checkpoints: Vec::new(),
            checkpoint_interval: 100_000,
        }
    }

    /// Configure the number of records between retrieval checkpoints.
    pub fn with_checkpoint_interval(mut self, interval: u64) -> RecordResult<Self> {
        if interval == 0 {
            return Err(RecordError::InvalidConfiguration(
                "checkpoint interval must be greater than zero".into(),
            ));
        }
        self.checkpoint_interval = interval;
        Ok(self)
    }

    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// Prepare lightweight previews in memory for fast repeated searches.
    ///
    /// Only the first three XML child elements of each record are retained.
    /// Calling this is optional; get() and list() continue to use the source
    /// directly when preparation has not been requested.
    pub fn prep(&mut self) -> RecordResult<usize> {
        self.prep_with_progress(|_| {})
    }

    /// Prepare lightweight previews while reporting the number of records prepared.
    pub fn prep_with_progress<F>(&mut self, mut progress: F) -> RecordResult<usize>
    where
        F: FnMut(usize),
    {
        let mut stream = self.source.open()?;
        let mut loaded = Vec::new();
        let mut checkpoints = Vec::new();

        while let Some(record) = stream.next_record()? {
            loaded.push(record.preview()?);
            let count = loaded.len();
            progress(count);

            if count as u64 % self.checkpoint_interval == 0 {
                if let Some(position) = stream.checkpoint_position() {
                    checkpoints.push(Checkpoint {
                        index: count as u64,
                        position,
                    });
                }
            }
        }

        let count = loaded.len();
        self.loaded = Some(loaded);
        self.checkpoints = checkpoints;
        Ok(count)
    }

    /// Get one complete record by its zero-based record index.
    ///
    /// When preparation produced checkpoints and the source supports seeking,
    /// retrieval starts at the nearest checkpoint.
    pub fn get(&self, index: u64) -> RecordResult<Option<DatasetRecord>> {
        let checkpoint = self
            .checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.index <= index);

        let mut stream = match checkpoint {
            Some(checkpoint) => self.source.open_from(checkpoint.position, checkpoint.index)?,
            None => self.source.open()?,
        };

        while let Some(record) = stream.next_record()? {
            if record.index() == index {
                return Ok(Some(record));
            }
        }

        Ok(None)
    }

    /// Find against prepared previews when available; otherwise scan the stream.
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
