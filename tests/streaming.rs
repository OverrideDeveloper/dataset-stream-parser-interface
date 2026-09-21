use dataset_stream_parser_interface::{RecordStream, XmlRecordStream, XmlStreamConfig};
use serde::Deserialize;
use std::io::Cursor;

#[derive(Debug, Deserialize, PartialEq)]
struct Page {
    title: Option<String>,
}

#[test]
fn streams_one_record_at_a_time() {
    let xml = br#"<pages><page><title>First</title></page><page><title>Second</title></page></pages>"#;
    let mut stream = XmlRecordStream::new(Cursor::new(xml), XmlStreamConfig::new("page")).unwrap();

    let first = stream.next_record().unwrap().unwrap();
    assert_eq!(first.index(), 0);
    assert_eq!(first.decode::<Page>().unwrap().title.as_deref(), Some("First"));

    let second = stream.next_record().unwrap().unwrap();
    assert_eq!(second.index(), 1);
    assert_eq!(second.decode::<Page>().unwrap().title.as_deref(), Some("Second"));

    assert!(stream.next_record().unwrap().is_none());
}

#[test]
fn rejects_empty_record_name() {
    let result = XmlRecordStream::new(Cursor::new(b"<root/>"), XmlStreamConfig::new(""));
    assert!(result.is_err());
}

#[test]
fn enforces_record_size_limit() {
    let xml = br#"<root><page><title>oversized</title></page></root>"#;
    let mut stream = XmlRecordStream::new(
        Cursor::new(xml),
        XmlStreamConfig::new("page").with_max_record_bytes(4),
    ).unwrap();

    assert!(stream.next_record().is_err());
}
