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

#[derive(Debug, Clone)]
struct FragmentElement {
    name: String,
    text: String,
}

impl FragmentElement {
    fn rendered(&self) -> String {
        format!("<{}>: {}", self.name, self.text)
    }

    fn rendered_bytes(&self) -> usize {
        self.rendered().len()
    }
}

/// Extract a bounded fragment around the first whole-word match.
///
/// The matching structural child is retained first. Remaining budget is
/// expanded by structural-neighbor distance. At each distance, the left
/// neighbor is considered first, then the right neighbor. If both fit, both
/// are retained. If only one fits, the fitting side is retained; when the
/// sides are otherwise equally eligible, the left side wins.
///
/// Structural elements are atomic: no element or word is truncated.
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
    let Some(match_index) = elements
        .iter()
        .position(|element| contains_whole_words(&element.text, query))
    else {
        return Ok(None);
    };

    let match_element = &elements[match_index];
    let match_cost = match_element.rendered_bytes();
    if match_cost > max_bytes {
        return Err(RecordError::InvalidConfiguration(format!(
            "matching fragment element exceeds max_bytes ({max_bytes})"
        )));
    }

    let mut selected = vec![match_index];
    let mut used = match_cost;

    let mut distance = 1usize;
    loop {
        let left_index = match_index.checked_sub(distance);
        let right_index = match_index
            .checked_add(distance)
            .filter(|index| *index < elements.len());

        if left_index.is_none() && right_index.is_none() {
            break;
        }

        let left_cost = left_index.map(|index| {
            elements[index].rendered_bytes() + usize::from(!selected.is_empty())
        });
        let right_cost = right_index.map(|index| {
            elements[index].rendered_bytes() + usize::from(!selected.is_empty())
        });

        let left_fits = left_cost.is_some_and(|cost| used + cost <= max_bytes);
        let right_fits = right_cost.is_some_and(|cost| used + cost <= max_bytes);

        match (left_index, right_index, left_fits, right_fits) {
            (Some(left), Some(right), true, true) => {
                used += left_cost.unwrap();
                selected.push(left);

                let right_cost = elements[right].rendered_bytes() + 1;
                if used + right_cost <= max_bytes {
                    used += right_cost;
                    selected.push(right);
                }
            }
            (Some(left), _, true, false) => {
                used += left_cost.unwrap();
                selected.push(left);
            }
            (_, Some(right), false, true) => {
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
    let mut root_seen = false;
    let mut elements = Vec::new();

    loop {
        buffer.clear();

        match reader.read_event_into(&mut buffer)? {
            Event::Start(_) if !root_seen => {
                root_seen = true;
            }
            Event::Start(event) if root_seen => {
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                let text = collect_element_text(&mut reader, &mut buffer)?;
                elements.push(FragmentElement { name, text });
            }
            Event::Empty(event) if root_seen => {
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                elements.push(FragmentElement {
                    name,
                    text: String::new(),
                });
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(elements)
}

fn collect_element_text(
    reader: &mut Reader<&[u8]>,
    buffer: &mut Vec<u8>,
) -> RecordResult<String> {
    let mut depth = 1usize;
    let mut text = String::new();

    loop {
        buffer.clear();

        match reader.read_event_into(buffer)? {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Text(event) => {
                text.push_str(&String::from_utf8_lossy(event.as_ref()));
            }
            Event::CData(event) => {
                text.push_str(&String::from_utf8_lossy(event.as_ref()));
            }
            Event::Empty(_) => {}
            Event::Eof => {
                return Err(RecordError::InvalidConfiguration(
                    "unexpected end of input while building fragment".into(),
                ));
            }
            _ => {}
        }
    }

    Ok(text)
}

fn contains_whole_words(text: &str, query: &str) -> bool {
    let text_words = text
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let query_words = query
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();

    if query_words.is_empty() || query_words.len() > text_words.len() {
        return false;
    }

    text_words
        .windows(query_words.len())
        .any(|window| window.iter().zip(&query_words).all(|(left, right)| {
            left.eq_ignore_ascii_case(right)
        }))
}

#[cfg(test)]
mod tests {
    use super::getfragment;
    use crate::DatasetRecord;

    fn record(xml: &str) -> DatasetRecord {
        DatasetRecord::new(42, xml.as_bytes().to_vec())
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
    fn rejects_empty_query_and_zero_budget() {
        assert!(getfragment(&record("<article><a>x</a></article>"), "", 100).is_err());
        assert!(getfragment(&record("<article><a>x</a></article>"), "x", 0).is_err());
    }
}
