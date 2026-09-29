use crate::{manip::{getfragment, FragmentRecord}, DatasetRecord, LoadedRecord, PreviewConfig, RecordError, RecordResult, RecordStream};
use serde::{Deserialize, Serialize};

/// Re-openable source of dataset record streams.
pub trait RecordSource {
    fn open(&self) -> RecordResult<Box<dyn RecordStream>>;

    /// Return checkpoints persisted by the source itself, if any.
    ///
    /// These checkpoints make a persistent source directly retrievable without
    /// requiring `prep()` to be run first. The default is an empty set so
    /// existing sources retain their current behavior.
    fn initial_checkpoints(&self) -> Vec<Checkpoint> { Vec::new() }

    /// Build the prepared preview for one authoritative record.
    ///
    /// XML sources retain the historical structured preview by default. Other
    /// source types can override this when their records have different
    /// semantics, without making DatasetEngine format-aware.
    fn preview(&self, record: &DatasetRecord, config: &PreviewConfig) -> RecordResult<LoadedRecord> {
        record.preview_xml(config)
    }
    fn fragment(&self, record: &DatasetRecord, query: &str, max_bytes: usize) -> RecordResult<Option<FragmentRecord>> {
        getfragment(record, query, max_bytes)
    }

    fn open_from(&self, position: u64, record_index: u64) -> RecordResult<Box<dyn RecordStream>> {
        let _ = (position, record_index);
        self.open()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub index: u64,
    pub position: u64,
}

impl<F> RecordSource for F
where
    F: Fn() -> RecordResult<Box<dyn RecordStream>>,
{
    fn open(&self) -> RecordResult<Box<dyn RecordStream>> { self() }
}

pub struct DatasetEngine<S> {
    source: S,
    loaded: Option<Vec<LoadedRecord>>,
    checkpoints: Vec<Checkpoint>,
    checkpoint_interval: u64,
    preview_config: PreviewConfig,
}

impl<S: RecordSource> DatasetEngine<S> {
    pub fn new(source: S) -> Self {
        let checkpoints = source.initial_checkpoints();
        Self {
            source,
            loaded: None,
            checkpoints,
            checkpoint_interval: 100,
            preview_config: PreviewConfig::default(),
        }
    }

    pub fn with_checkpoint_interval(mut self, interval: u64) -> RecordResult<Self> {
        self.set_checkpoint_interval(interval)?;
        Ok(self)
    }

    pub fn set_checkpoint_interval(&mut self, interval: u64) -> RecordResult<()> {
        if interval == 0 {
            return Err(RecordError::InvalidConfiguration("checkpoint interval must be greater than zero".into()));
        }
        self.checkpoint_interval = interval;
        Ok(())
    }

    pub fn checkpoint_interval(&self) -> u64 { self.checkpoint_interval }
    pub fn checkpoints(&self) -> &[Checkpoint] { &self.checkpoints }
    pub fn source(&self) -> &S { &self.source }

    pub fn with_preview_config(mut self, config: PreviewConfig) -> RecordResult<Self> {
        config.validate()?;
        self.preview_config = config;
        Ok(self)
    }

    pub fn set_preview_config(&mut self, config: PreviewConfig) -> RecordResult<()> {
        config.validate()?;
        self.preview_config = config;
        Ok(())
    }

    pub fn preview_config(&self) -> &PreviewConfig { &self.preview_config }

    pub fn prep(&mut self) -> RecordResult<usize> {
        self.prep_with_progress(|_| {})
    }

    pub fn prep_with_progress<F>(&mut self, mut progress: F) -> RecordResult<usize>
    where
        F: FnMut(usize),
    {
        let mut stream = self.source.open()?;
        let mut loaded = Vec::new();
        let mut checkpoints = Vec::new();

        while let Some(record) = stream.next_record()? {
            loaded.push(self.source.preview(&record, &self.preview_config)?);
            let count = loaded.len();
            progress(count);

            if count as u64 % self.checkpoint_interval == 0 {
                if let Some(position) = stream.checkpoint_position() {
                    checkpoints.push(Checkpoint { index: count as u64, position });
                }
            }
        }

        let count = loaded.len();
        self.loaded = Some(loaded);
        self.checkpoints = checkpoints;
        Ok(count)
    }

    pub fn get(&self, index: u64) -> RecordResult<Option<DatasetRecord>> {
        let checkpoint = self.checkpoints.iter().rev().find(|checkpoint| checkpoint.index <= index);
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

    pub fn get_fragment(&self, index: u64, query: &str, max_bytes: usize) -> RecordResult<Option<FragmentRecord>> {
        match self.get(index)? {
            Some(record) => self.source.fragment(&record, query, max_bytes),
            None => Ok(None),
        }
    }

    pub fn is_prepared(&self) -> bool { self.loaded.is_some() }
    pub fn prepared_preview_count(&self) -> usize { self.loaded.as_ref().map_or(0, Vec::len) }

    pub fn preview_at(&self, index: u64) -> Option<&LoadedRecord> {
        self.loaded.as_ref().and_then(|loaded| loaded.iter().find(|record| record.index() == index))
    }

    /// Search the prepared preview table and return the matching record indexes.
    ///
    /// This is the index-only primitive used when callers want discovery without
    /// materializing the matching previews.
    pub fn search_preview_indexes(&self, query: &str, limit: Option<usize>) -> RecordResult<Vec<u64>> {
        let loaded = self.loaded.as_ref().ok_or_else(|| {
            RecordError::InvalidConfiguration("preview table is not prepared; run prep() first".into())
        })?;
        if query.is_empty() {
            return Err(RecordError::InvalidConfiguration("preview search query cannot be empty".into()));
        }

        let mut matches = Vec::new();
        for record in loaded {
            if record.elements().iter().any(|element| contains_whole_words(element.text.as_bytes(), query)) {
                matches.push(record.index());
                if let Some(limit) = limit {
                    if matches.len() >= limit { break; }
                }
            }
        }
        Ok(matches)
    }

    /// Search the prepared preview table and return the matching bounded previews.
    ///
    /// The returned records retain their authoritative dataset indexes.
    pub fn search_previews(&self, query: &str, limit: Option<usize>) -> RecordResult<Vec<LoadedRecord>> {
        let loaded = self.loaded.as_ref().ok_or_else(|| {
            RecordError::InvalidConfiguration("preview table is not prepared; run prep() first".into())
        })?;
        if query.is_empty() {
            return Err(RecordError::InvalidConfiguration("query cannot be empty".into()));
        }

        let mut matches = Vec::new();
        for record in loaded {
            if record.elements().iter().any(|element| contains_whole_words(element.text.as_bytes(), query)) {
                matches.push(record.clone());
                if let Some(limit) = limit {
                    if matches.len() >= limit { break; }
                }
            }
        }
        Ok(matches)
    }

    pub fn clear_previews(&mut self) { self.loaded = None; }

    pub fn find(&self, query: &str, limit: Option<usize>) -> RecordResult<Vec<u64>> {
        if query.is_empty() {
            return Err(RecordError::InvalidConfiguration("find query cannot be empty".into()));
        }

        let mut matches = Vec::new();
        if let Some(loaded) = &self.loaded {
            for record in loaded {
                if record.elements().iter().any(|element| contains_whole_words(element.text.as_bytes(), query)) {
                    matches.push(record.index());
                    if let Some(limit) = limit {
                        if matches.len() >= limit { break; }
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
                    if matches.len() >= limit { break; }
                }
            }
        }
        Ok(matches)
    }

    pub fn list(&self, range: Option<std::ops::Range<u64>>) -> RecordResult<Vec<DatasetRecord>> {
        let mut stream = self.source.open()?;
        let (start, end) = match range {
            Some(range) => (range.start, Some(range.end)),
            None => (0, None),
        };
        let mut records = Vec::new();

        while let Some(record) = stream.next_record()? {
            if record.index() < start { continue; }
            if let Some(end) = end {
                if record.index() >= end { break; }
            }
            records.push(record);
        }
        Ok(records)
    }
}

fn contains_whole_words(text: &[u8], query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() { return false; }

    let text = String::from_utf8_lossy(text).to_lowercase();
    let query = query.to_lowercase();
    let mut search_start = 0usize;

    while let Some(relative_start) = text[search_start..].find(&query) {
        let start = search_start + relative_start;
        let end = start + query.len();
        let before_is_word = text[..start].chars().next_back().is_some_and(is_word_character);
        let after_is_word = text[end..].chars().next().is_some_and(is_word_character);

        if !before_is_word && !after_is_word { return true; }
        search_start = start + query.len();
    }
    false
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[cfg(test)]
mod tests {
    #[test]
    fn new_uses_100_record_checkpoint_default() {
        use crate::{DatasetEngine, RecordResult, RecordStream};
        use std::io::Cursor;

        let engine = DatasetEngine::new(move || -> RecordResult<Box<dyn RecordStream>> {
            Ok(Box::new(crate::XmlRecordStream::new(
                Cursor::new(br#"<pages><page><title>First</title></page></pages>"#.to_vec()),
                crate::XmlStreamConfig::new("page"),
            )?))
        });

        assert_eq!(engine.checkpoint_interval(), 100);
    }

    #[test]
    fn search_previews_returns_bounded_records_with_indexes() {
        use crate::{DatasetEngine, RecordResult, RecordStream, XmlRecordStream, XmlStreamConfig};
        use std::io::Cursor;

        let xml = br#"<pages><page><title>First</title></page><page><title>Second</title></page><page><title>First Again</title></page></pages>"#.to_vec();
        let mut engine = DatasetEngine::new(move || -> RecordResult<Box<dyn RecordStream>> {
            Ok(Box::new(XmlRecordStream::new(
                Cursor::new(xml.clone()),
                XmlStreamConfig::new("page"),
            )?))
        });

        engine.prep().unwrap();

        let matches = engine.search_previews("First", Some(1)).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].index(), 0);
        assert_eq!(matches[0].elements()[0].name, "title");

        assert_eq!(engine.search_preview_indexes("First", None).unwrap(), vec![0, 2]);
    }


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
