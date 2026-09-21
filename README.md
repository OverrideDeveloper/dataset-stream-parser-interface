# dataset-stream-parser-interface

A Rust dataset parser that streams meaningful records from large datasets and exposes them through a machine-facing API.

The first implementation targets XML because large public datasets such as Wikipedia are commonly distributed as enormous XML dumps. The library is intentionally not a Wikipedia parser. The XML record boundary is configurable so the same machinery can be reused for other datasets.

## Core idea

~~~text
large dataset
    |
    v
streaming parser
    |
    v
meaningful record boundary
    |
    v
bounded DatasetRecord
    |
    v
application/schema decoder
    |
    v
machine-facing API
~~~

Only the current record is materialized. Dataset size therefore does not determine the parser's working set; the bounded record does.

## MVP API

~~~rust
use dataset_stream_parser_interface::{
    RecordStream, XmlRecordStream, XmlStreamConfig,
};

let file = std::fs::File::open("dataset.xml")?;
let mut stream = XmlRecordStream::new(
    std::io::BufReader::new(file),
    XmlStreamConfig::new("page"),
)?;

while let Some(record) = stream.next_record()? {
    let value: MyRecord = record.decode()?;
}
~~~

DatasetRecord provides:

- zero-based record index
- serialized byte length
- borrowed access to serialized bytes
- schema-specific decode

The streaming layer does not know the dataset schema. That separation is deliberate.

## XML strategy

The MVP uses a hybrid strategy:

1. stream XML events;
2. recognize a meaningful record element;
3. collect only that element's subtree;
4. expose the bounded bytes as a DatasetRecord;
5. optionally deserialize that record into a normal Rust structure;
6. discard it and continue streaming.

This avoids whole-document deserialization while keeping application code ergonomic.

There is an intentional tradeoff: the bounded record is parsed once for boundary extraction and may be decoded a second time for schema access. Optimizing that double parse is a later concern; establishing the correct interface comes first.

## Memory invariant

The parser must not require the complete dataset in memory.

max_record_bytes provides an explicit upper bound for the serialized record held by the stream. Consumers should choose a bound appropriate to their dataset and expected record size.

This is a streaming interface, not a database. It does not build a complete corpus index.

## Dataset neutrality

Wikipedia is the first planned dataset, not the abstraction.

Potential future sources include:

- Wikipedia dumps
- Discogs datasets
- government datasets
- scientific corpora
- archives
- local document collections
- other XML or record-oriented datasets

Dataset-specific knowledge belongs above the stream boundary.

## Current scope

The MVP currently provides:

- Rust library crate
- streaming XML record extraction
- configurable record element
- bounded record size
- record indexing
- generic Serde decoding
- example CLI program
- unit tests for streaming and bounds

Not yet included:

- Wikipedia-specific schema
- compressed-input adapters
- random access
- indexing
- database storage
- semantic search
- network service

Those belong to later layers rather than the first grimoire.

## License

MIT. See LICENSE.

## Validation

The intended validation commands are:

~~~text
cargo test
cargo run --example stream_xml -- path/to/dataset.xml
~~~

cargo test should be run before treating the implementation as validated.
