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

## Dataset access engine

The crate now provides a small dataset access layer on top of a re-openable `RecordStream` source:

~~~rust
use dataset_stream_parser_interface::DatasetEngine;

// The source factory creates a fresh stream for each operation.
let engine = DatasetEngine::new(|| open_record_stream());

let record = engine.get(42)?;
let indexes = engine.find("Artificial intelligence", Some(10))?;
let records = engine.list(Some(0..10))?;
let all_records = engine.list(None)?;
~~~

The operations are deliberately mechanical:

- `get(index)` retrieves one record by zero-based index.
- `find(query, limit)` returns indexes whose serialized records contain the query as a complete word or phrase. Matching is case-insensitive; Unicode alphanumeric characters and `_` are treated as word characters.
- `list(range)` returns records in a Rust half-open range such as `0..10`; `None` means all records.
- `prep()` optionally prepares lightweight previews for repeated `find()` calls and builds retrieval checkpoints at a configurable interval (1,000 records by default).

`prep()` is an explicit preparation step rather than a requirement for dataset access. Without it, `get()`, `find()`, and `list()` continue to operate directly against the re-openable stream. When preparation is requested, only the first three XML child elements of each record are retained for the in-memory search cache.

The checkpoint spacing can be configured through `DatasetEngine::with_checkpoint_interval()` or `set_checkpoint_interval()`. The CLI also accepts an optional interval on the `prep` command, for example `prep 5000`.

The engine re-opens the source for each direct operation. The initial implementation may walk from the beginning of the dataset; checkpointed retrieval is a later optimization that can be added behind the same interface.

`find()` returns indexes rather than records so discovery and retrieval remain separate concerns: find where, then get what. When the source supports seeking, `get()` uses the nearest prepared checkpoint; non-seekable sources safely fall back to streaming from the beginning. The CLI can seek ordinary XML files; bzip2 multistream input remains sequential because its decompressed positions are not directly seekable.

## Current scope

The MVP currently provides:

- Rust library crate
- streaming XML record extraction
- configurable record element
- bounded record size
- record indexing
- generic Serde decoding
- example CLI program
- bzip2 multistream input for compressed datasets
- lightweight search preparation
- checkpointed retrieval for seekable sources
- unit tests for streaming and bounds

Not yet included:

- Wikipedia-specific schema
- compressed-input adapters beyond bzip2 multistream input
- database storage
- semantic search
- network service

Those belong to later layers rather than the first grimoire.

## License

MIT. See LICENSE.

## Validation

The example accepts an optional record-element argument (default: `page`). Files ending in `.bz2` are decoded with `MultiBzDecoder`, which supports the bzip2 multistream format used by Wikipedia dumps. The XML parser itself remains compression-agnostic.

The intended validation commands are:

~~~text
cargo test
cargo run --example stream_xml -- path/to/dataset.xml [record-element]
cargo run --example stream_xml -- path/to/dataset.xml.bz2 [record-element]
~~~

cargo test should be run before treating the implementation as validated.
