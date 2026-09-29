use crate::manip::{getfragment_text, FragmentRecord};
use crate::{
    Checkpoint, DatasetRecord, LoadedRecord, PreviewConfig, RecordError, RecordResult, RecordSource,
    RecordStream, TextRecordStream, TextStreamConfig,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const SIDECAR_VERSION: u32 = 1;
const DEFAULT_CHECKPOINT_INTERVAL: u64 = 100;

/// A contiguous range of logical records originating in one source text file.
///
/// The range is inclusive on both ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecordRange {
    pub start: u64,
    pub end: u64,
    pub filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DirectorySidecar {
    version: u32,
    ranges: Vec<FileRecordRange>,
    checkpoints: Vec<Checkpoint>,
}

/// A persistent dataset materialized from the flat `*.txt` files in a folder.
///
/// The source folder is enumerated once in deterministic filename order. Its
/// records are flattened into a sibling `folderName.txt` dataset and a
/// `folderName.sidecar.json` sidecar. The sidecar preserves source filename
/// ranges and retrieval checkpoints so DatasetEngine::get() can use the
/// persistent dataset without requiring prep().
pub struct DirectoryRecordSource {
    folder: PathBuf,
    dataset_path: PathBuf,
    sidecar_path: PathBuf,
    text_config: TextStreamConfig,
    checkpoint_interval: u64,
    ranges: Vec<FileRecordRange>,
    checkpoints: Vec<Checkpoint>,
}

impl DirectoryRecordSource {
    pub fn new(folder: impl Into<PathBuf>) -> RecordResult<Self> {
        Self::with_config(
            folder,
            TextStreamConfig::default(),
            DEFAULT_CHECKPOINT_INTERVAL,
        )
    }

    pub fn with_config(
        folder: impl Into<PathBuf>,
        text_config: TextStreamConfig,
        checkpoint_interval: u64,
    ) -> RecordResult<Self> {
        if checkpoint_interval == 0 {
            return Err(RecordError::InvalidConfiguration(
                "directory checkpoint interval must be greater than zero".into(),
            ));
        }

        let folder = folder.into();
        if !folder.is_dir() {
            return Err(RecordError::InvalidConfiguration(format!(
                "directory dataset source is not a directory: {}",
                folder.display()
            )));
        }

        let folder_name = folder.file_name().and_then(|name| name.to_str()).ok_or_else(|| {
            RecordError::InvalidConfiguration(format!(
                "directory dataset source has no usable folder name: {}",
                folder.display()
            ))
        })?;

        let parent = folder.parent().unwrap_or_else(|| Path::new("."));
        let dataset_path = parent.join(format!("{folder_name}.txt"));
        let sidecar_path = parent.join(format!("{folder_name}.sidecar.json"));

        let mut source = Self {
            folder,
            dataset_path,
            sidecar_path,
            text_config,
            checkpoint_interval,
            ranges: Vec::new(),
            checkpoints: Vec::new(),
        };

        if source.dataset_path.exists() && source.sidecar_path.exists() {
            source.load_sidecar()?;
        } else {
            source.materialize()?;
        }

        Ok(source)
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn dataset_path(&self) -> &Path {
        &self.dataset_path
    }

    pub fn sidecar_path(&self) -> &Path {
        &self.sidecar_path
    }

    pub fn file_ranges(&self) -> &[FileRecordRange] {
        &self.ranges
    }

    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    fn load_sidecar(&mut self) -> RecordResult<()> {
        let mut contents = String::new();
        File::open(&self.sidecar_path)?.read_to_string(&mut contents)?;

        let sidecar: DirectorySidecar = serde_json::from_str(&contents).map_err(|error| {
            RecordError::InvalidConfiguration(format!(
                "invalid directory dataset sidecar {}: {error}",
                self.sidecar_path.display()
            ))
        })?;

        if sidecar.version != SIDECAR_VERSION {
            return Err(RecordError::InvalidConfiguration(format!(
                "unsupported directory dataset sidecar version: {}",
                sidecar.version
            )));
        }

        self.ranges = sidecar.ranges;
        self.checkpoints = sidecar.checkpoints;
        Ok(())
    }

    fn materialize(&mut self) -> RecordResult<()> {
        let mut files = Vec::new();

        for entry in fs::read_dir(&self.folder)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let is_txt = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("txt"));

            if is_txt {
                files.push(path);
            }
        }

        files.sort_by(|left, right| {
            left.file_name()
                .unwrap_or_default()
                .cmp(right.file_name().unwrap_or_default())
        });

        let dataset_file = File::create(&self.dataset_path)?;
        let mut output = BufWriter::new(dataset_file);
        let mut bytes_written = 0u64;
        let mut next_index = 0u64;
        let mut ranges = Vec::new();
        let mut checkpoints = Vec::new();

        for path in files {
            let start_index = next_index;
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    RecordError::InvalidConfiguration(format!(
                        "text source file has no usable filename: {}",
                        path.display()
                    ))
                })?
                .to_owned();

            let input = BufReader::new(File::open(&path)?);
            let mut stream = TextRecordStream::new(input, self.text_config.clone())?
                .with_record_index(next_index);

            while let Some(record) = stream.next_record()? {
                output.write_all(record.as_bytes())?;
                output.write_all(b"\n\n")?;
                bytes_written += record.len() as u64 + 2;
                next_index += 1;

                if next_index % self.checkpoint_interval == 0 {
                    checkpoints.push(Checkpoint {
                        index: next_index,
                        position: bytes_written,
                    });
                }
            }

            if next_index > start_index {
                ranges.push(FileRecordRange {
                    start: start_index,
                    end: next_index - 1,
                    filename,
                });
            }
        }

        output.flush()?;

        let sidecar = DirectorySidecar {
            version: SIDECAR_VERSION,
            ranges,
            checkpoints,
        };

        let sidecar_json = serde_json::to_string_pretty(&sidecar).map_err(|error| {
            RecordError::InvalidConfiguration(format!(
                "could not serialize directory dataset sidecar: {error}"
            ))
        })?;

        let temporary_sidecar = self.sidecar_path.with_extension("sidecar.json.tmp");
        {
            let mut sidecar_file = File::create(&temporary_sidecar)?;
            sidecar_file.write_all(sidecar_json.as_bytes())?;
            sidecar_file.write_all(b"\n")?;
            sidecar_file.flush()?;
        }
        fs::rename(&temporary_sidecar, &self.sidecar_path)?;

        self.ranges = sidecar.ranges;
        self.checkpoints = sidecar.checkpoints;
        Ok(())
    }
}

impl RecordSource for DirectoryRecordSource {
    fn open(&self) -> RecordResult<Box<dyn RecordStream>> {
        let input = BufReader::new(File::open(&self.dataset_path)?);
        Ok(Box::new(TextRecordStream::new(
            input,
            self.text_config.clone(),
        )?))
    }

    fn initial_checkpoints(&self) -> Vec<Checkpoint> {
        self.checkpoints.clone()
    }

    fn preview(
        &self,
        record: &DatasetRecord,
        config: &PreviewConfig,
    ) -> RecordResult<LoadedRecord> {
        record.preview_text(config)
    }

    fn fragment(
        &self,
        record: &DatasetRecord,
        query: &str,
        max_bytes: usize,
    ) -> RecordResult<Option<FragmentRecord>> {
        getfragment_text(record, query, max_bytes)
    }

    fn open_from(
        &self,
        position: u64,
        record_index: u64,
    ) -> RecordResult<Box<dyn RecordStream>> {
        let mut input = BufReader::new(File::open(&self.dataset_path)?);
        input.seek(SeekFrom::Start(position))?;

        Ok(Box::new(
            TextRecordStream::new(input, self.text_config.clone())?
                .with_record_index(record_index),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::DirectoryRecordSource;
    use crate::{DatasetEngine, RecordSource, TextStreamConfig};

    fn temp_folder(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dataset-stream-parser-interface-{name}-{}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn materializes_flat_txt_files_in_filename_order_and_records_ranges() {
        let root = temp_folder("directory-ranges");
        std::fs::write(root.join("b.txt"), "B one\n\nB two\n").unwrap();
        std::fs::write(root.join("a.txt"), "A one\n\nA two\n").unwrap();
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("nested").join("ignored.txt"), "ignored").unwrap();
        std::fs::write(root.join("ignored.xml"), "ignored").unwrap();

        let source = DirectoryRecordSource::with_config(
            root.clone(),
            TextStreamConfig::default(),
            100,
        )
        .unwrap();

        assert_eq!(
            source
                .file_ranges()
                .iter()
                .map(|range| (&range.filename, range.start, range.end))
                .collect::<Vec<_>>(),
            vec![
                (&"a.txt".to_string(), 0, 1),
                (&"b.txt".to_string(), 2, 3),
            ]
        );

        let dataset = std::fs::read_to_string(source.dataset_path()).unwrap();
        assert!(dataset.contains("A one"));
        assert!(dataset.contains("B two"));
        assert!(!dataset.contains("ignored"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_checkpoints_make_get_available_without_prep() {
        let root = temp_folder("directory-get");
        std::fs::write(
            root.join("a.txt"),
            "A one\n\nA two\n\nA three\n\nA four\n",
        )
        .unwrap();

        let source = DirectoryRecordSource::with_config(
            &root,
            TextStreamConfig::default(),
            2,
        )
        .unwrap();

        let mut engine = DatasetEngine::new(source);
        assert!(!engine.is_prepared());

        let record = engine.get(2).unwrap().unwrap();
        assert_eq!(record.index(), 2);
        assert_eq!(String::from_utf8_lossy(record.as_bytes()), "A three\n");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_sidecar_is_rebuilt_automatically() {
        let root = temp_folder("directory-sidecar");
        std::fs::write(root.join("a.txt"), "A one\n\nA two\n").unwrap();

        let source = DirectoryRecordSource::new(root.clone()).unwrap();
        let sidecar = source.sidecar_path().to_path_buf();
        assert!(sidecar.exists());

        std::fs::remove_file(&sidecar).unwrap();
        let reloaded = DirectoryRecordSource::new(root.clone()).unwrap();

        assert!(reloaded.sidecar_path().exists());
        assert_eq!(reloaded.file_ranges().len(), 1);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_source_uses_text_preview_and_fragment_semantics() {
        let root = temp_folder("directory-text-semantics");
        std::fs::write(
            root.join("a.txt"),
            "The quick brown fox jumps over the lazy dog.\n",
        )
        .unwrap();

        let source = DirectoryRecordSource::new(root.clone()).unwrap();
        let mut engine = DatasetEngine::new(source);

        let previews = engine.prep().unwrap();
        assert_eq!(previews, 1);

        let preview = engine.preview_at(0).unwrap();
        assert_eq!(preview.elements()[0].name, "text");
        assert!(preview.elements()[0].text.contains("The quick"));

        let fragment = engine
            .get_fragment(0, "brown fox", 64)
            .unwrap()
            .unwrap();
        assert!(fragment.record.contains("brown fox"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
