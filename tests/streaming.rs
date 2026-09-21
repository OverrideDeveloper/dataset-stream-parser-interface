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


#[test]
fn dataset_engine_gets_a_record_by_index() {
    use dataset_stream_parser_interface::{DatasetEngine, RecordResult};

    let xml = br#"<pages><page><title>First</title></page><page><title>Second</title></page></pages>"#;
    let engine = DatasetEngine::new(|| -> RecordResult<Box<dyn RecordStream>> {
        Ok(Box::new(XmlRecordStream::new(
            Cursor::new(xml),
            XmlStreamConfig::new("page"),
        )?))
    });

    let record = engine.get(1).unwrap().unwrap();
    assert_eq!(record.index(), 1);
    assert_eq!(record.decode::<Page>().unwrap().title.as_deref(), Some("Second"));
    assert!(engine.get(2).unwrap().is_none());
}

#[test]
fn dataset_engine_finds_record_indexes() {
    use dataset_stream_parser_interface::{DatasetEngine, RecordResult};

    let xml = br#"<pages><page><title>First</title></page><page><title>Second</title></page><page><title>First Again</title></page></pages>"#;
    let engine = DatasetEngine::new(|| -> RecordResult<Box<dyn RecordStream>> {
        Ok(Box::new(XmlRecordStream::new(
            Cursor::new(xml),
            XmlStreamConfig::new("page"),
        )?))
    });

    assert_eq!(engine.find("First", None).unwrap(), vec![0, 2]);
    assert_eq!(engine.find("First", Some(1)).unwrap(), vec![0]);
    assert!(engine.find("", None).is_err());
}

#[test]
fn dataset_engine_lists_a_range_or_all_records() {
    use dataset_stream_parser_interface::{DatasetEngine, RecordResult};

    let xml = br#"<pages><page><title>First</title></page><page><title>Second</title></page><page><title>Third</title></page></pages>"#;
    let engine = DatasetEngine::new(|| -> RecordResult<Box<dyn RecordStream>> {
        Ok(Box::new(XmlRecordStream::new(
            Cursor::new(xml),
            XmlStreamConfig::new("page"),
        )?))
    });

    let range = engine.list(Some(1..3)).unwrap();
    assert_eq!(range.iter().map(|record| record.index()).collect::<Vec<_>>(), vec![1, 2]);

    let all = engine.list(None).unwrap();
    assert_eq!(all.iter().map(|record| record.index()).collect::<Vec<_>>(), vec![0, 1, 2]);
}
