/// The provider a model name goes to, by the maker's prefix, for the
/// providers whose models no other provider here serves under that name.
const PREFIXES: &[(&str, &str)] = &[
    ("claude-", "claude"),
    ("gpt-", "codex"),
    ("codex-", "codex"),
    ("grok-", "grok"),
    ("kimi-", "kimi"),
    ("glm-", "glm"),
];

/// Model names `/v1/models` offers for a provider. Any other name with the
/// provider's prefix routes the same way; these are a starting point for
/// clients that want a list.
pub fn models(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &["claude-opus-4-5", "claude-sonnet-4-5", "claude-haiku-4-5"],
        "codex" => &["gpt-5-codex", "gpt-5"],
        "grok" => &["grok-code-fast-1", "grok-4"],
        "kimi" => &["kimi-for-coding"],
        "glm" => &["glm-4.6"],
        "opencode" => &["opencode/glm-4.6", "opencode/kimi-k2"],
        _ => &[],
    }
}

/// A listed model name without the provider it names.
pub fn bare(model: &str) -> &str {
    model.split_once('/').map_or(model, |(_, rest)| rest)
}

/// The cheapest model each provider answers on, for a request that only
/// needs to be counted.
pub fn cheapest(provider: &str) -> Option<&'static str> {
    match provider {
        "claude" => Some("claude-haiku-4-5"),
        "codex" => Some("gpt-5"),
        "grok" => Some("grok-code-fast-1"),
        "kimi" => Some("kimi-for-coding"),
        "glm" => Some("glm-4.6"),
        "opencode" => Some("glm-4.6"),
        _ => None,
    }
}

/// `provider/model` names its provider, which a model on several providers
/// needs. Otherwise the prefix decides. Returns the provider and the model
/// name to send it.
pub fn route<'m>(model: &'m str, providers: &[&str]) -> Option<(String, &'m str)> {
    if let Some((provider, rest)) = model.split_once('/')
        && providers.contains(&provider)
        && !rest.is_empty()
    {
        return Some((provider.to_owned(), rest));
    }
    let lower = model.to_ascii_lowercase();
    PREFIXES
        .iter()
        .find(|(prefix, _)| lower.starts_with(prefix))
        .filter(|(_, p)| providers.contains(p))
        .map(|(_, p)| ((*p).to_owned(), model))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[&str] = &[
        "claude", "codex", "cursor", "glm", "grok", "kimi", "opencode",
    ];

    #[test]
    fn the_prefix_picks_the_provider() {
        let r = |m| route(m, ALL);
        assert_eq!(
            r("claude-sonnet-4-5"),
            Some(("claude".into(), "claude-sonnet-4-5"))
        );
        assert_eq!(r("Claude-Opus"), Some(("claude".into(), "Claude-Opus")));
        assert_eq!(r("gpt-5-codex"), Some(("codex".into(), "gpt-5-codex")));
        assert_eq!(r("codex-mini"), Some(("codex".into(), "codex-mini")));
        assert_eq!(r("grok-4"), Some(("grok".into(), "grok-4")));
        assert_eq!(
            r("kimi-for-coding"),
            Some(("kimi".into(), "kimi-for-coding"))
        );
        assert_eq!(r("glm-4.6"), Some(("glm".into(), "glm-4.6")));
        assert_eq!(r("llama-3"), None);
        assert_eq!(r(""), None);
    }

    #[test]
    fn a_provider_prefix_wins_and_is_taken_off() {
        let r = |m| route(m, ALL);
        assert_eq!(r("opencode/glm-4.6"), Some(("opencode".into(), "glm-4.6")));
        assert_eq!(r("kimi/kimi-k2"), Some(("kimi".into(), "kimi-k2")));
        assert_eq!(r("opencode/"), None);
        assert_eq!(r("meta/llama"), None);
    }

    #[test]
    fn only_the_providers_given_are_routed_to() {
        assert_eq!(route("claude-x", &["codex"]), None);
        assert_eq!(route("codex/gpt-5", &["claude"]), None);
    }

    #[test]
    fn every_listed_model_routes_to_its_provider() {
        for p in ALL {
            for m in models(p) {
                assert_eq!(route(m, ALL).map(|(p, _)| p).as_deref(), Some(*p), "{m}");
            }
        }
    }
}
