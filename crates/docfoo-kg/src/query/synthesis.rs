//! The streaming writer — ported from kg_demo `pipeline.py` `_synthesize`
//! (system prompt, evidence + KNOWN RELATIONS packing) with the answer
//! streamed over SSE instead of buffered.
//!
//! Evidence blocks are labelled `[S1] (doc:lines)`, `[S2] (doc:lines)`, … and
//! the writer cites by tag (`[S1]`). After generation the tags are expanded
//! back to the UI-native `[doc:lines]` / `[doc:line]` citation tokens, so a
//! small model never has to transform a section header into a citation.

use crate::llm::ChatClient;
use serde_json::json;
use std::sync::atomic::AtomicBool;

/// Writer token budget (pipeline.py _synthesize); sampling comes from the model.
pub const SYNTH_MAX_TOKENS: u32 = 6000;

/// Writer system prompt — kept short and example-driven so a 4B model can
/// follow the citation rule reliably without copying evidence headers.
const SYNTHESIS_SYSTEM: &str = "Answer only from the provided evidence.\n\n\
RULES\n\
- If the evidence supports part of the question, answer that part; name any part it does not support.\n\
- Write NOT COVERED only when the evidence does not address the question at all. A passing mention is not an explanation — state what the evidence does say and what it leaves out.\n\
- KNOWN RELATIONS are pointers, not explanations.\n\
- Never invent numbers, quotes, or file names.\n\
\n\
CITATIONS\n\
Each evidence block is labelled with a tag, e.g. [S1], [S2].\n\
To cite a fact from a block, end that sentence or bullet with its tag: [S1].\n\
Use [S2] for block S2, and so on. Do not write file names or line numbers.\n\
\n\
FIGURES\n\
When you use a section that has a figure, embed it inline with empty alt text,\n\
copying the image path exactly from the evidence:\n\
  ![](document_name/assets/file_name.jpg)\n\
Never put a caption, image name or description in the reply.\n\
\n\
MATH\n\
Keep LaTeX exactly as given ($...$ and $$...$$).\n\
\n\
TABLES\n\
Reproduce any markdown table's pipe rows unchanged.";

/// Pull `<doc>:<start>-<end>` out of a block header shaped
/// `[<title> — <doc> lines <start>-<end>]`.
fn extract_doc_span(header: &str) -> Option<String> {
    let h = header.trim().trim_start_matches('[').trim_end_matches(']');
    let idx = h.rfind(" lines ")?;
    let doc = h[..idx].rsplit(" — ").next().unwrap_or(&h[..idx]);
    Some(format!("{}:{}", doc.trim(), h[idx + " lines ".len()..].trim()))
}

/// Label each evidence block `[S<n>] (doc:lines)` and return the citation
/// token (`[doc:lines]`) used to expand the writer's `[Sn]` tags.
fn prepare_blocks(blocks: &[String]) -> (Vec<String>, Vec<String>) {
    let mut labeled = Vec::with_capacity(blocks.len());
    let mut tokens = Vec::with_capacity(blocks.len());
    for (i, b) in blocks.iter().enumerate() {
        let n = i + 1;
        match b.split_once('\n') {
            Some((header, body)) => {
                let span = extract_doc_span(header);
                let head = match &span {
                    Some(s) => format!("[S{n}] ({s})"),
                    None => format!("[S{n}]"),
                };
                labeled.push(format!("{head}\n{body}"));
                tokens.push(span.map(|s| format!("[{s}]")).unwrap_or_default());
            }
            None => {
                labeled.push(format!("[S{n}] {b}"));
                tokens.push(String::new());
            }
        }
    }
    (labeled, tokens)
}

/// Expand the writer's `[S1]` / `[S1, S3]` tags into UI-native citations.
fn expand_tags(answer: &str, tokens: &[String]) -> String {
    let Ok(re) = regex::Regex::new(r"\[((?:S\d+)(?:\s*,\s*S\d+)*)\]") else {
        return answer.to_string();
    };
    re.replace_all(answer, |caps: &regex::Captures| {
        let mut out: Vec<String> = vec![];
        for part in caps[1].split(',') {
            let Ok(n) = part.trim().strip_prefix('S').unwrap_or("").parse::<usize>() else {
                continue;
            };
            if let Some(tok) = tokens.get(n.saturating_sub(1)) {
                if !tok.is_empty() {
                    out.push(tok.clone());
                }
            }
        }
        if out.is_empty() { caps[0].to_string() } else { out.join(" ") }
    })
    .to_string()
}

/// Shared writer call: answer strictly from the delivered evidence (plus
/// KNOWN RELATIONS triples of the visited subgraph). Streams deltas to
/// `on_delta` as they arrive; the returned answer has `[Sn]` tags expanded
/// to `[doc:lines]`.
pub fn synthesize(
    llm: &dyn ChatClient,
    query: &str,
    blocks: &[String],
    triple_lines: &[String],
    cancel: &AtomicBool,
    on_delta: &mut dyn FnMut(&str),
) -> Result<String, crate::llm::LlmError> {
    let (labeled, tokens) = prepare_blocks(blocks);
    let mut user_msg = "EVIDENCE:\n\n".to_string() + &labeled.join("\n\n");
    if !triple_lines.is_empty() {
        user_msg += "\n\nKNOWN RELATIONS:\n";
        user_msg += &triple_lines.join("\n");
    }
    user_msg += &format!("\n\nQUESTION: {query}");
    let messages = vec![
        json!({ "role": "system", "content": SYNTHESIS_SYSTEM }),
        json!({ "role": "user", "content": user_msg }),
    ];
    let raw = llm.chat_stream(&messages, SYNTH_MAX_TOKENS, on_delta, cancel)?;
    Ok(expand_tags(&raw, &tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_labelled_and_tagged() {
        let blocks = vec![
            "[Some Title — doc_a/content.md lines 59-61]\nbody one".to_string(),
            "[Other — doc_b/x.md lines 7-7]\nbody two".to_string(),
        ];
        let (labeled, tokens) = prepare_blocks(&blocks);
        assert!(labeled[0].starts_with("[S1] (doc_a/content.md:59-61)"));
        assert!(labeled[1].starts_with("[S2] (doc_b/x.md:7-7)"));
        assert_eq!(tokens[0], "[doc_a/content.md:59-61]");
    }

    #[test]
    fn tags_expand_to_citations() {
        let tokens = vec!["[doc_a/content.md:59-61]".to_string(), "[doc_b/x.md:7-7]".to_string()];
        assert_eq!(expand_tags("fact [S1].", &tokens), "fact [doc_a/content.md:59-61].");
        assert_eq!(expand_tags("both [S1, S2]", &tokens), "both [doc_a/content.md:59-61] [doc_b/x.md:7-7]");
        assert_eq!(expand_tags("none [S9]", &tokens), "none [S9]");
    }
}
