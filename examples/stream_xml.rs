use bzip2::bufread::MultiBzDecoder;
use dataset_stream_parser_interface::{RecordStream, XmlRecordStream, XmlStreamConfig};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Debug, Deserialize)]
struct Page {
    title: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: stream_xml <dataset.xml|dataset.xml.bz2> [record-element]");
    let record_element = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "page".to_string());

    let file = File::open(&path)?;
    let input: Box<dyn BufRead> = if path.to_ascii_lowercase().ends_with(".bz2") {
        Box::new(BufReader::new(MultiBzDecoder::new(BufReader::new(file))))
    } else {
        Box::new(BufReader::new(file))
    };

    let mut stream = XmlRecordStream::new(
        input,
        XmlStreamConfig::new(record_element),
    )?;

    while let Some(record) = stream.next_record()? {
        let page: Page = record.decode()?;
        println!("#{}: {}", record.index(), page.title.unwrap_or_default());
    }

    Ok(())
}
