use bzip2::bufread::MultiBzDecoder;
use dataset_stream_parser_api::engine_from_source;
use dataset_stream_parser_interface::{
    DatasetEngine, DirectoryRecordSource, RecordResult, RecordSource, RecordStream, XmlRecordStream,
    XmlStreamConfig,
};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek};
use tiny_http_dh::{Header, Response, Server};

struct FileRecordSource {
    path: String,
    record_element: String,
}

enum ApiRecordSource {
    File(FileRecordSource),
    Directory(DirectoryRecordSource),
}

impl RecordSource for ApiRecordSource {
    fn open(&self) -> RecordResult<Box<dyn RecordStream>> {
        match self {
            Self::File(source) => source.open(),
            Self::Directory(source) => source.open(),
        }
    }

    fn initial_checkpoints(&self) -> Vec<dataset_stream_parser_interface::Checkpoint> {
        match self {
            Self::File(source) => source.initial_checkpoints(),
            Self::Directory(source) => source.initial_checkpoints(),
        }
    }

    fn preview(
        &self,
        record: &dataset_stream_parser_interface::DatasetRecord,
        config: &dataset_stream_parser_interface::PreviewConfig,
    ) -> RecordResult<dataset_stream_parser_interface::LoadedRecord> {
        match self {
            Self::File(source) => source.preview(record, config),
            Self::Directory(source) => source.preview(record, config),
        }
    }

    fn fragment(
        &self,
        record: &dataset_stream_parser_interface::DatasetRecord,
        query: &str,
        max_bytes: usize,
    ) -> RecordResult<Option<dataset_stream_parser_interface::manip::FragmentRecord>> {
        match self {
            Self::File(source) => source.fragment(record, query, max_bytes),
            Self::Directory(source) => source.fragment(record, query, max_bytes),
        }
    }

    fn open_from(&self, position: u64, record_index: u64) -> RecordResult<Box<dyn RecordStream>> {
        match self {
            Self::File(source) => source.open_from(position, record_index),
            Self::Directory(source) => source.open_from(position, record_index),
        }
    }
}

impl FileRecordSource {
    fn open_input(
        &self,
        position: Option<u64>,
        record_index: u64,
    ) -> Result<Box<dyn RecordStream>, Box<dyn std::error::Error>> {
        let mut file = File::open(&self.path)?;
        let compressed = self.path.to_ascii_lowercase().ends_with(".bz2");
        let start_index = if compressed && position.is_some() { 0 } else { record_index };

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

        Ok(Box::new(
            XmlRecordStream::new(input, XmlStreamConfig::new(self.record_element.clone()))?
                .with_record_index(start_index),
        ))
    }
}

impl RecordSource for FileRecordSource {
    fn open(&self) -> RecordResult<Box<dyn RecordStream>> {
        self.open_input(None, 0).map_err(|error| {
            dataset_stream_parser_interface::RecordError::Io(io::Error::other(error.to_string()))
        })
    }

    fn open_from(&self, position: u64, record_index: u64) -> RecordResult<Box<dyn RecordStream>> {
        self.open_input(Some(position), record_index).map_err(|error| {
            dataset_stream_parser_interface::RecordError::Io(io::Error::other(error.to_string()))
        })
    }
}

fn respond(request: tiny_http_dh::Request, status: u16, body: String) {
    let content_type = Header::from_bytes(
        &b"Content-Type"[..],
        &b"application/json; charset=utf-8"[..],
    )
    .expect("static content-type header is valid");

    let response = Response::from_string(body)
        .with_status_code(status)
        .with_header(content_type);

    let _ = request.respond(response);
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let program = "dataset-stream-parser-api";
    let prep = args.iter().any(|arg| arg == "--prep");
    args.retain(|arg| arg != "--prep");

    let path = args.first().ok_or_else(|| {
        format!("{program} <dataset.xml|dataset.xml.bz2|folder> [corpus] [record-element] [bind] [--prep]")
    })?;
    let corpus = args.get(1).cloned().unwrap_or_else(|| "dblp".into());
    let record_element = args.get(2).cloned().unwrap_or_else(|| "page".into());
    let bind = args.get(3).cloned().unwrap_or_else(|| "127.0.0.1:60005".into());

    let input_path = std::path::Path::new(path);
    let source = if input_path.is_dir() {
        ApiRecordSource::Directory(DirectoryRecordSource::new(input_path)?)
    } else {
        ApiRecordSource::File(FileRecordSource {
            path: path.clone(),
            record_element: record_element.clone(),
        })
    };
    let mut engine = DatasetEngine::new(source);

    if prep {
        println!("Preparing bounded named-text previews...");
        let count = engine.prep_with_progress(|count| {
            if count % 10_000 == 0 {
                print!("\rPrepared {count} records...");
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
        })?;
        println!("\rPrepared {count} records.");
        println!("Search cache ready.");
    }

    let state = engine_from_source(corpus.clone(), engine);
    let server = Server::http(&bind)?;

    println!("Dataset HTTP API");
    println!("  dataset: {path}");
    println!("  corpus:  {corpus}");
    if input_path.is_dir() {
        println!("  record:  paragraph / bounded text chunk");
    } else {
        println!("  record:  <{record_element}>");
    }
    println!("  listen:  http://{bind}");
    println!("  prepared: {}", state.is_prepared());
    println!("  startup preparation: {}", if prep { "enabled" } else { "disabled" });

    for mut request in server.incoming_requests() {
        let method = request.method().as_str().to_string();
        let url = request.url().to_string();

        let mut body = Vec::new();
        if method == "POST" {
            if let Err(error) = request.as_reader().read_to_end(&mut body) {
                let body = serde_json::json!({
                    "error": {
                        "code": "invalid_request",
                        "message": format!("failed to read request body: {error}")
                    }
                }).to_string();
                respond(request, 400, body);
                continue;
            }
        }

        let response = state.handle(&method, &url, &body);
        respond(request, response.status, response.body);
    }

    Ok(())
}
