use bzip2::bufread::MultiBzDecoder;
use dataset_stream_parser_interface::{
    DatasetEngine, RecordStream, XmlRecordStream, XmlStreamConfig,
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

fn print_help() {
    println!("Commands:");
    println!("  prep [interval]       Prepare previews; optionally set checkpoint spacing");
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

            println!("Preparing first three child elements from each record...");
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

        if let Some(rest) = command.strip_prefix("find ") {
            let mut parts = rest.trim().splitn(2, ' ');
            let query = parts.next().unwrap_or_default().trim_matches('"');
            let limit = parts.next().and_then(|value| value.parse::<usize>().ok());

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
