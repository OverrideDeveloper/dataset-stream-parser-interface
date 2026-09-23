use bzip2::bufread::MultiBzDecoder;
use dataset_stream_parser_interface::{
    DatasetEngine, PreviewRecord, RecordStream, XmlRecordStream, XmlStreamConfig,
};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, Write};

struct FileRecordSource {
    path: String,
    record_element: String,
}

impl FileRecordSource {
    fn open_input(
        &self,
        position: Option<u64>,
        record_index: u64,
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
            Box::new(BufReader::new(MultiBzDecoder::new(BufReader::new(file))))
        } else {
            Box::new(BufReader::new(file))
        };

        Ok(Box::new(XmlRecordStream::new(
            input,
            XmlStreamConfig::new(self.record_element.clone()),
        )?.with_record_index(start_index)))
    }
}

impl dataset_stream_parser_interface::RecordSource for FileRecordSource {
    fn open(&self) -> dataset_stream_parser_interface::RecordResult<Box<dyn RecordStream>> {
        self.open_input(None, 0).map_err(|error| {
            dataset_stream_parser_interface::RecordError::Io(std::io::Error::other(error.to_string()))
        })
    }

    fn open_from(
        &self,
        position: u64,
        record_index: u64,
    ) -> dataset_stream_parser_interface::RecordResult<Box<dyn RecordStream>> {
        self.open_input(Some(position), record_index).map_err(|error| {
            dataset_stream_parser_interface::RecordError::Io(std::io::Error::other(error.to_string()))
        })
    }
}



#[allow(dead_code)]
fn open_source(
    path: &str,
    record_element: &str,
) -> Result<Box<dyn RecordStream>, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let input: Box<dyn BufRead> = if path.to_ascii_lowercase().ends_with(".bz2") {
        Box::new(BufReader::new(MultiBzDecoder::new(BufReader::new(file))))
    } else {
        Box::new(BufReader::new(file))
    };

    Ok(Box::new(XmlRecordStream::new(
        input,
        XmlStreamConfig::new(record_element.to_string()),
    )?))
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
    println!("  prep [interval]       Prepare named-text previews; optionally set checkpoint spacing");
    println!("  showpreptable [index] Show prepared preview metadata or one preview");
    println!("  searchpreptable <text> [limit] Search only the prepared preview table");
    println!("  clearpreptable        Release the prepared preview table");
    println!("  find <text> [limit]   Find whole-word/phrase matches");
    println!("  get <index>           Retrieve one record by index");
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
        format!("usage: {program} <dataset.xml|dataset.xml.bz2> [record-element]")
    })?;
    let record_element = args.next().unwrap_or_else(|| "page".into());

    let mut engine = DatasetEngine::new(FileRecordSource {
        path: path.clone(),
        record_element: record_element.clone(),
    });

    println!("Dataset CLI");
    println!("  dataset: {path}");
    println!("  record:  <{record_element}>");
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
            let interval = command
                .strip_prefix("prep ")
                .and_then(|value| value.trim().parse::<u64>().ok());

            if command != "prep" && interval.is_none() {
                eprintln!("usage: prep [interval]");
                continue;
            }

            if let Some(interval) = interval {
                if let Err(error) = engine.set_checkpoint_interval(interval) {
                    eprintln!("error: {error:?}");
                    continue;
                }
                println!("Checkpoint interval: {interval} records");
            }

            println!("Preparing bounded named-text previews (target 4 KiB, stop at <title>, max 10 children)...");
            let result = engine.prep_with_progress(|count| {
                if count % 10_000 == 0 {
                    print!("\rPrepared {count} records...");
                    let _ = io::stdout().flush();
                }
            });

            match result {
                Ok(count) => {
                    print!("\rPrepared {count} records.\n");
                    println!("Search cache ready.");
                }
                Err(error) => eprintln!("\nerror: {error:?}"),
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
                            println!("#{} ({} bytes):", record.index(), record.len());
                            println!("{}", String::from_utf8_lossy(record.as_bytes()));
                        }
                    }
                }
                continue;
            }

            match value.parse::<u64>() {
                Ok(index) => match engine.preview_at(index) {
                    Some(record) => {
                        println!("#{} ({} bytes):", record.index(), record.len());
                        println!("{}", String::from_utf8_lossy(record.as_bytes()));
                    }
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

            match engine.search_previews(query, limit) {
                Ok(matches) => println!("{matches:?}"),
                Err(error) => eprintln!("error: {error:?}"),
            }
            continue;
        }

        if command == "searchpreptable" {
            eprintln!("usage: searchpreptable <text> [limit]");
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
