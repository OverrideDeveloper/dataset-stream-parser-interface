use dataset_stream_parser_interface::{RecordStream, XmlRecordStream, XmlStreamConfig};
use serde::Deserialize;
use std::fs::File;
use std::io::BufReader;

#[derive(Debug, Deserialize)]
struct Page {
    title: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("usage: stream_xml <dataset.xml>");
    let file = File::open(path)?;

    let mut stream = XmlRecordStream::new(
        BufReader::new(file),
        XmlStreamConfig::new("page"),
    )?;

    while let Some(record) = stream.next_record()? {
        let page: Page = record.decode()?;
        println!("#{}: {}", record.index(), page.title.unwrap_or_default());
    }

    Ok(())
}
