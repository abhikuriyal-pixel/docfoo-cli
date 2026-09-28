//! Tokenization helpers — ported from kg_demo `kg.py`.
//!
//! `normalize_token` is a light stemmer tuned for technical English;
//! `tokenize` + `STOPWORDS` define the lexical contract that BM25 and the
//! noise-floor estimator both rely on. `slugify` is the single source of
//! truth for entity identity (dedup + cross-document merge).

/// Light stemming for BM25 tokens. Ported line-for-line from kg_demo so the
/// two implementations tokenize identically (`stalls`->`stall`,
/// `running`->`run`, `policies`->`policy`, ...).
pub fn normalize_token(t: &str) -> String {
    let mut t = t.to_string();
    if t.chars().count() <= 3 {
        return t;
    }
    if t.ends_with("'s") {
        t.truncate(t.len() - 2);
    }
    if t.ends_with("ies") && t.chars().count() > 4 {
        let mut s: String = t.chars().take(t.chars().count() - 3).collect();
        s.push('y');
        t = s;
    } else if t.ends_with("es")
        && t.chars().count() > 4
        && !t.ends_with("ses")
        && !t.ends_with("xes")
    {
        t.truncate(t.len() - 2);
    } else if t.ends_with('s') && !t.ends_with("ss") && !t.ends_with("us") && !t.ends_with("is") {
        t.pop();
    }

    // collapse doubled final consonants left by suffix stripping
    // (e.g. "controlled" -> "controll" -> "control")
    fn collapse(mut stem: String) -> String {
        let chars: Vec<char> = stem.chars().collect();
        if chars.len() >= 3 {
            let (a, b) = (chars[chars.len() - 1], chars[chars.len() - 2]);
            if a == b && !"aeiou".contains(a) {
                stem.pop();
            }
        }
        stem
    }

    if t.ends_with("ing") && t.chars().count() - 3 >= 4 {
        t = collapse(t[..t.len() - 3].to_string());
    } else if t.ends_with("ed") && t.chars().count() - 2 >= 4 {
        t = collapse(t[..t.len() - 2].to_string());
    }
    t
}

/// Canonical entity-identity slug: `slugify` plus a conservative plural
/// collapse, so "Vision Transformers" and "Vision Transformer" share one id.
/// The plural rules deliberately skip `ses`, `xes`, `zes`, `ches` and `shes`
/// endings (houses, processes, matches) so distinct singulars are never
/// folded together by accident. The display name is never changed.
pub fn canonical_slug(name: &str) -> String {
    let slug = slugify(name);
    if slug.chars().count() <= 4 {
        return slug;
    }
    if slug.ends_with("ies") {
        return format!("{}y", &slug[..slug.len() - 3]);
    }
    if slug.ends_with('s')
        && !slug.ends_with("ss")
        && !slug.ends_with("us")
        && !slug.ends_with("is")
        && !slug.ends_with("as")
        && !slug.ends_with("os")
        && !slug.ends_with("ses")
        && !slug.ends_with("xes")
        && !slug.ends_with("zes")
        && !slug.ends_with("ches")
        && !slug.ends_with("shes")
    {
        return slug[..slug.len() - 1].to_string();
    }
    slug
}

/// Canonical entity-identity slug: lowercase, non-alphanumerics -> `_`,
/// stripped at the edges. Single source of truth for extraction dedup and
/// cross-document merge.
pub fn slugify(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_was_sep = true; // trims leading separators
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('_');
            last_was_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

pub const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "of", "to", "in", "on", "for", "at", "by", "is", "are", "was",
    "were", "be", "been", "it", "its", "as", "with", "that", "this", "these", "those", "if",
    "from", "into", "than", "then", "so", "such", "not", "no", "can", "will", "which", "what",
    "when", "where", "who", "how", "why", "do", "does", "did", "has", "have", "had", "i",
];

/// The graph's tokenizer: `[a-z0-9]+` runs, stopword-filtered, stemmed.
pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        .filter(|t| !t.is_empty())
        .filter(|t| !STOPWORDS.contains(t))
        .map(normalize_token)
        .collect()
}

/// Tiny deterministic PRNG (SplitMix64) — replaces Python's seeded
/// `random.Random(42)` in the noise-floor estimator. Any fixed-seed,
/// platform-stable generator preserves DD-17's actual requirement:
/// byte-identical floors across runs of *this* implementation.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform integer in `0..bound` (rejection sampling).
    pub fn below(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0);
        loop {
            let r = self.next_u64();
            // Rejection-sample to avoid modulo bias.
            let limit = u64::MAX - u64::MAX % bound as u64;
            if r < limit {
                return (r % bound as u64) as usize;
            }
        }
    }

    /// Ordered sample of `n` distinct indices from `0..len`
    /// (Python's `random.sample` equivalent, ascending order).
    /// Order-statistic sampling: at index i, pick i with probability
    /// `(n - taken) / (len - i)`; the last mandatory picks fall out
    /// naturally when remaining slots equal remaining needs.
    pub fn sample_indices(&mut self, n: usize, len: usize) -> Vec<usize> {
        let mut picked: Vec<usize> = Vec::with_capacity(n);
        let mut need = n.min(len);
        for i in 0..len {
            if need == 0 {
                break;
            }
            if self.below(len - i) < need {
                picked.push(i);
                need -= 1;
            }
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_token_matches_kg_demo_pairs() {
        // the exact pairs kg_demo's `python kg.py` self-test asserts
        let pairs = [
            ("stalls", "stall"),
            ("motors", "motor"),
            ("policies", "policy"),
            ("running", "run"),
            ("controlled", "control"),
            ("class", "class"),
            ("bus", "bus"),
        ];
        for (src, want) in pairs {
            assert_eq!(normalize_token(src), want, "normalize_token({src})");
        }
    }

    #[test]
    fn slugify_is_canonical() {
        assert_eq!(slugify("Ohm's Law"), "ohm_s_law");
        assert_eq!(slugify("  De Morgan--Laws! "), "de_morgan_laws");
        assert_eq!(slugify("PID Controller"), "pid_controller");
    }

    #[test]
    fn canonical_slug_collapses_simple_plurals_only() {
        // must merge
        assert_eq!(canonical_slug("Vision Transformers"), "vision_transformer");
        assert_eq!(canonical_slug("Vision Transformer"), "vision_transformer");
        assert_eq!(canonical_slug("Atomic Tokens"), "atomic_token");
        assert_eq!(canonical_slug("Policies"), "policy");
        // must not merge (the plural rule skips these endings)
        assert_eq!(canonical_slug("Processes"), "processes");
        assert_eq!(canonical_slug("Houses"), "houses");
        assert_eq!(canonical_slug("Buses"), "buses");
        assert_eq!(canonical_slug("Classes"), "classes");
        assert_eq!(canonical_slug("Canvas"), "canvas");
        // short words pass through untouched
        assert_eq!(canonical_slug("Bus"), "bus");
        assert_eq!(canonical_slug("Gas"), "gas");
    }

    #[test]
    fn tokenize_stems_and_filters() {
        assert_eq!(
            tokenize("The motors are running in the buses"),
            vec!["motor", "run", "buse"]
        );
    }

    #[test]
    fn rng_sampling_is_deterministic_and_distinct() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let sa = a.sample_indices(4, 100);
        let sb = b.sample_indices(4, 100);
        assert_eq!(sa, sb);
        assert_eq!(sa.len(), 4);
        assert!(sa.windows(2).all(|w| w[0] < w[1]), "must be ascending");
        assert!(sa.iter().all(|&i| i < 100));
    }
}
