//! Server-sent events, framed.
//!
//! Transport only: lines in, events out. What the data *means* is the
//! provider's business and is decoded a layer up, in
//! [`openai`](super::openai). Keeping the split lets the framer be strict
//! about the spec and the decoder be forgiving about vendors.

use std::io::{BufRead, Result};

/// One dispatched event. The chat wire format uses only the `data` field, so
/// only `data` is kept — `event`, `id`, and `retry` are read and dropped
/// rather than modelled, which is the whole of what this client needs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Event {
    pub(super) data: String,
}

/// Frames a byte stream into events, following the spec's dispatch rules:
/// `field: value` lines accumulate, a blank line dispatches what has
/// accumulated, `:`-leading lines are comments, and a field this client does
/// not care about is skipped rather than fatal — which is what keeps a
/// provider's next addition from becoming a breaking change.
#[derive(Debug)]
pub(super) struct Events<R> {
    reader: R,
    line: String,
}

impl<R: BufRead> Events<R> {
    pub(super) const fn new(reader: R) -> Self {
        Self {
            reader,
            line: String::new(),
        }
    }
}

impl<R: BufRead> Iterator for Events<R> {
    type Item = Result<Event>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut data: Option<String> = None;
        loop {
            self.line.clear();
            match self.reader.read_line(&mut self.line) {
                Err(error) => return Some(Err(error)),
                // End of stream. A half-accumulated event is discarded, as
                // the spec says: a truncated event is not an event.
                Ok(0) => return None,
                Ok(_) => {}
            }

            let line = self.line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                match data.take() {
                    Some(data) => return Some(Ok(Event { data })),
                    // A blank line with nothing pending dispatches nothing.
                    None => continue,
                }
            }
            if line.starts_with(':') {
                continue;
            }

            let (field, value) = line.split_once(':').map_or((line, ""), |(field, value)| {
                // Exactly one optional space after the colon belongs to the
                // framing, not the value.
                (field, value.strip_prefix(' ').unwrap_or(value))
            });
            if field == "data" {
                match &mut data {
                    Some(accumulated) => {
                        accumulated.push('\n');
                        accumulated.push_str(value);
                    }
                    None => data = Some(value.to_owned()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(stream: &str) -> Vec<String> {
        Events::new(stream.as_bytes())
            .map(|event| event.unwrap().data)
            .collect()
    }

    #[test]
    fn a_blank_line_dispatches_an_event() {
        assert_eq!(framed("data: one\n\ndata: two\n\n"), ["one", "two"]);
    }

    #[test]
    fn comments_and_uninteresting_fields_are_skipped() {
        let stream = ": OPENROUTER PROCESSING\n\
                      event: message\n\
                      id: 4\n\
                      data: payload\n\n";
        assert_eq!(framed(stream), ["payload"]);
    }

    #[test]
    fn multiple_data_lines_join_with_newlines() {
        assert_eq!(framed("data: first\ndata: second\n\n"), ["first\nsecond"]);
    }

    #[test]
    fn one_optional_space_after_the_colon_is_framing_not_content() {
        assert_eq!(
            framed("data:tight\n\ndata:  loose\n\n"),
            ["tight", " loose"]
        );
    }

    #[test]
    fn carriage_returns_are_line_endings_too() {
        assert_eq!(framed("data: one\r\n\r\n"), ["one"]);
    }

    #[test]
    fn a_truncated_event_is_discarded() {
        assert_eq!(framed("data: complete\n\ndata: cut off"), ["complete"]);
    }
}
