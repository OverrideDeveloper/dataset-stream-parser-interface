use bzip2::bufread::MultiBzDecoder;
use dataset_stream_parser_interface::{
    Checkpoint, DatasetEngine, DirectoryRecordSource, PreviewConfig, PreviewRecord, RecordSource, RecordStream,
    TextRecordStream, TextStreamConfig, XmlRecordStream, XmlStreamConfig,
};
use dataset_stream_parser_interface::manip::{getfragment_text, searchtogetfragment, FragmentRecord};
use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, Write};

struct FileRecordSource {
    path: String,
    record_element: String,
    text: bool,
    save_path: RefCell<Option<String>>,
}

struct TeeReader<R, W> {
    reader: R,
    writer: W,
}

impl<R: Read, W: Write> Read for TeeReader<R, W> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.reader.read(buffer)?;
        if count > 0 {
            self.writer.write_all(&buffer[..count])?;
        }
        Ok(count)
    }
}

impl FileRecordSource {
    fn set_save_path(&self, path: String) {
        *self.save_path.borrow_mut() = Some(path);
    }

    fn open_input(
        &self,
        position: Option<u64>,
        record_index: u64,
        allow_save: bool,
    ) -> Result<Box<dyn RecordStream>, Box<dyn std::error::Error>> {
        let mut file = File::open(&self.path)?;
        let compressed = self.path.to_ascii_lowercase().ends_with(".bz2");
        let start_index = if compressed && position.is_some() {
            0
        } else {
            record_index
        };

        if let Some(position) = position {
            if !compressed {
                file.seek(std::io::SeekFrom::Start(position))?;
            }
        }

        let input: Box<dyn BufRead> = if compressed {
            let decoder = MultiBzDecoder::new(BufReader::new(file));
            if allow_save {
                if let Some(save_path) = self.save_path.borrow_mut().take() {
                    let partial_path = format!("{save_path}.partial");
                    let output = File::create(partial_path)?;
                    Box::new(BufReader::new(TeeReader {
                        reader: decoder,
                        writer: output,
                    }))
                } else {
                    Box::new(BufReader::new(decoder))
                }
            } else {
                Box::new(BufReader::new(decoder))
            }
        } else {
            Box::new(BufReader::new(file))
        };

        if self.text {
            Ok(Box::new(TextRecordStream::new(
                input,
                TextStreamConfig::default(),
            )?.with_record_index(start_index)))
        } else {
            Ok(Box::new(XmlRecordStream::new(
                input,
                XmlStreamConfig::new(self.record_element.clone()),
            )?.with_record_index(start_index)))
        }
    }
}

impl dataset_stream_parser_interface::RecordSource for FileRecordSource {
    fn fragment(
        &self,
        record: &dataset_stream_parser_interface::DatasetRecord,
        query: &str,
        max_bytes: usize,
    ) -> dataset_stream_parser_interface::RecordResult<Option<FragmentRecord>> {
        if self.text {
            getfragment_text(record, query, max_bytes)
        } else {
            dataset_stream_parser_interface::manip::getfragment(record, query, max_bytes)
        }
    }

    fn preview(
        &self,
        record: &dataset_stream_parser_interface::DatasetRecord,
        config: &PreviewConfig,
    ) -> dataset_stream_parser_interface::RecordResult<PreviewRecord> {
        if self.text {
            record.preview_text(config)
        } else {
            record.preview_xml(config)
        }
    }

    fn open(&self) -> dataset_stream_parser_interface::RecordResult<Box<dyn RecordStream>> {
        self.open_input(None, 0, true).map_err(|error| {
            dataset_stream_parser_interface::RecordError::Io(std::io::Error::other(error.to_string()))
        })
    }

    fn open_from(
        &self,
        position: u64,
        record_index: u64,
    ) -> dataset_stream_parser_interface::RecordResult<Box<dyn RecordStream>> {
        self.open_input(Some(position), record_index, false).map_err(|error| {
            dataset_stream_parser_interface::RecordError::Io(std::io::Error::other(error.to_string()))
        })
    }
}



impl dataset_stream_parser_interface::RecordSource for &FileRecordSource {
    fn fragment(
        &self,
        record: &dataset_stream_parser_interface::DatasetRecord,
        query: &str,
        max_bytes: usize,
    ) -> dataset_stream_parser_interface::RecordResult<Option<FragmentRecord>> {
        (*self).fragment(record, query, max_bytes)
    }

    fn preview(
        &self,
        record: &dataset_stream_parser_interface::DatasetRecord,
        config: &PreviewConfig,
    ) -> dataset_stream_parser_interface::RecordResult<PreviewRecord> {
        (*self).preview(record, config)
    }

    fn open(&self) -> dataset_stream_parser_interface::RecordResult<Box<dyn RecordStream>> {
        (*self).open()
    }

    fn open_from(
        &self,
        position: u64,
        record_index: u64,
    ) -> dataset_stream_parser_interface::RecordResult<Box<dyn RecordStream>> {
        (*self).open_from(position, record_index)
    }
}

enum CliRecordSource {
    File(FileRecordSource),
    Directory(DirectoryRecordSource),
}

impl CliRecordSource {
    fn configure_save(&self, path: &str) {
        if let Self::File(source) = self {
            source.set_save_path(path.to_string());
        }
    }
}

impl RecordSource for CliRecordSource {
    fn fragment(&self, record: &dataset_stream_parser_interface::DatasetRecord, query: &str, max_bytes: usize) -> dataset_stream_parser_interface::RecordResult<Option<FragmentRecord>> {
        match self {
            Self::File(source) => source.fragment(record, query, max_bytes),
            Self::Directory(source) => source.fragment(record, query, max_bytes),
        }
    }

    fn preview(&self, record: &dataset_stream_parser_interface::DatasetRecord, config: &PreviewConfig) -> dataset_stream_parser_interface::RecordResult<PreviewRecord> {
        match self {
            Self::File(source) => source.preview(record, config),
            Self::Directory(source) => source.preview(record, config),
        }
    }

    fn open(&self) -> dataset_stream_parser_interface::RecordResult<Box<dyn RecordStream>> {
        match self {
            Self::File(source) => source.open(),
            Self::Directory(source) => source.open(),
        }
    }

    fn open_from(&self, position: u64, record_index: u64) -> dataset_stream_parser_interface::RecordResult<Box<dyn RecordStream>> {
        match self {
            Self::File(source) => source.open_from(position, record_index),
            Self::Directory(source) => source.open_from(position, record_index),
        }
    }

    fn initial_checkpoints(&self) -> Vec<Checkpoint> {
        match self {
            Self::File(source) => source.initial_checkpoints(),
            Self::Directory(source) => source.initial_checkpoints(),
        }
    }
}

fn configure_save(source: &CliRecordSource, path: &str) {
    source.configure_save(path);
}

fn print_record(record: &dataset_stream_parser_interface::DatasetRecord) {
    println!("#{}:", record.index());
    println!("{}", String::from_utf8_lossy(record.as_bytes()));
}

fn print_preview(record: &PreviewRecord) {
    println!("#{} ({} bytes):", record.index(), record.len());
    for element in record.elements() {
        println!("  <{}>: {}", element.name, element.text);
    }
}

fn print_help() {
    println!("Commands:");
    println!("  prep [interval] [--save] Prepare previews; optionally set checkpoint spacing and save a decompressed .bz2 corpus");
    println!("  showpreptable [index] Show prepared preview metadata or one preview");
    println!("  searchpreptable <text> [limit] Search only the prepared preview table");
    println!("  spotonpreptable <text> [limit] Search and display matching previews");
    println!("  clearpreptable        Release the prepared preview table");
    println!("  find <text> [limit]   Find whole-word/phrase matches");
    println!("  get <index>           Retrieve one record by index");
    println!("  getfragment <index> <text> [max-bytes]  Extract bounded evidence around a match");
    println!("  searchtogetfragment <text> [record-limit] [max-bytes]  Search and extract bounded evidence");
    println!("  list <start>..<end>   List a half-open range, e.g. list 0..10");
    println!("  list *                List every record (use with care)");
    println!("  help                  Show this help");
    println!("  quit                  Exit");
}

fn parse_range(value: &str) -> Option<std::ops::Range<u64>> {
    let (start, end) = value.split_once("..")?;
    Some(start.parse().ok()?..end.parse().ok()?)
}

fn parse_search_args(value: &str) -> Option<(&str, Option<usize>)> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let value = value.trim_matches('"');
    if let Some((query, limit)) = value.rsplit_once(' ') {
        if let Ok(limit) = limit.parse::<usize>() {
            return Some((query.trim_matches('"').trim(), Some(limit)));
        }
    }

    Some((value, None))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args();
    let program = args.next().unwrap_or_else(|| "dataset-stream-parser-cli".into());
    let path = args.next().ok_or_else(|| {
        format!("usage: {program} <dataset-folder|dataset.xml|dataset.xml.bz2|dataset.txt|dataset.txt.bz2> [record-element]")
    })?;
    let record_element = args.next().unwrap_or_else(|| "page".into());
    let input_path = std::path::Path::new(&path);
    let is_directory = input_path.is_dir();

    let source = if is_directory {
        if args.next().is_some() {
            return Err("record-element is not used with folder datasets".into());
        }
        CliRecordSource::Directory(DirectoryRecordSource::new(input_path)?)
    } else {
        let lowercase_path = path.to_ascii_lowercase();
        let dataset_path = lowercase_path.strip_suffix(".bz2").unwrap_or(&lowercase_path);
        let text = dataset_path.ends_with(".txt");

        CliRecordSource::File(FileRecordSource {
            path: path.clone(),
            record_element: record_element.clone(),
            text,
            save_path: RefCell::new(None),
        })
    };
    let is_compressed_file = !is_directory && path.to_ascii_lowercase().ends_with(".bz2");
    let mut engine = DatasetEngine::new(&source);

    println!("Dataset CLI");
    println!("  dataset: {path}");
    if is_directory {
        println!("  format:  flat TXT folder");
        println!("  record:  paragraph / bounded text chunk");
    } else {
        let lowercase_path = path.to_ascii_lowercase();
        let dataset_path = lowercase_path.strip_suffix(".bz2").unwrap_or(&lowercase_path);
        if dataset_path.ends_with(".txt") {
            println!("  format:  plain text");
            println!("  record:  paragraph / bounded text chunk");
        } else {
            println!("  format:  XML");
            println!("  record:  <{record_element}>");
        }
    }
    println!("Type 'help' for commands.");

    let stdin = io::stdin();
    let mut input = String::new();

    loop {
        print!("> ");
        io::stdout().flush()?;
        input.clear();

        if stdin.read_line(&mut input)? == 0 {
            break;
        }

        let command = input.trim();
        if command.is_empty() {
            continue;
        }

        if command == "quit" || command == "exit" {
            break;
        }

        if command == "help" {
            print_help();
            continue;
        }

        if command == "prep" || command.starts_with("prep ") {
            let mut interval = None;
            let mut save = false;
            let mut invalid = false;

            for value in command
                .strip_prefix("prep")
                .unwrap_or("")
                .split_whitespace()
            {
                if value == "--save" {
                    save = true;
                } else if interval.is_none() {
                    interval = value.parse::<u64>().ok();
                    if interval.is_none() {
                        invalid = true;
                    }
                } else {
                    invalid = true;
                }
            }

            if invalid {
                eprintln!("usage: prep [interval] [--save]");
                continue;
            }

            if save && !is_compressed_file {
                eprintln!("error: --save is only valid for .bz2 datasets");
                continue;
            }

            let save_path = if save {
                let target = path[..path.len() - 4].to_string();
                let partial = format!("{target}.partial");

                if std::path::Path::new(&target).exists() {
                    eprintln!("error: save target already exists: {target}");
                    continue;
                }
                if std::path::Path::new(&partial).exists() {
                    eprintln!("error: partial save already exists: {partial}");
                    continue;
                }

                Some(target)
            } else {
                None
            };

            if let Some(interval) = interval {
                if let Err(error) = engine.set_checkpoint_interval(interval) {
                    eprintln!("error: {error:?}");
                    continue;
                }
                println!("Checkpoint interval: {interval} records");
            }

            if let Some(target) = &save_path {
                configure_save(&source, target);
                println!("Saving decompressed corpus to: {target}");
            }

            if is_directory {
                println!("Preparing bounded text previews (target 4 KiB, starting at the beginning of each record)...");
            } else {
                println!("Preparing bounded named-text previews (target 4 KiB, stop at <title>, max 10 children)...");
            }
            let result = engine.prep_with_progress(|count| {
                if count % 10_000 == 0 {
                    print!("\rPrepared {count} records...");
                    let _ = io::stdout().flush();
                }
            });

            match result {
                Ok(count) => {
                    if let Some(target) = save_path {
                        let partial = format!("{target}.partial");
                        std::fs::rename(&partial, &target)?;
                        println!("Decompressed corpus saved: {target}");
                    }
                    print!("\rPrepared {count} records.\n");
                    println!("Search cache ready.");
                }
                Err(error) => {
                    if let Some(target) = save_path {
                        let partial = format!("{target}.partial");
                        let _ = std::fs::remove_file(partial);
                    }
                    eprintln!("\nerror: {error:?}");
                }
            }
            continue;
        }

        if command == "showpreptable" || command.starts_with("showpreptable ") {
            if !engine.is_prepared() {
                eprintln!("Preview table is not prepared. Run 'prep' first.");
                continue;
            }

            let value = command.strip_prefix("showpreptable").unwrap().trim();
            if value.is_empty() {
                let count = engine.prepared_preview_count();
                let config = engine.preview_config();
                println!("Preview table: {count} records");
                println!(
                    "  target: {} bytes, stop element: {}, max children: {}",
                    config.target_bytes,
                    config.stop_element.as_deref().unwrap_or("<none>"),
                    config.max_children
                );
                let sample_count = count.min(3);
                if sample_count > 0 {
                    println!("Sample previews:");
                    for index in 0..sample_count {
                        if let Some(record) = engine.preview_at(index as u64) {
                            print_preview(record);
                        }
                    }
                }
                continue;
            }

            match value.parse::<u64>() {
                Ok(index) => match engine.preview_at(index) {
                    Some(record) => print_preview(record),
                    None => println!("preview for record #{index} not found"),
                },
                Err(_) => eprintln!("usage: showpreptable [index]"),
            }
            continue;
        }

        if let Some(rest) = command.strip_prefix("searchpreptable ") {
            let Some((query, limit)) = parse_search_args(rest) else {
                eprintln!("usage: searchpreptable <text> [limit]");
                continue;
            };

            match engine.search_preview_indexes(query, limit) {
                Ok(matches) => println!("{matches:?}"),
                Err(error) => eprintln!("error: {error:?}"),
            }
            continue;
        }

        if command == "searchpreptable" {
            eprintln!("usage: searchpreptable <text> [limit]");
            continue;
        }

        if let Some(rest) = command.strip_prefix("spotonpreptable ") {
            let Some((query, limit)) = parse_search_args(rest) else {
                eprintln!("usage: spotonpreptable <text> [limit]");
                continue;
            };

            match engine.search_previews(query, limit) {
                Ok(matches) => {
                    for record in &matches {
                        print_preview(record);
                    }
                }
                Err(error) => eprintln!("error: {error:?}"),
            }
            continue;
        }

        if command == "spotonpreptable" {
            eprintln!("usage: spotonpreptable <text> [limit]");
            continue;
        }

        if command == "clearpreptable" {
            if engine.is_prepared() {
                engine.clear_previews();
                println!("Preview table cleared.");
            } else {
                println!("Preview table is not prepared.");
            }
            continue;
        }

        if let Some(rest) = command.strip_prefix("find ") {
            let Some((query, limit)) = parse_search_args(rest) else {
                eprintln!("usage: find <text> [limit]");
                continue;
            };

            match engine.find(query, limit) {
                Ok(matches) => println!("{matches:?}"),
                Err(error) => eprintln!("error: {error:?}"),
            }
            continue;
        }

        if let Some(rest) = command.strip_prefix("getfragment ") {
            let mut parts = rest.trim().splitn(2, ' ');
            let Some(index_text) = parts.next() else {
                eprintln!("usage: getfragment <index> <text> [max-bytes]");
                continue;
            };
            let Some(query_and_limit) = parts.next() else {
                eprintln!("usage: getfragment <index> <text> [max-bytes]");
                continue;
            };

            let Ok(index) = index_text.parse::<u64>() else {
                eprintln!("usage: getfragment <index> <text> [max-bytes]");
                continue;
            };

            let (query, limit) = match parse_search_args(query_and_limit) {
                Some((query, limit)) => (query, limit.unwrap_or(4096)),
                None => {
                    eprintln!("usage: getfragment <index> <text> [max-bytes]");
                    continue;
                }
            };

            match engine.get_fragment(index, query, limit) {
                Ok(Some(fragment)) => {
                    println!("#{} ({} bytes):", fragment.index, fragment.record.len());
                    println!("{}", fragment.record);
                }
                Ok(None) => println!("no match for query in record #{index}"),
                Err(error) => eprintln!("error: {error:?}"),
            }
            continue;
        }

        if let Some(rest) = command.strip_prefix("searchtogetfragment ") {
            let value = rest.trim();
            if value.is_empty() {
                eprintln!("usage: searchtogetfragment <text> [record-limit] [max-bytes]");
                continue;
            }

            let mut parts = value.split_whitespace().collect::<Vec<_>>();
            let mut numeric_suffix = Vec::new();

            while let Some(last) = parts.last().and_then(|value| value.parse::<usize>().ok()) {
                numeric_suffix.push(last);
                parts.pop();
                if numeric_suffix.len() == 2 {
                    break;
                }
            }

            let (record_limit, max_bytes) = match numeric_suffix.as_slice() {
                [limit] => (Some(*limit), None),
                [max_bytes, record_limit] => (Some(*record_limit), Some(*max_bytes)),
                [] => (None, None),
                _ => unreachable!(),
            };

            let query = parts.join(" ").trim_matches('"').trim().to_string();
            if query.is_empty() {
                eprintln!("usage: searchtogetfragment <text> [record-limit] [max-bytes]");
                continue;
            }

            match searchtogetfragment(&engine, &query, record_limit, max_bytes) {
                Ok(matches) => {
                    if matches.is_empty() {
                        println!("no fragments found for query: {query}");
                    } else {
                        for fragment in &matches {
                            println!("#{} ({} bytes):", fragment.index, fragment.record.len());
                            println!("{}", fragment.record);
                        }
                    }
                }
                Err(error) => eprintln!("error: {error:?}"),
            }
            continue;
        }

        if let Some(rest) = command.strip_prefix("get ") {
            match rest.trim().parse::<u64>() {
                Ok(index) => match engine.get(index) {
                    Ok(Some(record)) => print_record(&record),
                    Ok(None) => println!("record #{index} not found"),
                    Err(error) => eprintln!("error: {error:?}"),
                },
                Err(_) => eprintln!("usage: get <index>"),
            }
            continue;
        }

        if let Some(rest) = command.strip_prefix("list ") {
            let value = rest.trim();

            if value == "*" {
                println!("Listing all records; this materializes the entire result set.");
                match engine.list(None) {
                    Ok(records) => {
                        for record in &records {
                            print_record(record);
                        }
                    }
                    Err(error) => eprintln!("error: {error:?}"),
                }
                continue;
            }

            match parse_range(value) {
                Some(range) => match engine.list(Some(range)) {
                    Ok(records) => {
                        for record in &records {
                            print_record(record);
                        }
                    }
                    Err(error) => eprintln!("error: {error:?}"),
                },
                None => eprintln!("usage: list <start>..<end> or list *"),
            }
            continue;
        }

        eprintln!("unknown command; type 'help'");
    }

    Ok(())
}
