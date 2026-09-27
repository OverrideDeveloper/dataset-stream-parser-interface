# dataset-stream-parser-api

A small local HTTP interface over `dataset-stream-parser-interface`.

The API deliberately stays below Alice and above the dataset engine:

    HTTP request
        |
        v
    web/api
        |
        v
    DatasetEngine
        |
        +--> search_previews()
        |
        +--> get()
        |
        v
    dataset

The HTTP layer does not perform dataset preparation. Run `prep` with the existing CLI first, then start the API against the same dataset.

## Run

From the repository root:

    cargo run --manifest-path web/api/Cargo.toml -- C:\\data\\Examples\\dblp.xml dblp article 127.0.0.1:60005

Arguments:

    <dataset.xml|dataset.xml.bz2> [corpus] [record-element] [bind]

Defaults:

- corpus: `dblp`
- record element: `page`
- bind address: `127.0.0.1:60005`

The server is intentionally local-only by default.

## Endpoints

### Check readiness

    GET /local_data/status?corpus=dblp

Response:

    {
      "corpus": "dblp",
      "prepared": true
    }

### Search prepared previews

    POST /local_data/search
    Content-Type: application/json

    {
      "corpus": "dblp",
      "query": "Algebraic Systems",
      "limit": 5
    }

Response:

    {
      "corpus": "dblp",
      "query": "Algebraic Systems",
      "results": [
        {
          "index": 24980,
          "preview": "<author>: Candan Gdc\\n<title>: On Non-Hermitian Positive (Semi)Definite Linear Algebraic Systems Arising from Dissipative Hamiltonian DAEs."
        }
      ]
    }

The preview is the bounded textual representation already produced by the engine. The CLI's `#index (bytes):` display decoration is deliberately not part of the API value.

Search returns HTTP 200 with an empty `results` array when nothing matches.

### Retrieve an authoritative record

    GET /local_data?corpus=dblp&i=24980

Response:

    {
      "corpus": "dblp",
      "index": 24980,
      "record": "<article mdate=\"2023-08-28\" key=\"journals/siamsc/GuducuLMS22\">..."
    }

The `record` value is the authoritative serialized dataset record returned by `DatasetEngine::get()`.

## Error contract

| Condition | HTTP | code |
|---|---:|---|
| malformed request | 400 | `invalid_request` |
| invalid search query | 400 | `invalid_query` |
| unknown corpus | 404 | `corpus_not_found` |
| endpoint not found | 404 | `not_found` |
| corpus not prepared | 409 | `corpus_not_prepared` |
| missing record | 404 | `record_not_found` |
| unexpected engine failure | 500 | `internal_error` |

Errors have this shape:

    {
      "error": {
        "code": "corpus_not_prepared",
        "message": "corpus preview table is not prepared"
      }
    }

## Scope

This is intentionally a local dataset service, not a general web application.

It does not:

- initiate `prep()`;
- parse dataset-specific schemas;
- perform semantic search;
- interpret records;
- expose the full preview table;
- add authentication or remote-access policy;
- turn previews into a second schema-specific data model.

Those concerns belong to higher layers.
