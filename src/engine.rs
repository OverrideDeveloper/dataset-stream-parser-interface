use crate::{DatasetRecord, LoadedRecord, PreviewConfig, RecordError, RecordResult, RecordStream};

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
    preview_config: PreviewConfig,
}

impl<S: RecordSource> DatasetEngine<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            loaded: None,
            checkpoints: Vec::new(),
            checkpoint_interval: 1_000,
            preview_config: PreviewConfig::default(),
        }
    }

    /// Configure the number of records between retrieval checkpoints.
    pub fn with_checkpoint_interval(mut self, interval: u64) -> RecordResult<Self> {
        self.set_checkpoint_interval(interval)?;
        Ok(self)
    }

    /// Set the number of records between retrieval checkpoints.
    pub fn set_checkpoint_interval(&mut self, interval: u64) -> RecordResult<()> {
        if interval == 0 {
            return Err(RecordError::InvalidConfiguration(
                "checkpoint interval must be greater than zero".into(),
            ));
        }
        self.checkpoint_interval = interval;
        Ok(())
    }

    /// Return the configured number of records between retrieval checkpoints.
    pub fn checkpoint_interval(&self) -> u64 {
        self.checkpoint_interval
    }

    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// Configure the bounded XML preview used by preparation.
    pub fn with_preview_config(mut self, config: PreviewConfig) -> RecordResult<Self> {
        config.validate_for_engine()?;
        self.preview_config = config;
        Ok(self)
    }

    /// Set the bounded XML preview used by preparation.
    pub fn set_preview_config(&mut self, config: PreviewConfig) -> RecordResult<()> {
        config.validate_for_engine()?;
        self.preview_config = config;
        Ok(())
    }

    /// Return the current XML preview configuration.
    pub fn preview_config(&self) -> &PreviewConfig {
        &self.preview_config
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
            loaded.push(record.preview(&self.preview_config)?);
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

        let mut matches = Vec::new();

        if let Some(loaded) = &self.loaded {
            for record in loaded {
                if contains_whole_words(record.as_bytes(), query) {
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
            if contains_whole_words(record.as_bytes(), query) {
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

/// Return true when the query occurs as a complete word or phrase in the text.
///
/// Matching is case-insensitive. Unicode alphanumeric characters and '_' are
/// treated as word characters; punctuation and whitespace form boundaries.
fn contains_whole_words(text: &[u8], query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return false;
    }

    let text = String::from_utf8_lossy(text).to_lowercase();
    let query = query.to_lowercase();

    let mut search_start = 0usize;
    while let Some(relative_start) = text[search_start..].find(&query) {
        let start = search_start + relative_start;
        let end = start + query.len();

        let before_is_word = text[..start]
            .chars()
            .next_back()
            .is_some_and(is_word_character);
        let after_is_word = text[end..]
            .chars()
            .next()
            .is_some_and(is_word_character);

        if !before_is_word && !after_is_word {
            return true;
        }

        search_start = start + query.len();
    }

    false
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[cfg(test)]
mod tests {
    use super::contains_whole_words;

    #[test]
    fn find_matches_complete_words_case_insensitively() {
        assert!(contains_whole_words(b"Lua is a programming language", "lua"));
        assert!(contains_whole_words(b"A LUA interpreter", "lua"));
        assert!(!contains_whole_words(b"simulator-based", "lua"));
        assert!(!contains_whole_words(b"player", "lay"));
    }

    #[test]
    fn find_accepts_punctuation_as_word_boundaries() {
        assert!(contains_whole_words(b"Use (Lua), please.", "lua"));
        assert!(contains_whole_words(b"Lua-programming", "lua"));
    }

    #[test]
    fn find_matches_whole_phrases() {
        assert!(contains_whole_words(b"Lua programming language", "lua programming"));
        assert!(!contains_whole_words(b"Lua programmer", "lua programming"));
    }
}
