mod fragment;

pub use fragment::{getfragment, getfragment_text, FragmentRecord};

use crate::{DatasetEngine, RecordError, RecordResult, RecordSource};

/// Search the prepared preview table, retrieve each matching authoritative record,
/// and extract bounded evidence from it.
///
/// The search limit defaults to five records and the evidence budget defaults to
/// 512 bytes. Each matching record is retrieved at most once.
pub fn searchtogetfragment<S: RecordSource>(
    engine: &DatasetEngine<S>,
    query: &str,
    record_limit: Option<usize>,
    max_bytes: Option<usize>,
) -> RecordResult<Vec<FragmentRecord>> {
    let record_limit = record_limit.unwrap_or(5);
    let max_bytes = max_bytes.unwrap_or(512);

    if record_limit == 0 {
        return Err(RecordError::InvalidConfiguration(
            "searchtogetfragment record_limit must be greater than zero".into(),
        ));
    }

    if max_bytes == 0 {
        return Err(RecordError::InvalidConfiguration(
            "searchtogetfragment max_bytes must be greater than zero".into(),
        ));
    }

    let indexes = engine.search_preview_indexes(query, Some(record_limit))?;
    let mut fragments = Vec::new();

    for index in indexes {
        if let Some(record) = engine.get(index)? {
            if let Some(fragment) = engine.source_fragment(index, query, max_bytes)? {
                fragments.push(fragment);
            }
        }
    }

    Ok(fragments)
}

#[cfg(test)]
mod tests {
    use super::searchtogetfragment;
    use crate::{RecordResult, RecordStream, XmlRecordStream, XmlStreamConfig};

    #[test]
    fn searches_previews_then_retrieves_bounded_fragments() {
        use std::io::Cursor;

        let xml = br#"<pages>
            <page><author>E. F. Codd</author><title>Relational Algebra</title><year>1970</year></page>
            <page><author>Grace Hopper</author><title>Compilers</title><year>1952</year></page>
            <page><author>E. F. Codd</author><title>Further Normalization</title><year>1971</year></page>
        </pages>"#.to_vec();

        let mut engine = crate::DatasetEngine::new(move || -> RecordResult<Box<dyn RecordStream>> {
            Ok(Box::new(XmlRecordStream::new(
                Cursor::new(xml.clone()),
                XmlStreamConfig::new("page"),
            )?))
        });

        engine.prep().unwrap();

        let results = searchtogetfragment(&engine, "Codd", None, None).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].index, 0);
        assert_eq!(
            results[0].record,
            "<author>: E. F. Codd\n<title>: Relational Algebra\n<year>: 1970"
        );
        assert_eq!(results[1].index, 2);
    }

    #[test]
    fn uses_record_limit_and_byte_defaults() {
        use std::io::Cursor;

        let xml = br#"<pages>
            <page><title>target</title><x>one</x></page>
            <page><title>target</title><x>two</x></page>
            <page><title>target</title><x>three</x></page>
            <page><title>target</title><x>four</x></page>
            <page><title>target</title><x>five</x></page>
            <page><title>target</title><x>six</x></page>
        </pages>"#.to_vec();

        let mut engine = crate::DatasetEngine::new(move || -> RecordResult<Box<dyn RecordStream>> {
            Ok(Box::new(XmlRecordStream::new(
                Cursor::new(xml.clone()),
                XmlStreamConfig::new("page"),
            )?))
        });

        engine.prep().unwrap();

        let results = searchtogetfragment(&engine, "target", None, None).unwrap();

        assert_eq!(results.len(), 5);
        assert!(results.iter().all(|fragment| fragment.record.len() <= 512));
    }
}
