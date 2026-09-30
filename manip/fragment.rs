use crate::{record::bounded_text_window_around_match, DatasetRecord, RecordError, RecordResult};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A bounded, normalized fragment of one authoritative dataset record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FragmentRecord {
    pub index: u64,
    pub record: String,
}

#[derive(Debug, Clone)]
struct FragmentElement {
    name: String,
    text: String,
    content: String,
    children: Vec<FragmentElement>,
}

impl FragmentElement {
    fn rendered(&self) -> String {
        format!("<{}>: {}", self.name, self.content)
    }

    fn rendered_bytes(&self) -> usize {
        self.name.len() + 4 + self.content.len()
    }
}

const MAX_FRAGMENT_DEPTH: usize = 128;
const MAX_FRAGMENT_ELEMENTS: usize = 100_000;
const TARGET_EVIDENCE_FRAGMENT_BYTES: usize = 512;
const EVIDENCE_NOISE_RATIO_LIMIT: f64 = 0.11;
const EVIDENCE_NOISE_CHARS: &[char] = &['[', ']', '{', '}', '|', '<', '>', '=', '#'];

/// Extract bounded evidence around whole-word matches.
///
/// Structural elements that fit within max_bytes remain atomic. Oversized
/// structural elements are treated as containers and searched through their
/// children. Oversized leaf text is decomposed into bounded windows around
/// every whole-word match. Multiple matching fragments are retained while
/// they fit within max_bytes; structural neighbors are then added around
/// the first matching fragment when budget remains.
///
/// No matched word or structural element is truncated. For oversized text,
/// only the surrounding context is bounded.
/// Extract bounded evidence from a plain-text record around whole-word matches.
pub fn getfragment_text(
    record: &DatasetRecord,
    query: &str,
    max_bytes: usize,
) -> RecordResult<Option<FragmentRecord>> {
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

    let text = String::from_utf8_lossy(record.as_bytes());
    let match_ranges = whole_word_match_ranges(&text, query);
    if match_ranges.is_empty() {
        return Ok(None);
    }

    let target_fragment_bytes = max_bytes.min(TARGET_EVIDENCE_FRAGMENT_BYTES);
    let mut fragments = Vec::new();
    let mut seen = HashSet::new();

    for (match_start, match_end) in match_ranges {
        if match_end - match_start > max_bytes {
            return Err(RecordError::InvalidConfiguration(
                "matching fragment exceeds max_bytes".into(),
            ));
        }

        let window = bounded_text_window_around_match(
            &text,
            match_start,
            match_end,
            target_fragment_bytes,
        );
        if seen.insert(window.to_owned()) && is_useful_evidence(window) {
            fragments.push(window);
        }
    }

    if fragments.is_empty() {
        return Ok(None);
    }

    let mut evidence = String::new();
    for fragment in fragments {
        let separator = usize::from(!evidence.is_empty());
        if evidence.len() + separator + fragment.len() > max_bytes {
            break;
        }
        if separator != 0 {
            evidence.push('\n');
        }
        evidence.push_str(fragment);
    }

    Ok(Some(FragmentRecord {
        index: record.index(),
        record: evidence,
    }))
}

pub fn getfragment(
    record: &DatasetRecord,
    query: &str,
    max_bytes: usize,
) -> RecordResult<Option<FragmentRecord>> {
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

    let elements = parse_top_level_elements(record.as_bytes())?;
    let elements = collect_evidence_elements(elements, query, max_bytes)?;

    let matching_indices = elements
        .iter()
        .enumerate()
        .filter_map(|(index, element)| {
            (contains_whole_words(&element.content, query)
                && is_useful_evidence(&element.content))
                .then_some(index)
        })
        .collect::<Vec<_>>();

    if matching_indices.is_empty() {
        return Ok(None);
    }

    let mut selected = Vec::new();
    let mut used = 0usize;

    // Matching evidence gets priority over contextual neighbors.
    for index in matching_indices.iter().copied() {
        let cost = elements[index].rendered_bytes() + usize::from(!selected.is_empty());
        if used + cost > max_bytes {
            break;
        }

        used += cost;
        selected.push(index);
    }

    let match_index = matching_indices[0];

    // Preserve the existing nearest-neighbor behavior when budget remains.
    let mut distance = 1usize;
    loop {
        let left_index = match_index.checked_sub(distance);
        let right_index = match_index
            .checked_add(distance)
            .filter(|index| *index < elements.len());

        if left_index.is_none() && right_index.is_none() {
            break;
        }

        let left_cost = left_index
            .filter(|index| !selected.contains(index))
            .map(|index| elements[index].rendered_bytes() + usize::from(!selected.is_empty()));
        let right_cost = right_index
            .filter(|index| !selected.contains(index))
            .map(|index| elements[index].rendered_bytes() + usize::from(!selected.is_empty()));

        let left_fits = left_cost.is_some_and(|cost| used + cost <= max_bytes);
        let right_fits = right_cost.is_some_and(|cost| used + cost <= max_bytes);

        match (left_index, right_index, left_fits, right_fits) {
            (Some(left), Some(right), true, true)
                if !selected.contains(&left) && !selected.contains(&right) =>
            {
                used += left_cost.unwrap();
                selected.push(left);

                let right_cost = elements[right].rendered_bytes() + 1;
                if used + right_cost <= max_bytes {
                    used += right_cost;
                    selected.push(right);
                }
            }
            (Some(left), _, true, false) if !selected.contains(&left) => {
                used += left_cost.unwrap();
                selected.push(left);
            }
            (_, Some(right), false, true) if !selected.contains(&right) => {
                used += right_cost.unwrap();
                selected.push(right);
            }
            _ => {}
        }

        distance += 1;
    }

    selected.sort_unstable();

    let text = selected
        .into_iter()
        .map(|index| elements[index].rendered())
        .collect::<Vec<_>>()
        .join("\n");

    debug_assert!(text.len() <= max_bytes);

    Ok(Some(FragmentRecord {
        index: record.index(),
        record: text,
    }))
}

fn parse_top_level_elements(bytes: &[u8]) -> RecordResult<Vec<FragmentElement>> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);

    let mut buffer = Vec::new();
    let mut stack = Vec::<FragmentElement>::new();
    let mut root = None;

    loop {
        buffer.clear();

        match reader.read_event_into(&mut buffer)? {
            Event::Start(event) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                stack.push(FragmentElement {
                    name,
                    text: String::new(),
                    content: String::new(),
                    children: Vec::new(),
                });
            }
            Event::Empty(event) => {
                let child = FragmentElement {
                    name: String::from_utf8_lossy(event.name().as_ref()).into_owned(),
                    text: String::new(),
                    content: String::new(),
                    children: Vec::new(),
                };

                if let Some(parent) = stack.last_mut() {
                    parent.children.push(child);
                } else if root.is_none() {
                    root = Some(child);
                } else {
                    return Err(RecordError::InvalidConfiguration(
                        "multiple root elements in fragment record".into(),
                    ));
                }
            }
            Event::Text(event) => {
                if let Some(current) = stack.last_mut() {
                    let text = String::from_utf8_lossy(event.as_ref());
                    current.text.push_str(&text);
                    current.content.push_str(&text);
                }
            }
            Event::CData(event) => {
                if let Some(current) = stack.last_mut() {
                    let text = String::from_utf8_lossy(event.as_ref());
                    current.text.push_str(&text);
                    current.content.push_str(&text);
                }
            }
            Event::End(_) => {
                let Some(completed) = stack.pop() else {
                    return Err(RecordError::InvalidConfiguration(
                        "unexpected closing element in fragment record".into(),
                    ));
                };

                if let Some(parent) = stack.last_mut() {
                    parent.content.push_str(&completed.content);
                    parent.children.push(completed);
                } else if root.is_none() {
                    root = Some(completed);
                } else {
                    return Err(RecordError::InvalidConfiguration(
                        "multiple root elements in fragment record".into(),
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if !stack.is_empty() {
        return Err(RecordError::InvalidConfiguration(
            "unexpected end of input while building fragment".into(),
        ));
    }

    Ok(root.map(|element| element.children).unwrap_or_default())
}

fn collect_evidence_elements(
    elements: Vec<FragmentElement>,
    query: &str,
    max_bytes: usize,
) -> RecordResult<Vec<FragmentElement>> {
    let mut pending = elements
        .into_iter()
        .rev()
        .map(|element| (element, 0usize))
        .collect::<Vec<_>>();
    let mut evidence = Vec::new();
    let mut seen = 0usize;

    while let Some((element, depth)) = pending.pop() {
        seen += 1;
        if seen > MAX_FRAGMENT_ELEMENTS {
            return Err(RecordError::InvalidConfiguration(
                "fragment traversal exceeded the element limit".into(),
            ));
        }

        if element.rendered_bytes() <= max_bytes {
            evidence.push(element);
            continue;
        }

        if depth >= MAX_FRAGMENT_DEPTH {
            return Err(RecordError::InvalidConfiguration(
                "fragment traversal exceeded the depth limit".into(),
            ));
        }

        // Mixed-content XML can contain meaningful direct text alongside
        // nested children, so inspect the direct text before descending.
        if !element.text.is_empty() {
            evidence.extend(bounded_text_fragments(
                &element.name,
                &element.text,
                query,
                max_bytes,
            )?);
        }

        for child in element.children.into_iter().rev() {
            pending.push((child, depth + 1));
        }
    }

    Ok(evidence)
}

fn bounded_text_fragments(
    name: &str,
    text: &str,
    query: &str,
    max_bytes: usize,
) -> RecordResult<Vec<FragmentElement>> {
    let prefix_bytes = name.len() + 4;
    if prefix_bytes >= max_bytes {
        return if whole_word_match_ranges(text, query).is_empty() {
            Ok(Vec::new())
        } else {
            Err(RecordError::InvalidConfiguration(
                "matching fragment element exceeds max_bytes".into(),
            ))
        };
    }

    let match_ranges = whole_word_match_ranges(text, query);
    if match_ranges.is_empty() {
        return Ok(Vec::new());
    }

    let content_budget = max_bytes - prefix_bytes;
    let target_fragment_bytes = content_budget.min(TARGET_EVIDENCE_FRAGMENT_BYTES);

    let mut fragments = Vec::new();
    let mut seen = HashSet::new();

    for (match_start, match_end) in match_ranges {
        let match_bytes = match_end - match_start;
        if match_bytes > content_budget {
            return Err(RecordError::InvalidConfiguration(
                "matching fragment exceeds max_bytes".into(),
            ));
        }

        let window = bounded_text_window_around_match(text, match_start, match_end, target_fragment_bytes);
        let key = window.to_owned();

        if seen.insert(key.clone()) && is_useful_evidence(&key) {
            fragments.push(FragmentElement {
                name: name.to_owned(),
                text: key.clone(),
                content: key,
                children: Vec::new(),
            });
        }
    }

    Ok(fragments)
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

fn contains_whole_words(text: &str, query: &str) -> bool {
    !whole_word_match_ranges(text, query).is_empty()
}

fn is_useful_evidence(text: &str) -> bool {
    let character_count = text.chars().count();

    if character_count == 0 {
        return false;
    }

    let non_alphanumeric_count = text
        .chars()
        .filter(|character| EVIDENCE_NOISE_CHARS.contains(character))
        .count();

    let noise_ratio = non_alphanumeric_count as f64 / character_count as f64;

    noise_ratio <= EVIDENCE_NOISE_RATIO_LIMIT
}

#[cfg(test)]
mod tests {
    use super::{getfragment, getfragment_text};
    use crate::DatasetRecord;

    fn record(xml: &str) -> DatasetRecord {
        DatasetRecord::new(42, xml.as_bytes().to_vec())
    }

    #[test]
    fn text_fragments_extract_bounded_evidence_around_matches() {
        let record = DatasetRecord::new(
            246,
            b"CHAP. XXIV. Shu-sun Wu-shu having spoken revilingly of Chung-ni, Tsze-kung said, 'It is of no use doing so. Chung-ni cannot be reviled. The talents and virtue of other men are hillocks and mounds which may be stepped over.".to_vec(),
        );

        let result = getfragment_text(&record, "stepped", 64).unwrap().unwrap();

        assert_eq!(result.index, 246);
        assert!(result.record.contains("stepped"));
        assert!(result.record.len() <= 64);
    }

    #[test]
    fn text_fragments_reject_a_match_larger_than_the_budget() {
        let record = DatasetRecord::new(1, b"supercalifragilisticexpialidocious".to_vec());

        let result = getfragment_text(&record, "supercalifragilisticexpialidocious", 8);

        assert!(result.is_err());
    }

    #[test]
    fn expands_by_nearest_neighbors_and_keeps_both_sides_when_they_fit() {
        let result = getfragment(
            &record(
                "<article><a>one</a><b>target</b><c>three</c><d>four</d></article>",
            ),
            "target",
            42,
        )
        .unwrap()
        .unwrap();

        assert_eq!(result.index, 42);
        assert_eq!(
            result.record,
            "<a>: one\n<b>: target\n<c>: three\n<d>: four"
        );
    }

    #[test]
    fn left_wins_when_only_one_equally_near_neighbor_fits() {
        let result = getfragment(
            &record(
                "<article><a>left</a><b>target</b><c>right-is-long</c></article>",
            ),
            "target",
            24,
        )
        .unwrap()
        .unwrap();

        assert_eq!(result.record, "<a>: left\n<b>: target");
    }

    #[test]
    fn does_not_truncate_an_atomic_element() {
        let result = getfragment(
            &record("<article><a>before</a><b>target</b><c>after</c></article>"),
            "target",
            18,
        )
        .unwrap()
        .unwrap();

        assert_eq!(result.record, "<b>: target");
    }

    #[test]
    fn continues_on_the_other_side_when_one_side_reaches_the_end() {
        let result = getfragment(
            &record("<article><a>target</a><b>two</b><c>three</c></article>"),
            "target",
            31,
        )
        .unwrap()
        .unwrap();

        assert_eq!(result.record, "<a>: target\n<b>: two\n<c>: three");
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
    fn discards_evidence_above_the_noise_ratio_threshold() {
        let noisy = format!("target{}{}", "[".repeat(11), "a".repeat(82));
        assert!(getfragment(
            &record(&format!("<article><text>{noisy}</text></article>")),
            "target",
            200,
        )
        .unwrap()
        .is_some());

        let noisy = format!("target{}{}", "[".repeat(12), "a".repeat(81));
        assert!(getfragment(
            &record(&format!("<article><text>{noisy}</text></article>")),
            "target",
            200,
        )
        .unwrap()
        .is_none());

        let plain = format!("target{}{}", "[".repeat(12), "a".repeat(81));
        let plain_record = DatasetRecord::new(43, plain.as_bytes().to_vec());
        assert!(getfragment_text(&plain_record, "target", 200)
            .unwrap()
            .is_none());
    }

    #[test]
    fn ignores_the_fragment_renderer_when_calculating_noise() {
        let result = getfragment(
            &record("<article><b>target</b></article>"),
            "target",
            100,
        )
        .unwrap()
        .unwrap();

        assert_eq!(result.record, "<b>: target");
    }

    #[test]
    fn rejects_empty_query_and_zero_budget() {
        assert!(getfragment(&record("<article><a>x</a></article>"), "", 100).is_err());
        assert!(getfragment(&record("<article><a>x</a></article>"), "x", 0).is_err());
    }

    #[test]
    fn descends_through_oversized_containers_to_find_nested_evidence() {
        let result = getfragment(
            &record(
                "<page><title>Analytical engine</title><revision><id>1</id><text>Charles Babbage designed the Analytical Engine as a mechanical general-purpose computer.</text></revision></page>",
            ),
            "mechanical general-purpose computer",
            96,
        )
        .unwrap()
        .unwrap();

        assert!(!result.record.contains("<title>: Analytical engine"));
        assert!(result.record.contains("mechanical general-purpose computer"));
        assert!(result.record.len() <= 96);
    }

    #[test]
    fn returns_multiple_bounded_fragments_for_multiple_matches() {
        let result = getfragment(
            &record(
                &format!(
                    "<page><text>First Analytical Engine discussion. {} Second Analytical Engine discussion. {} Third Analytical Engine discussion. {}</text></page>",
                    "padding ".repeat(120),
                    "padding ".repeat(120),
                    "padding ".repeat(120),
                ),
            ),
            "Analytical Engine",
            1600,
        )
        .unwrap()
        .unwrap();

        assert!(result.record.matches("Analytical Engine").count() >= 3);
        assert!(result.record.len() <= 1600);
    }

    #[test]
    fn finds_matches_that_would_otherwise_cross_a_chunk_boundary() {
        let padding = "x".repeat(500);
        let xml = format!(
            "<page><text>{padding} Analytical Engine is here. {padding}</text></page>"
        );

        let result = getfragment(
            &record(&xml),
            "Analytical Engine",
            512,
        )
        .unwrap()
        .unwrap();

        assert!(result.record.contains("Analytical Engine"));
    }
}
