use dataset_stream_parser_interface::{
    manip::{getfragment, searchtogetfragment},
    DatasetEngine, LoadedRecord, RecordError, RecordSource,
};
use serde::{Deserialize, Serialize};
use url::Url;

const SEARCH_PATH: &str = "/local_data/search";
const STATUS_PATH: &str = "/local_data/status";
const RECORD_PATH: &str = "/local_data";
const FIND_EVIDENCE_PATH: &str = "/local_data/findevidence";
const GET_EVIDENCE_PATH: &str = "/local_data/getevidence";

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub corpus: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct FindEvidenceRequest {
    pub corpus: Option<String>,
    pub query: Option<String>,
    pub record_limit: Option<usize>,
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub index: u64,
    pub preview: String,
}

#[derive(Debug, Serialize)]
struct SearchResponse {
    corpus: String,
    query: String,
    results: Vec<SearchResult>,
}

#[derive(Debug, Serialize)]
struct EvidenceResult {
    index: u64,
    record: String,
}

#[derive(Debug, Serialize)]
struct EvidenceResponse {
    corpus: String,
    query: String,
    results: Vec<EvidenceResult>,
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    corpus: String,
    prepared: bool,
}

#[derive(Debug, Serialize)]
struct RecordResponse {
    corpus: String,
    index: u64,
    record: String,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    pub body: String,
}

pub struct ApiState<S> {
    corpus: String,
    engine: DatasetEngine<S>,
}

impl<S: RecordSource> ApiState<S> {
    pub fn new(corpus: impl Into<String>, engine: DatasetEngine<S>) -> Self {
        Self { corpus: corpus.into(), engine }
    }

    pub fn corpus(&self) -> &str { &self.corpus }

    pub fn is_prepared(&self) -> bool { self.engine.is_prepared() }

    pub fn handle(&self, method: &str, request_url: &str, body: &[u8]) -> ApiResponse {
        match (method, request_url.split('?').next().unwrap_or(request_url)) {
            ("GET", STATUS_PATH) => self.handle_status(request_url),
            ("POST", SEARCH_PATH) => self.handle_search(body),
            ("POST", FIND_EVIDENCE_PATH) => self.handle_find_evidence(body),
            ("GET", RECORD_PATH) => self.handle_record(request_url),
            ("GET", GET_EVIDENCE_PATH) => self.handle_get_evidence(request_url),
            _ => error_response(404, "not_found", "endpoint not found"),
        }
    }

    fn handle_status(&self, request_url: &str) -> ApiResponse {
        let Some(corpus) = query_parameter(request_url, "corpus") else {
            return error_response(400, "invalid_request", "corpus is required");
        };
        if corpus != self.corpus {
            return error_response(404, "corpus_not_found", "requested corpus is not configured");
        }
        json_response(200, &StatusResponse {
            corpus: self.corpus.clone(),
            prepared: self.engine.is_prepared(),
        })
    }

    fn handle_search(&self, body: &[u8]) -> ApiResponse {
        let request: SearchRequest = match serde_json::from_slice(body) {
            Ok(request) => request,
            Err(_) => return error_response(400, "invalid_request", "request body must be valid JSON"),
        };
        let Some(corpus) = request.corpus else {
            return error_response(400, "invalid_request", "corpus is required");
        };
        if corpus != self.corpus {
            return error_response(404, "corpus_not_found", "requested corpus is not configured");
        }
        let Some(query) = request.query else {
            return error_response(400, "invalid_request", "query is required");
        };
        if query.trim().is_empty() {
            return error_response(400, "invalid_query", "query cannot be empty");
        }

        match self.engine.search_previews(&query, request.limit) {
            Ok(records) => {
                let results = records.iter().map(preview_result).collect();
                json_response(200, &SearchResponse {
                    corpus: self.corpus.clone(),
                    query,
                    results,
                })
            }
            Err(error) => map_engine_error(error),
        }
    }

    fn handle_find_evidence(&self, body: &[u8]) -> ApiResponse {
        let request: FindEvidenceRequest = match serde_json::from_slice(body) {
            Ok(request) => request,
            Err(_) => return error_response(400, "invalid_request", "request body must be valid JSON"),
        };
        let Some(corpus) = request.corpus else {
            return error_response(400, "invalid_request", "corpus is required");
        };
        if corpus != self.corpus {
            return error_response(404, "corpus_not_found", "requested corpus is not configured");
        }
        let Some(query) = request.query else {
            return error_response(400, "invalid_request", "query is required");
        };
        if query.trim().is_empty() {
            return error_response(400, "invalid_query", "query cannot be empty");
        }

        match searchtogetfragment(
            &self.engine,
            &query,
            request.record_limit,
            request.max_bytes,
        ) {
            Ok(fragments) => {
                let results = fragments.into_iter()
                    .map(|fragment| EvidenceResult {
                        index: fragment.index,
                        record: fragment.record,
                    })
                    .collect();

                json_response(200, &EvidenceResponse {
                    corpus: self.corpus.clone(),
                    query,
                    results,
                })
            }
            Err(error) => map_engine_error(error),
        }
    }

    fn handle_record(&self, request_url: &str) -> ApiResponse {
        let Some(corpus) = query_parameter(request_url, "corpus") else {
            return error_response(400, "invalid_request", "corpus is required");
        };
        if corpus != self.corpus {
            return error_response(404, "corpus_not_found", "requested corpus is not configured");
        }
        let Some(index) = query_parameter(request_url, "i") else {
            return error_response(400, "invalid_request", "i is required");
        };
        let index = match index.parse::<u64>() {
            Ok(index) => index,
            Err(_) => return error_response(400, "invalid_request", "i must be a non-negative integer"),
        };

        match self.engine.get(index) {
            Ok(Some(record)) => json_response(200, &RecordResponse {
                corpus: self.corpus.clone(),
                index: record.index(),
                record: String::from_utf8_lossy(record.as_bytes()).into_owned(),
            }),
            Ok(None) => error_response(404, "record_not_found", "record index was not found"),
            Err(error) => map_engine_error(error),
        }
    }

    fn handle_get_evidence(&self, request_url: &str) -> ApiResponse {
        let Some(corpus) = query_parameter(request_url, "corpus") else {
            return error_response(400, "invalid_request", "corpus is required");
        };
        if corpus != self.corpus {
            return error_response(404, "corpus_not_found", "requested corpus is not configured");
        }

        let Some(index) = query_parameter(request_url, "i") else {
            return error_response(400, "invalid_request", "i is required");
        };
        let index = match index.parse::<u64>() {
            Ok(index) => index,
            Err(_) => return error_response(400, "invalid_request", "i must be a non-negative integer"),
        };

        let Some(query) = query_parameter(request_url, "query") else {
            return error_response(400, "invalid_request", "query is required");
        };
        if query.trim().is_empty() {
            return error_response(400, "invalid_query", "query cannot be empty");
        }

        let max_bytes = match query_parameter(request_url, "max_bytes") {
            Some(value) => match value.parse::<usize>() {
                Ok(value) => Some(value),
                Err(_) => return error_response(400, "invalid_request", "max_bytes must be a positive integer"),
            },
            None => None,
        };

        match self.engine.get(index) {
            Ok(Some(record)) => match getfragment(&record, &query, max_bytes.unwrap_or(512)) {
                Ok(Some(fragment)) => json_response(200, &EvidenceResponse {
                    corpus: self.corpus.clone(),
                    query,
                    results: vec![EvidenceResult {
                        index: fragment.index,
                        record: fragment.record,
                    }],
                }),
                Ok(None) => error_response(
                    404,
                    "evidence_not_found",
                    "no evidence matching query was found in the record",
                ),
                Err(error) => map_engine_error(error),
            },
            Ok(None) => error_response(404, "record_not_found", "record index was not found"),
            Err(error) => map_engine_error(error),
        }
    }
}

fn preview_result(record: &LoadedRecord) -> SearchResult {
    SearchResult { index: record.index(), preview: format_preview(record) }
}

fn format_preview(record: &LoadedRecord) -> String {
    record.elements().iter()
        .map(|element| format!("<{}>: {}", element.name, element.text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn query_parameter(request_url: &str, name: &str) -> Option<String> {
    let parsed = Url::parse(&format!("http://localhost{request_url}")).ok()?;
    parsed.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
}

fn json_response<T: Serialize>(status: u16, value: &T) -> ApiResponse {
    match serde_json::to_string(value) {
        Ok(body) => ApiResponse { status, body },
        Err(_) => error_response(500, "internal_error", "failed to serialize response"),
    }
}

fn error_response(status: u16, code: &str, message: &str) -> ApiResponse {
    let body = serde_json::to_string(&ErrorBody {
        error: ErrorDetail { code: code.into(), message: message.into() },
    }).unwrap_or_else(|_| {
        format!(r#"{{"error":{{"code":"internal_error","message":"{message}"}}}}"#)
    });
    ApiResponse { status, body }
}

fn map_engine_error(error: RecordError) -> ApiResponse {
    match error {
        RecordError::InvalidConfiguration(message)
            if message.contains("preview table is not prepared") =>
            error_response(409, "corpus_not_prepared", "corpus preview table is not prepared"),
        RecordError::InvalidConfiguration(message)
            if message.contains("query cannot be empty") =>
            error_response(400, "invalid_query", &message),
        RecordError::InvalidConfiguration(message) =>
            error_response(400, "invalid_request", &message),
        RecordError::Io(_) | RecordError::Xml(_) | RecordError::Decode(_) =>
            error_response(500, "internal_error", "dataset operation failed"),
    }
}

pub fn engine_from_source<S: RecordSource>(
    corpus: impl Into<String>,
    engine: DatasetEngine<S>,
) -> ApiState<S> {
    ApiState::new(corpus, engine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataset_stream_parser_interface::{RecordResult, RecordStream, XmlRecordStream, XmlStreamConfig};
    use std::io::Cursor;

    fn state() -> ApiState<impl RecordSource> {
        let xml = br#"<pages><article><author>Alice</author><title>First Algebraic System</title><year>2026</year></article><article><author>Bob</author><title>Second</title></article></pages>"#.to_vec();
        let mut engine = DatasetEngine::new(move || -> RecordResult<Box<dyn RecordStream>> {
            Ok(Box::new(XmlRecordStream::new(Cursor::new(xml.clone()), XmlStreamConfig::new("article"))?))
        });
        engine.prep().unwrap();
        ApiState::new("dblp", engine)
    }

    #[test]
    fn search_returns_index_and_plain_preview() {
        let response = state().handle("POST", "/local_data/search",
            br#"{"corpus":"dblp","query":"algebraic system","limit":5}"#);
        assert_eq!(response.status, 200);
        let value: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(value["results"][0]["index"], 0);
        assert_eq!(value["results"][0]["preview"], "<author>: Alice\n<title>: First Algebraic System\n<year>: 2026");
        assert!(!value["results"][0]["preview"].as_str().unwrap().contains("#0"));
    }

    #[test]
    fn find_evidence_searches_and_bounds_results() {
        let response = state().handle(
            "POST",
            "/local_data/findevidence",
            br#"{"corpus":"dblp","query":"algebraic system","record_limit":5,"max_bytes":512}"#,
        );
        assert_eq!(response.status, 200);

        let value: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(value["corpus"], "dblp");
        assert_eq!(value["query"], "algebraic system");
        assert_eq!(value["results"][0]["index"], 0);
        assert_eq!(
            value["results"][0]["record"],
            "<author>: Alice\n<title>: First Algebraic System\n<year>: 2026"
        );
    }

    #[test]
    fn find_evidence_uses_manipulation_defaults() {
        let response = state().handle(
            "POST",
            "/local_data/findevidence",
            br#"{"corpus":"dblp","query":"algebraic system"}"#,
        );
        assert_eq!(response.status, 200);

        let value: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(value["results"].as_array().unwrap().len(), 1);
        assert!(value["results"][0]["record"].as_str().unwrap().len() <= 512);
    }

    #[test]
    fn get_evidence_retrieves_and_bounds_one_record() {
        let response = state().handle(
            "GET",
            "/local_data/getevidence?corpus=dblp&i=0&query=algebraic%20system&max_bytes=512",
            &[],
        );
        assert_eq!(response.status, 200);

        let value: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(value["corpus"], "dblp");
        assert_eq!(value["query"], "algebraic system");
        assert_eq!(value["results"][0]["index"], 0);
        assert_eq!(
            value["results"][0]["record"],
            "<author>: Alice\n<title>: First Algebraic System\n<year>: 2026"
        );
    }

    #[test]
    fn get_evidence_reports_missing_match() {
        let response = state().handle(
            "GET",
            "/local_data/getevidence?corpus=dblp&i=0&query=missing",
            &[],
        );
        assert_eq!(response.status, 404);
        assert!(response.body.contains("evidence_not_found"));
    }

    #[test]
    fn get_evidence_reports_missing_record() {
        let response = state().handle(
            "GET",
            "/local_data/getevidence?corpus=dblp&i=99&query=missing",
            &[],
        );
        assert_eq!(response.status, 404);
        assert!(response.body.contains("record_not_found"));
    }

    #[test]
    fn evidence_requires_preparation() {
        let xml = br#"<pages><article><title>First</title></article></pages>"#.to_vec();
        let engine = DatasetEngine::new(move || -> RecordResult<Box<dyn RecordStream>> {
            Ok(Box::new(XmlRecordStream::new(Cursor::new(xml.clone()), XmlStreamConfig::new("article"))?))
        });
        let response = ApiState::new("dblp", engine).handle(
            "POST", "/local_data/findevidence", br#"{"corpus":"dblp","query":"First"}"#);
        assert_eq!(response.status, 409);
        assert!(response.body.contains("corpus_not_prepared"));
    }

    #[test]
    fn search_rejects_unknown_corpus() {
        let response = state().handle("POST", "/local_data/search",
            br#"{"corpus":"other","query":"algebraic systems"}"#);
        assert_eq!(response.status, 404);
        assert!(response.body.contains("corpus_not_found"));
    }

    #[test]
    fn search_requires_preparation() {
        let xml = br#"<pages><article><title>First</title></article></pages>"#.to_vec();
        let engine = DatasetEngine::new(move || -> RecordResult<Box<dyn RecordStream>> {
            Ok(Box::new(XmlRecordStream::new(Cursor::new(xml.clone()), XmlStreamConfig::new("article"))?))
        });
        let response = ApiState::new("dblp", engine).handle(
            "POST", "/local_data/search", br#"{"corpus":"dblp","query":"First"}"#);
        assert_eq!(response.status, 409);
        assert!(response.body.contains("corpus_not_prepared"));
    }
}
