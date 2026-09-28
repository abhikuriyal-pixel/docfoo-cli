//! Depth heuristics — ported from kg_demo `pipeline.py` (DEEP_WORDS).

const DEEP_WORDS: [&str; 14] = [
    "why", "how", "compare", "explain", "difference", "advantage", "trade-off",
    "tradeoff", "trade off", "implications", "instead", "rather than", "reason",
    "consequence",
];

pub fn classify_depth(query: &str) -> &'static str {
    let low = query.to_lowercase();
    if DEEP_WORDS.iter().any(|w| low.contains(w)) { "deep" } else { "simple" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_words_trigger_deep_classification() {
        assert_eq!(classify_depth("How does cache work?"), "deep");
        assert_eq!(classify_depth("compare RAM and cache"), "deep");
        assert_eq!(classify_depth("cache"), "simple");
        assert_eq!(classify_depth("What is RAM?"), "simple");
    }
}
