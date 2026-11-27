//! Model selection rules for agent hooks.
//!
//! `hookah` hooks run for every model by default ([`ModelFilter::All`]). When the configuration
//! restricts hooks to specific models ([`ModelFilter::Only`]), a hook runs only if the harness
//! reports a model matching at least one configured pattern.
//!
//! Patterns are matched ASCII case-insensitively with `*` (any run of characters) and `?` (any
//! single character). A pattern without `/` matches either the full identifier or the bare model
//! name after the last `/` (so `claude-sonnet-*` matches both `claude-sonnet-4-6` and
//! `anthropic/claude-sonnet-4-6`). A pattern containing `/` matches the full identifier only.

/// Which AI models `hookah` hooks should run for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelFilter {
    /// Run for every model, including harnesses that do not report one.
    #[default]
    All,
    /// Run only when the reported model matches at least one pattern. An empty list matches none.
    Only(Vec<String>),
}

impl ModelFilter {
    /// Builds a filter from raw configured patterns.
    ///
    /// Leading and trailing whitespace is trimmed, empty entries are dropped, and duplicates are
    /// removed while preserving order. Any `"*"` entry means [`ModelFilter::All`].
    pub fn from_patterns(patterns: impl IntoIterator<Item = String>) -> Self {
        let mut cleaned = Vec::new();
        for raw in patterns {
            let trimmed = raw.trim();
            if trimmed == "*" {
                return Self::All;
            }
            if !trimmed.is_empty() && !cleaned.iter().any(|existing| existing == trimmed) {
                cleaned.push(trimmed.to_owned());
            }
        }
        Self::Only(cleaned)
    }

    /// True when no model restriction is in effect.
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    /// True when the filter explicitly excludes every model (`models = []`).
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Only(patterns) if patterns.is_empty())
    }

    /// The configured patterns, or `None` when every model is allowed.
    pub fn patterns(&self) -> Option<&[String]> {
        match self {
            Self::All => None,
            Self::Only(patterns) => Some(patterns),
        }
    }

    /// Whether a hook invocation with the reported `model` should run.
    pub fn allows(&self, model: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Only(patterns) => {
                let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) else {
                    return false;
                };
                patterns.iter().any(|pattern| model_matches(pattern, model))
            }
        }
    }
}

/// Whether `model` matches `pattern`.
///
/// Matching is ASCII case-insensitive. `*` matches any sequence of characters and `?` matches any
/// single character. A pattern without `/` matches either the full model string or the bare model
/// name after the last `/`; a pattern with `/` matches the full string only.
pub fn model_matches(pattern: &str, model: &str) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    let model = model.trim().to_ascii_lowercase();
    if pattern.is_empty() || model.is_empty() {
        return false;
    }
    if pattern.contains('/') {
        return wildcard_match(&pattern, &model);
    }
    wildcard_match(&pattern, &model)
        || model
            .rsplit_once('/')
            .is_some_and(|(_, bare)| !bare.is_empty() && wildcard_match(&pattern, bare))
}

/// Iterative glob matcher supporting `*` and `?` in linear/quadratic worst-case time.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut star: Option<usize> = None;
    let mut backtrack = 0;

    while t < txt.len() {
        if p < pat.len() && (pat[p] == '?' || pat[p] == txt[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == '*' {
            star = Some(p);
            p += 1;
            backtrack = t;
        } else if let Some(s) = star {
            p = s + 1;
            backtrack += 1;
            t = backtrack;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == '*' {
        p += 1;
    }
    p == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_allows_everything_including_unknown_models() {
        let filter = ModelFilter::default();
        assert!(filter.is_all());
        assert_eq!(filter.patterns(), None);
        assert!(filter.allows(None));
        assert!(filter.allows(Some("")));
        assert!(filter.allows(Some("claude-sonnet-4-6")));
    }

    #[test]
    fn wildcard_star_pattern_collapses_to_all() {
        let filter = ModelFilter::from_patterns(["gpt-5".into(), " * ".into()]);
        assert_eq!(filter, ModelFilter::All);
        assert!(filter.allows(None));
    }

    #[test]
    fn empty_only_filter_matches_nothing() {
        let filter = ModelFilter::from_patterns(Vec::<String>::new());
        assert!(!filter.is_all());
        assert_eq!(filter.patterns(), Some(&[][..]));
        assert!(!filter.allows(None));
        assert!(!filter.allows(Some("gpt-5")));
    }

    #[test]
    fn only_filter_skips_unknown_or_blank_models() {
        let filter = ModelFilter::from_patterns(["gpt-5*".into()]);
        assert!(!filter.allows(None));
        assert!(!filter.allows(Some("")));
        assert!(!filter.allows(Some("   ")));
        assert!(filter.allows(Some("gpt-5")));
    }

    #[test]
    fn trims_and_deduplicates_patterns_in_order() {
        let filter = ModelFilter::from_patterns([
            " gpt-5 ".into(),
            "".into(),
            "claude-sonnet-*".into(),
            "gpt-5".into(),
        ]);
        assert_eq!(
            filter,
            ModelFilter::Only(vec!["gpt-5".into(), "claude-sonnet-*".into()])
        );
    }

    #[test]
    fn exact_case_insensitive_and_wildcard_matching() {
        assert!(model_matches("gpt-5", "gpt-5"));
        assert!(model_matches("gpt-5", "GPT-5"));
        assert!(!model_matches("gpt-5", "gpt-5.4"));
        assert!(model_matches("gpt-5*", "gpt-5"));
        assert!(model_matches("gpt-5*", "gpt-5.3-codex"));
        assert!(model_matches("o?-mini", "o4-mini"));
        assert!(!model_matches("o?-mini", "o40-mini"));
        assert!(model_matches("*opus*", "claude-opus-4-6"));
        assert!(!model_matches("", "gpt-5"));
        assert!(!model_matches("gpt-5", ""));
    }

    #[test]
    fn bare_patterns_match_provider_prefixed_models_and_provider_patterns_stay_scoped() {
        assert!(model_matches(
            "claude-sonnet-*",
            "anthropic/claude-sonnet-4-6"
        ));
        assert!(model_matches(
            "claude-sonnet-4-6",
            "openrouter/anthropic/claude-sonnet-4-6"
        ));
        assert!(model_matches(
            "anthropic/claude-sonnet-*",
            "anthropic/claude-sonnet-4-6"
        ));
        assert!(!model_matches(
            "anthropic/claude-sonnet-*",
            "openai/claude-sonnet-4-6"
        ));
        assert!(!model_matches(
            "anthropic/claude-sonnet-*",
            "claude-sonnet-4-6"
        ));
        assert!(model_matches("anthropic/*", "anthropic/claude-opus-4-6"));
    }

    #[test]
    fn repeated_stars_do_not_backtrack_exponentially() {
        let pat = "*a*a*a*a*a*a*a*a*a*a*b";
        let text = "a".repeat(64);
        assert!(!model_matches(pat, &text));
    }
}
