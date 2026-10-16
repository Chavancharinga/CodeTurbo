//! A resposta do modelo chegando aos pedaços, no protocolo de texto: a prosa
//! vai para a conversa na hora, e cada chamada de ferramenta (`<tool_call>`
//! ou o `<invoke>` nativo do Claude) vira "escrevendo <ferramenta> <arquivo> ·
//! N caracteres", em vez de aparecer como JSON cru ou de ficar tudo calado
//! até o fim da resposta.

use crate::Delta;

const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";
const INVOKE: &str = "<invoke ";
const INVOKE_CLOSE: &str = "</invoke>";
const MARKERS: [&str; 2] = [OPEN, INVOKE];
/// Characters between two progress reports of the same call.
const REPORT_EVERY: usize = 400;

/// A string value after `key` (`"name": "write_file"`), once its closing
/// quote has arrived.
fn quoted_after(text: &str, key: &str) -> Option<String> {
    let rest = text[text.find(key)? + key.len()..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// `name="write_file"` of an `<invoke ...>` tag, once complete.
fn invoke_name(call: &str) -> Option<String> {
    let rest = &call[call.find(INVOKE)? + INVOKE.len()..];
    let rest = rest.trim_start().strip_prefix("name=\"")?;
    rest.find('"').map(|i| rest[..i].to_string())
}

/// The `path` parameter of an `<invoke>` call, once its closing tag arrived.
fn invoke_path(call: &str) -> Option<String> {
    let tag = "<parameter name=\"path\">";
    let rest = &call[call.find(tag)? + tag.len()..];
    rest.find("</parameter>").map(|i| rest[..i].trim().to_string())
}

#[derive(Debug, Default)]
pub struct ReplyStream {
    buf: String,
    /// How much of `buf` is already shown or accounted for.
    shown: usize,
    /// Where the tool call being written starts.
    call: Option<usize>,
    /// Size of the call at the last report, and whether the path went out.
    reported: usize,
    path_reported: bool,
}

impl ReplyStream {
    /// Everything received so far.
    pub fn text(&self) -> &str {
        &self.buf
    }

    /// Takes the next piece of the reply; returns what to show now.
    pub fn push(&mut self, piece: &str) -> Vec<Delta> {
        self.buf.push_str(piece);
        let mut out = Vec::new();
        loop {
            let Some(start) = self.call else {
                let rest = &self.buf[self.shown..];
                if let Some(i) = MARKERS.iter().filter_map(|m| rest.find(m)).min() {
                    if i > 0 {
                        out.push(Delta::Text(rest[..i].to_string()));
                    }
                    self.shown += i;
                    self.call = Some(self.shown);
                    self.reported = 0;
                    self.path_reported = false;
                    continue;
                }
                // Hold back a tail that may be the start of a marker.
                // Long enough for the longest marker minus its last character.
                let longest = MARKERS.iter().map(|m| m.len()).max().unwrap_or(1) - 1;
                let hold = (1..=rest.len().min(longest))
                    .rev()
                    .find(|&k| rest.is_char_boundary(rest.len() - k) && MARKERS.iter().any(|m| m.starts_with(&rest[rest.len() - k..])))
                    .unwrap_or(0);
                let end = rest.len() - hold;
                if end > 0 {
                    out.push(Delta::Text(rest[..end].to_string()));
                    self.shown += end;
                }
                break;
            };
            let call = &self.buf[start..];
            let closed = if call.starts_with(OPEN) {
                call.find(CLOSE).map(|i| i + CLOSE.len())
            } else {
                call.find(INVOKE_CLOSE).map(|i| i + INVOKE_CLOSE.len())
            };
            // A small call can arrive whole in one piece: it is still
            // reported, once, before it is closed.
            let seen = &call[..closed.unwrap_or(call.len())];
            let tool = quoted_after(seen, "\"name\"").or_else(|| invoke_name(seen));
            let path = quoted_after(seen, "\"path\"").or_else(|| invoke_path(seen));
            let chars = seen.len();
            if let Some(tool) = tool {
                let first = self.reported == 0;
                let new_path = path.is_some() && !self.path_reported;
                if first || new_path || chars >= self.reported + REPORT_EVERY {
                    self.reported = chars.max(1);
                    self.path_reported |= path.is_some();
                    out.push(Delta::Composing { tool, path, chars });
                }
            }
            if let Some(len) = closed {
                self.shown = start + len;
                self.call = None;
                continue;
            }
            break;
        }
        out
    }

    /// The reply ended: whatever was held back is shown.
    pub fn finish(&mut self) -> Vec<Delta> {
        if self.call.is_none() && self.shown < self.buf.len() {
            let rest = self.buf[self.shown..].to_string();
            self.shown = self.buf.len();
            return vec![Delta::Text(rest)];
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(pieces: &[&str]) -> (String, Vec<(String, Option<String>, usize)>) {
        let mut s = ReplyStream::default();
        let mut prose = String::new();
        let mut calls = Vec::new();
        let mut take = |ds: Vec<Delta>| {
            for d in ds {
                match d {
                    Delta::Text(t) => prose.push_str(&t),
                    Delta::Composing { tool, path, chars } => calls.push((tool, path, chars)),
                    _ => {}
                }
            }
        };
        for p in pieces {
            take(s.push(p));
        }
        take(s.finish());
        (prose, calls)
    }

    #[test]
    fn prose_streams_and_the_call_becomes_progress() {
        let (prose, calls) = run(&[
            "Vou criar o arquivo. ",
            "<tool",
            "_call>{\"name\": \"write_f",
            "ile\", \"input\": {\"path\": \"src/a.rs\", \"content\": \"fn main() {}",
            "\"}}</tool_call>",
        ]);
        assert_eq!(prose, "Vou criar o arquivo. ");
        assert!(!prose.contains("tool_call") && !prose.contains('{'));
        assert_eq!(calls.first().map(|c| c.0.as_str()), Some("write_file"));
        let last = calls.last().unwrap();
        assert_eq!((last.0.as_str(), last.1.as_deref()), ("write_file", Some("src/a.rs")));
        assert!(last.2 > 20);
    }

    #[test]
    fn plain_text_is_complete_even_with_a_stray_angle_bracket() {
        let (prose, calls) = run(&["se a ", "< b e c <", "- d"]);
        assert_eq!(prose, "se a < b e c <- d");
        assert!(calls.is_empty());
        // A held-back "<" at the very end still comes out.
        assert_eq!(run(&["x <"]).0, "x <");
        assert_eq!(run(&["x <tool"]).0, "x <tool", "an unfinished marker at the end is just text");
    }

    #[test]
    fn claudes_native_invoke_form_works_too() {
        let (prose, calls) = run(&[
            "Certo.\n<invoke name=\"edit_file\">\n<parameter name=\"pa",
            "th\">src/b.rs</parameter>\n<parameter name=\"new\">xyz</parameter>\n</invoke>",
        ]);
        assert_eq!(prose, "Certo.\n");
        let last = calls.last().unwrap();
        assert_eq!((last.0.as_str(), last.1.as_deref()), ("edit_file", Some("src/b.rs")));
    }

    #[test]
    fn invoke_inside_tool_call_and_several_calls_with_prose_between() {
        let (prose, calls) = run(&[
            "Primeiro.\n<tool_call>\n<invoke name=\"read_file\"><parameter name=\"path\">a.rs</parameter></invoke>\n</tool_call>\n",
            "Depois.\n<tool_call>{\"name\": \"grep\", \"input\": {\"pattern\": \"fn\"}}</tool_call>",
        ]);
        assert_eq!(prose, "Primeiro.\n\nDepois.\n");
        let tools: Vec<&str> = calls.iter().map(|c| c.0.as_str()).collect();
        assert!(tools.contains(&"read_file") && tools.contains(&"grep"), "{tools:?}");
    }

    #[test]
    fn a_long_call_reports_its_growth_but_not_every_character() {
        let big = "x".repeat(2000);
        let mut s = ReplyStream::default();
        let mut reports = 0;
        reports += s.push("<tool_call>{\"name\": \"write_file\", \"input\": {\"path\": \"big.txt\", \"content\": \"").len();
        for chunk in big.as_bytes().chunks(10) {
            reports += s.push(std::str::from_utf8(chunk).unwrap()).len();
        }
        assert!((3..=12).contains(&reports), "{reports} reports for 2000 characters");
    }

    #[test]
    fn the_streamed_text_matches_what_the_final_parse_keeps() {
        let reply = "Olá. <tool_call>{\"name\": \"bash\", \"input\": {\"command\": \"ls\"}}</tool_call> Pronto.";
        let (prose, _) = run(&[reply]);
        let parsed: String = crate::cli::parse_reply(reply, 0)
            .into_iter()
            .filter_map(|b| match b {
                crate::ContentBlock::Text { text } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(prose.split_whitespace().collect::<Vec<_>>(), parsed.split_whitespace().collect::<Vec<_>>());
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;

    /// Feeds `reply` in two pieces, cut at `at`; returns the prose shown.
    fn shown(reply: &str, at: usize) -> String {
        let mut s = ReplyStream::default();
        let mut prose = String::new();
        let mut take = |ds: Vec<Delta>| {
            for d in ds {
                if let Delta::Text(t) = d {
                    prose.push_str(&t);
                }
            }
        };
        take(s.push(&reply[..at]));
        take(s.push(&reply[at..]));
        take(s.finish());
        prose
    }

    #[test]
    fn no_cut_of_the_reply_leaks_a_piece_of_a_tool_call() {
        let replies = [
            "Vou abrir a pagina.\n\n<tool_call>{\"name\":\"browse\",\"input\":{\"actions\":[{\"do\":\"open\",\"url\":\"https://github.com\"}]}}</tool_call>",
            "Certo.\n<invoke name=\"edit_file\"><parameter name=\"path\">a.rs</parameter></invoke>",
            "Duas:\n<tool_call>{\"name\":\"a\",\"input\":{}}</tool_call>\n<tool_call>{\"name\":\"b\",\"input\":{}}</tool_call>",
        ];
        for reply in replies {
            for at in (0..=reply.len()).filter(|&i| reply.is_char_boundary(i)) {
                let prose = shown(reply, at);
                assert!(!prose.contains("tool_call") && !prose.contains("invoke") && !prose.contains('{'), "cut at {at}: {prose:?}");
                assert!(prose.starts_with(reply.split('<').next().unwrap().trim_end()), "cut at {at}: {prose:?}");
            }
        }
    }
}
