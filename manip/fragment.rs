use crate::{DatasetRecord, RecordError, RecordResult};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};

/// A bounded, normalized fragment of one authoritative dataset record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FragmentRecord {
    pub index: u64,
    pub record: String,
}

/// Extract one bounded evidence window around the first whole-word match.
///
/// The authoritative record remains unchanged. XML records are projected to
/// plain text before lexical matching so structural markup does not consume
/// evidence budget. The first match becomes the anchor and the window grows
/// through complete words on either side until max_bytes is reached.
pub fn getfragment(
    record: &DatasetRecord,
    query: &str,
    max_bytes: usize,
) -> RecordResult<Option<FragmentRecord>> {
    validate_fragment_request(query, max_bytes)?;

    let text = clean_xml_text(record.as_bytes())?;
    let Some((match_start, match_end)) = first_whole_word_match(&text, query) else {
        return Ok(None);
    };

    let evidence = bounded_coherent_text_window(&text, match_start, match_end, max_bytes)?;

    Ok(Some(FragmentRecord {
        index: record.index(),
        record: evidence.to_owned(),
    }))
}

/// Extract one bounded evidence window from a plain-text record around the
/// first whole-word match.
pub fn getfragment_text(
    record: &DatasetRecord,
    query: &str,
    max_bytes: usize,
) -> RecordResult<Option<FragmentRecord>> {
    validate_fragment_request(query, max_bytes)?;

    let text = String::from_utf8_lossy(record.as_bytes());
    let Some((match_start, match_end)) = first_whole_word_match(&text, query) else {
        return Ok(None);
    };

    let evidence = bounded_coherent_text_window(&text, match_start, match_end, max_bytes)?;

    Ok(Some(FragmentRecord {
        index: record.index(),
        record: evidence.to_owned(),
    }))
}

fn validate_fragment_request(query: &str, max_bytes: usize) -> RecordResult<()> {
    if query.trim().is_empty() {
        return Err(RecordError::InvalidConfiguration(
            "fragment query cannot be empty".into(),
        ));
    }
    if max_bytes == 0 {
        return Err(RecordError::InvalidConfiguration(
            "fragment max_bytes must be greater than zero".into(),
        ));
    }
    Ok(())
}

/// Project XML into text suitable for lexical evidence extraction.
///
/// Element markup, comments, declarations and processing instructions are
/// discarded while text and CDATA are retained. <ref> elements are treated as
/// source markup noise and their contents are discarded. MediaWiki-style
/// constructs such as [[...]] and '''...''' remain text; template blocks are
/// discarded as source markup noise. This engine deliberately does not attempt
/// to interpret higher-level Wiki markup.
/// Project XML into text suitable for lexical evidence extraction.
///
/// Wikipedia XML is handled as a special projection: when a text element with
/// xml:space="preserve" occurs inside a page, only that payload is projected.
/// This keeps page metadata from becoming part of the evidence search space.
/// Other XML is projected generically, and non-XML text is left to the same
/// plain-text path used by getfragment_text.
fn clean_xml_text(bytes: &[u8]) -> RecordResult<String> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);

    let mut buffer = Vec::new();
    let mut generic_text = String::new();
    let mut wiki_text = String::new();
    let mut pending_space = false;
    let mut wiki_pending_space = false;
    let mut page_depth = 0usize;
    let mut wiki_text_depth = 0usize;
    let mut ignored_ref_depth = 0usize;
    let mut saw_xml_event = false;
    let mut saw_wiki_text = false;

    loop {
        buffer.clear();

        match reader.read_event_into(&mut buffer)? {
            Event::Start(event) => {
                saw_xml_event = true;
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();

                if name.eq_ignore_ascii_case("page") {
                    page_depth += 1;
                }

                if wiki_text_depth > 0 {
                    if name.eq_ignore_ascii_case("ref") {
                        ignored_ref_depth += 1;
                    }
                    append_projected_event_space(&mut wiki_text, &mut wiki_pending_space);
                } else if page_depth > 0
                    && name.eq_ignore_ascii_case("text")
                    && has_xml_space_preserve(event.attributes())
                {
                    wiki_text_depth = 1;
                    saw_wiki_text = true;
                    wiki_pending_space = false;
                }

                append_projected_event_space(&mut generic_text, &mut pending_space);
            }
            Event::End(event) => {
                saw_xml_event = true;
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();

                if wiki_text_depth > 0 {
                    if name.eq_ignore_ascii_case("ref") && ignored_ref_depth > 0 {
                        ignored_ref_depth -= 1;
                    }

                    if name.eq_ignore_ascii_case("text") && wiki_text_depth == 1 {
                        wiki_text_depth = 0;
                    } else {
                        wiki_pending_space = true;
                    }
                }

                if name.eq_ignore_ascii_case("page") && page_depth > 0 {
                    page_depth -= 1;
                }

                pending_space = true;
            }
            Event::Empty(event) => {
                saw_xml_event = true;
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();

                if wiki_text_depth > 0 && !name.eq_ignore_ascii_case("ref") {
                    wiki_pending_space = true;
                }

                append_projected_event_space(&mut generic_text, &mut pending_space);
            }
            Event::Text(event) => {
                saw_xml_event = true;
                let value = event.as_ref();

                if wiki_text_depth > 0 && ignored_ref_depth == 0 {
                    append_clean_text(&mut wiki_text, &mut wiki_pending_space, value);
                }
                append_clean_text(&mut generic_text, &mut pending_space, value);
            }
            Event::CData(event) => {
                saw_xml_event = true;
                let value = event.as_ref();

                if wiki_text_depth > 0 && ignored_ref_depth == 0 {
                    append_clean_text(&mut wiki_text, &mut wiki_pending_space, value);
                }
                append_clean_text(&mut generic_text, &mut pending_space, value);
            }
            Event::GeneralRef(_) => {
                saw_xml_event = true;

                if wiki_text_depth > 0 && ignored_ref_depth == 0 {
                    wiki_pending_space = true;
                }
                pending_space = true;
            }
            Event::Eof => break,
            _ => saw_xml_event = true,
        }
    }

    if saw_wiki_text {
        return Ok(clean_wikipedia_text(wiki_text));
    }

    if saw_xml_event {
        return Ok(remove_template_blocks(generic_text.trim()));
    }

    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn has_xml_space_preserve<'a>(
    attributes: quick_xml::events::attributes::Attributes<'a>,
) -> bool {
    attributes.flatten().any(|attribute| {
        let key = String::from_utf8_lossy(attribute.key.as_ref());
        if !key.eq_ignore_ascii_case("xml:space") {
            return false;
        }

        attribute
            .unescape_value()
            .map(|value| value.eq_ignore_ascii_case("preserve"))
            .unwrap_or(false)
    })
}

fn append_projected_event_space(output: &mut String, pending_space: &mut bool) {
    if !output.is_empty() {
        *pending_space = true;
    }
}

fn clean_wikipedia_text(text: String) -> String {
    let text = text.replace("\\n", "");
    let text = match text.find("== References ==") {
        Some(index) => &text[..index],
        None => &text,
    };

    remove_template_blocks(text.trim())
}

fn remove_template_blocks(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;

    while index < text.len() {
        let remainder = &text[index..];

        if remainder.starts_with("{{") {
            if let Some(end) = remainder.find("}}") {
                index += end + 2;
                if output.chars().last().is_some_and(|character| !character.is_whitespace()) {
                    output.push(' ');
                }
                continue;
            }
        }

        let character = remainder.chars().next().unwrap();
        output.push(character);
        index += character.len_utf8();
    }

    output
}

fn append_clean_text(output: &mut String, pending_space: &mut bool, bytes: &[u8]) {
    let value = String::from_utf8_lossy(bytes);
    if value.is_empty() {
        return;
    }

    if *pending_space && !output.is_empty() && value.chars().next().is_some_and(|character| !character.is_whitespace()) {
        output.push(' ');
    }
    *pending_space = false;

    output.push_str(&value);
}

fn first_whole_word_match(text: &str, query: &str) -> Option<(usize, usize)> {
    whole_word_match_ranges(text, query).into_iter().next()
}

fn bounded_coherent_text_window(
    text: &str,
    match_start: usize,
    match_end: usize,
    max_bytes: usize,
) -> RecordResult<&str> {
    if match_end - match_start > max_bytes {
        return Err(RecordError::InvalidConfiguration(
            "matching fragment exceeds max_bytes".into(),
        ));
    }

    let mut start = match_start;
    let mut end = match_end;

    loop {
        let mut expanded = false;

        if start > 0 {
            let candidate = previous_text_boundary(text, start);

            if candidate < start && end - candidate <= max_bytes {
                start = candidate;
                expanded = true;
            }
        }

        if end < text.len() {
            let candidate = next_text_boundary(text, end);

            if candidate > end && candidate - start <= max_bytes {
                end = candidate;
                expanded = true;
            }
        }

        if !expanded {
            break;
        }
    }

    Ok(&text[start..end])
}

fn previous_text_boundary(text: &str, start: usize) -> usize {
    let mut cursor = start;

    while cursor > 0 {
        let Some((index, character)) = text[..cursor].char_indices().next_back() else {
            break;
        };

        if !character.is_whitespace() {
            break;
        }

        cursor = index;
    }

    text[..cursor]
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map(|(index, character)| index + character.len_utf8())
        .unwrap_or(0)
}

fn next_text_boundary(text: &str, end: usize) -> usize {
    let mut cursor = end;

    while cursor < text.len() {
        let character = text[cursor..].chars().next().unwrap();
        if !character.is_whitespace() {
            break;
        }
        cursor += character.len_utf8();
    }

    text[cursor..]
        .char_indices()
        .find(|(_, character)| character.is_whitespace())
        .map(|(index, _)| cursor + index)
        .unwrap_or(text.len())
}

fn whole_word_match_ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    let text_words = word_ranges(text);
    let query_words = word_ranges(query);

    if query_words.is_empty() || query_words.len() > text_words.len() {
        return Vec::new();
    }

    text_words
        .windows(query_words.len())
        .filter_map(|window| {
            let matches = window
                .iter()
                .zip(&query_words)
                .all(|((_, _, left), (_, _, right))| left.eq_ignore_ascii_case(right));

            matches.then_some((window[0].0, window[window.len() - 1].1))
        })
        .collect()
}

fn word_ranges<'a>(text: &'a str) -> Vec<(usize, usize, &'a str)> {
    let mut ranges = Vec::new();
    let mut start = None;

    for (index, character) in text.char_indices() {
        let is_word = character.is_alphanumeric() || character == '_';

        match (start, is_word) {
            (None, true) => start = Some(index),
            (Some(word_start), false) => {
                ranges.push((word_start, index, &text[word_start..index]));
                start = None;
            }
            _ => {}
        }
    }

    if let Some(word_start) = start {
        ranges.push((word_start, text.len(), &text[word_start..]));
    }

    ranges
}

#[cfg(test)]
mod tests {
    use super::{clean_xml_text, getfragment, getfragment_text};
    use crate::DatasetRecord;

    fn record(xml: &str) -> DatasetRecord {
        DatasetRecord::new(42, xml.as_bytes().to_vec())
    }

    #[test]
    fn text_fragments_use_first_match_as_the_anchor() {
        let record = DatasetRecord::new(
            246,
            b"before target alpha beta gamma target later".to_vec(),
        );

        let result = getfragment_text(&record, "target", 24).unwrap().unwrap();

        assert_eq!(result.index, 246);
        assert_eq!(result.record.matches("target").count(), 1);
        assert!(result.record.starts_with("before target"));
        assert!(result.record.len() <= 24);
    }

    #[test]
    fn larger_text_budget_expands_around_the_same_first_match() {
        let record = DatasetRecord::new(
            246,
            b"one two three four target five six seven eight nine ten".to_vec(),
        );

        let small = getfragment_text(&record, "target", 20).unwrap().unwrap();
        let large = getfragment_text(&record, "target", 40).unwrap().unwrap();

        assert!(small.record.contains("target"));
        assert!(large.record.contains("target"));
        assert!(large.record.contains(&small.record));
        assert!(large.record.len() >= small.record.len());
        assert_eq!(large.record.matches("target").count(), 1);
    }

    #[test]
    fn text_fragments_reject_a_match_larger_than_the_budget() {
        let record = DatasetRecord::new(1, b"supercalifragilisticexpialidocious".to_vec());

        let result = getfragment_text(&record, "supercalifragilisticexpialidocious", 8);

        assert!(result.is_err());
    }

    #[test]
    fn xml_fragment_removes_xml_markup_but_retains_text() {
        let result = getfragment(
            &record("<article><title>Analytical Engine</title><text>Charles Babbage designed the Analytical Engine.</text></article>"),
            "Analytical Engine",
            64,
        )
        .unwrap()
        .unwrap();

        assert!(!result.record.contains("<title>"));
        assert!(!result.record.contains("<text>"));
        assert!(result.record.contains("Analytical Engine"));
        assert!(result.record.contains("Charles Babbage"));
        assert!(result.record.len() <= 64);
    }

    #[test]
    fn xml_fragment_discards_ref_contents() {
        let result = getfragment(
            &record("<article><text>Analytical Engine was important.<ref>noisy reference text</ref> More context.</text></article>"),
            "Analytical Engine",
            128,
        )
        .unwrap()
        .unwrap();

        assert!(result.record.contains("Analytical Engine"));
        assert!(!result.record.contains("noisy reference text"));
    }

    #[test]
    fn xml_fragment_preserves_wiki_markup_as_text() {
        let result = getfragment(
            &record("<article><text>[[Analytical Engine|Analytical Engine]] was a '''mechanical''' computer.</text></article>"),
            "Analytical Engine",
            128,
        )
        .unwrap()
        .unwrap();

        assert!(result.record.contains("[[Analytical Engine|Analytical Engine]]"));
        assert!(result.record.contains("'''mechanical'''"));
    }

    #[test]
    fn returns_one_fragment_for_multiple_matches() {
        let result = getfragment(
            &record(
                &format!(
                    "<page><text>First Analytical Engine discussion. {} Second Analytical Engine discussion. {} Third Analytical Engine discussion.</text></page>",
                    "padding ".repeat(120),
                    "padding ".repeat(120),
                ),
            ),
            "Analytical Engine",
            1600,
        )
        .unwrap()
        .unwrap();

        assert!(result.record.starts_with("First Analytical Engine discussion."));
        assert!(result.record.len() <= 1600);
    }

    #[test]
    fn finds_first_match_through_oversized_xml_containers() {
        let result = getfragment(
            &record(
                "<page><title>Metadata</title><revision><id>1</id><text>Charles Babbage designed the Analytical Engine as a mechanical general-purpose computer.</text></revision></page>",
            ),
            "mechanical general-purpose computer",
            96,
        )
        .unwrap()
        .unwrap();

        assert!(result.record.contains("mechanical general-purpose computer"));
        assert!(!result.record.contains("<revision>"));
        assert!(result.record.len() <= 96);
    }

    #[test]
    fn wikipedia_projection_anchors_on_article_text_not_page_metadata() {
        let result = getfragment(
            &record(
                r#"<page>
                    <title>Analytical Engine</title>
                    <id>123</id>
                    <revision>
                        <id>456</id>
                        <text xml:space="preserve">The Analytical Engine was a proposed mechanical general-purpose computer. \n [[Charles Babbage]] described it in detail.</text>
                    </revision>
                </page>"#,
            ),
            "Analytical Engine",
            256,
        )
        .unwrap()
        .unwrap();

        assert!(result.record.starts_with("The Analytical Engine"));
        assert!(!result.record.contains("<page>"));
        assert!(!result.record.contains("123"));
        assert!(!result.record.contains("456"));
        assert!(!result.record.contains(r#"\n"#));
    }

    #[test]
    fn wikipedia_projection_stops_at_references_heading() {
        let result = getfragment(
            &record(
                r#"<page><title>Analytical Engine</title><revision><text xml:space="preserve">The Analytical Engine was a proposed computer. More article text.

== References ==
* The Analytical Engine reference contains the target term.</text></revision></page>"#,
            ),
            "Analytical Engine",
            512,
        )
        .unwrap()
        .unwrap();

        assert!(result.record.contains("The Analytical Engine was a proposed computer."));
        assert!(!result.record.contains("== References =="));
        assert!(!result.record.contains("reference contains"));
    }

    #[test]
    fn non_wikipedia_xml_keeps_generic_xml_projection() {
        let result = getfragment(
            &record("<catalog><title>Analytical Engine</title><description>Mechanical computing history.</description></catalog>"),
            "Analytical Engine",
            128,
        )
        .unwrap()
        .unwrap();

        assert!(result.record.contains("Analytical Engine"));
        assert!(result.record.contains("Mechanical computing history."));
    }

    #[test]
    fn wikipedia_projection_keeps_actual_newlines() {
        let result = clean_xml_text(
            br#"<page><revision><text xml:space="preserve">first line
second line\nthird line</text></revision></page>"#,
        )
        .unwrap();

        assert!(result.contains("first line\nsecond line"));
        assert!(result.contains("second linethird line"));
    }

    #[test]
    fn returns_none_when_query_is_absent() {
        assert!(getfragment(
            &record("<article><a>hello</a></article>"),
            "target",
            100
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn matches_whole_words_case_insensitively() {
        assert!(getfragment(
            &record("<article><title>Algebraic Systems</title></article>"),
            "algebraic systems",
            100,
        )
        .unwrap()
        .is_some());

        assert!(getfragment(
            &record("<article><title>Algebraic SystemsX</title></article>"),
            "algebraic systems",
            100,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn rejects_empty_query_and_zero_budget() {
        assert!(getfragment(&record("<article><a>x</a></article>"), "", 100).is_err());
        assert!(getfragment(&record("<article><a>x</a></article>"), "x", 0).is_err());
    }

    #[test]
    fn finds_matches_that_would_otherwise_cross_a_chunk_boundary() {
        let padding = "x".repeat(500);
        let xml = format!(
            "<page><text>{padding} Analytical Engine is here. {padding}</text></page>"
        );

        let result = getfragment(&record(&xml), "Analytical Engine", 512)
            .unwrap()
            .unwrap();

        assert!(result.record.contains("Analytical Engine"));
        assert!(result.record.len() <= 512);
    }

    #[test]
    fn clean_xml_text_keeps_entity_references_as_text() {
        let result = clean_xml_text(b"<text>Tom &amp; Jerry</text>").unwrap();
        assert!(result.contains("Tom"));
        assert!(!result.contains("&amp;"));
        assert!(result.contains("Jerry"));
    }
}
