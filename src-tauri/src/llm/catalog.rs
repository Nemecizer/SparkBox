//! Which of a provider's models to offer, in what order, and which one is the
//! default. Chat, the Server window, the local API server and the menu bar all
//! use these rules, so they never disagree about "the OpenAI default".

use std::cmp::Ordering;

use super::{Provider, ProviderKind, DEFAULT_CLAUDE_MODEL};

/// Parts of an OpenAI model id that mean it isn't a chat model.
const OPENAI_NOT_CHAT: &[&str] = &[
    "babbage", "davinci", "sora", "tts", "whisper", "embedding", "dall-e", "moderation", "audio", "realtime", "transcribe",
    "search", "image",
];

/// Model names an OpenAI-compatible engine may list that don't chat.
const ENGINE_NOT_CHAT: &[&str] = &["embed", "rerank", "whisper", "tts", "colbert", "bge-"];

/// True when `model` takes a conversation and returns text.
pub fn is_chat_model(kind: ProviderKind, model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    match kind {
        ProviderKind::Openai => {
            let family = m.starts_with("gpt-")
                || m.starts_with("chatgpt-")
                || (m.starts_with('o') && m[1..].chars().next().is_some_and(|c| c.is_ascii_digit()));
            family && !OPENAI_NOT_CHAT.iter().any(|x| m.contains(x))
        }
        ProviderKind::Anthropic => m.starts_with("claude-"),
        // Demo cluster engines stand in for real ones, so the same rule applies.
        ProviderKind::OpenaiCompatible | ProviderKind::Demo => !ENGINE_NOT_CHAT.iter().any(|x| m.contains(x)),
    }
}

/// What tunes a model's answers in Chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tuning {
    /// A reasoning effort (Claude's adaptive thinking, OpenAI's gpt-5 and o-series). These
    /// reject a temperature.
    Effort,
    Temperature,
    /// Neither (Claude before adaptive thinking: the current API takes no sampling parameters).
    Neither,
}

/// OpenAI's reasoning models: gpt-5 and later, and the o-series. The ChatGPT snapshots
/// (gpt-5-chat-latest) don't reason.
pub fn openai_reasons(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    (m.starts_with("gpt-5") && !m.contains("-chat")) || (m.starts_with('o') && m[1..].chars().next().is_some_and(|c| c.is_ascii_digit()))
}

/// Whether `model` takes a reasoning effort or a temperature.
pub fn tuning(kind: ProviderKind, model: &str) -> Tuning {
    match kind {
        ProviderKind::Anthropic if super::chat::claude_supports_adaptive(model) => Tuning::Effort,
        ProviderKind::Anthropic => Tuning::Neither,
        ProviderKind::Openai if openai_reasons(model) => Tuning::Effort,
        _ => Tuning::Temperature,
    }
}

/// "gpt-5.5-mini" → [5, 5]; "claude-opus-5-5" → [5, 5]; "o4-mini" → [4].
fn version(model: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for part in model.split(['-', '.', ':']) {
        match part.parse::<u32>() {
            // A dated snapshot (2026, 20260801) isn't part of the version.
            Ok(n) if n < 1000 => out.push(n),
            Ok(_) => break,
            Err(_) if !out.is_empty() => break,
            Err(_) => {
                // "o4" and "gpt4o"-style ids carry the number inside a word.
                let digits: String = part.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = digits.parse::<u32>() {
                    if part.starts_with('o') || part.starts_with("gpt") {
                        out.push(n);
                    }
                }
            }
        }
    }
    out
}

/// A dated or preview build sorts after the plain name it belongs to.
fn snapshot_penalty(model: &str) -> u8 {
    let m = model.to_ascii_lowercase();
    let dated = m.split(['-', ':']).any(|p| p.len() >= 4 && p.chars().all(|c| c.is_ascii_digit()));
    u8::from(dated) + u8::from(m.contains("preview") || m.contains("latest"))
}

/// Lower is shown first.
fn variant(model: &str) -> u8 {
    let m = model.to_ascii_lowercase();
    if m.contains("-nano") {
        4
    } else if m.contains("-mini") {
        3
    } else if m.contains("-pro") {
        1
    } else if m.contains("codex") || m.contains("chat-latest") {
        2
    } else {
        0
    }
}

fn openai_family(model: &str) -> u8 {
    let m = model.to_ascii_lowercase();
    if m.starts_with("gpt-") {
        0
    } else if m.starts_with('o') {
        1
    } else {
        2 // chatgpt-…
    }
}

fn claude_tier(model: &str) -> u8 {
    ["opus", "sonnet", "haiku"].iter().position(|t| model.contains(t)).map(|i| i as u8).unwrap_or(3)
}

/// The curated order: the flagship first, then newer before older, big before small.
pub fn compare(kind: ProviderKind, a: &str, b: &str) -> Ordering {
    let newer = |a: &str, b: &str| version(b).cmp(&version(a));
    let by = match kind {
        ProviderKind::Openai => openai_family(a)
            .cmp(&openai_family(b))
            .then_with(|| newer(a, b))
            .then_with(|| snapshot_penalty(a).cmp(&snapshot_penalty(b)))
            .then_with(|| variant(a).cmp(&variant(b))),
        ProviderKind::Anthropic => (b == DEFAULT_CLAUDE_MODEL)
            .cmp(&(a == DEFAULT_CLAUDE_MODEL))
            .then_with(|| newer(a, b))
            .then_with(|| claude_tier(a).cmp(&claude_tier(b)))
            .then_with(|| snapshot_penalty(a).cmp(&snapshot_penalty(b))),
        _ => Ordering::Equal,
    };
    by.then_with(|| a.cmp(b))
}

/// The models a provider offers in Chat and the Server, in the curated order.
pub fn visible_models(p: &Provider) -> Vec<String> {
    let mut out: Vec<String> = p
        .models
        .iter()
        .filter(|m| !p.chat_only || is_chat_model(p.kind, m))
        .filter(|m| !p.hidden.contains(m))
        .cloned()
        .collect();
    out.sort_by(|a, b| compare(p.kind, a, b));
    out.dedup();
    out
}

/// The one default-model rule: the saved choice while it's still offered, else the flagship.
pub fn default_model(p: &Provider) -> Option<String> {
    let visible = visible_models(p);
    match &p.default_model {
        Some(d) if visible.contains(d) => Some(d.clone()),
        _ => visible.into_iter().next(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openai(models: &[&str]) -> Provider {
        Provider {
            id: "openai".into(),
            name: "OpenAI".into(),
            kind: ProviderKind::Openai,
            models: models.iter().map(|m| m.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn openai_lists_only_chat_models_with_the_flagship_first() {
        let p = openai(&[
            "babbage-002", "chatgpt-4o-latest", "dall-e-3", "davinci-002", "gpt-4.1", "gpt-4.1-mini", "gpt-4o-audio-preview",
            "gpt-4o-mini-tts", "gpt-5", "gpt-5-mini", "gpt-5-nano", "gpt-5.5", "gpt-5.5-2026-08-07", "gpt-5.5-mini",
            "gpt-5.5-pro", "gpt-image-1", "o3", "o4-mini", "omni-moderation-latest", "sora-2", "text-embedding-3-large",
            "tts-1", "whisper-1", "gpt-realtime", "gpt-4o-transcribe",
        ]);
        let v = visible_models(&p);
        assert_eq!(
            v,
            [
                "gpt-5.5", "gpt-5.5-pro", "gpt-5.5-mini", "gpt-5.5-2026-08-07", "gpt-5", "gpt-5-mini", "gpt-5-nano", "gpt-4.1",
                "gpt-4.1-mini", "o4-mini", "o3", "chatgpt-4o-latest"
            ]
        );
        assert_eq!(default_model(&p).as_deref(), Some("gpt-5.5"), "never babbage-002 just because it sorts first");
    }

    #[test]
    fn the_allowlist_and_a_saved_default_are_respected() {
        let mut p = openai(&["gpt-5.5", "gpt-5", "text-embedding-3-large"]);
        p.hidden = vec!["gpt-5.5".into()];
        assert_eq!(visible_models(&p), ["gpt-5"]);
        assert_eq!(default_model(&p).as_deref(), Some("gpt-5"), "a hidden model can't be the default");
        p.hidden.clear();
        p.default_model = Some("gpt-5".into());
        assert_eq!(default_model(&p).as_deref(), Some("gpt-5"));
        p.chat_only = false;
        assert!(visible_models(&p).contains(&"text-embedding-3-large".to_string()));
    }

    #[test]
    fn claude_puts_the_default_then_newer_bigger_models_first() {
        let p = Provider {
            kind: ProviderKind::Anthropic,
            models: ["claude-haiku-4-5", "claude-sonnet-5-5", "claude-opus-5", "claude-opus-5-5", "claude-sonnet-4-6"]
                .iter()
                .map(|m| m.to_string())
                .collect(),
            ..Default::default()
        };
        assert_eq!(visible_models(&p), ["claude-opus-5-5", "claude-sonnet-5-5", "claude-opus-5", "claude-sonnet-4-6", "claude-haiku-4-5"]);
        assert_eq!(default_model(&p).as_deref(), Some(DEFAULT_CLAUDE_MODEL));
    }

    #[test]
    fn engines_hide_embedding_models() {
        assert!(is_chat_model(ProviderKind::OpenaiCompatible, "meta-llama/Llama-3.3-70B-Instruct"));
        assert!(!is_chat_model(ProviderKind::OpenaiCompatible, "BAAI/bge-m3"));
        assert!(!is_chat_model(ProviderKind::OpenaiCompatible, "nomic-embed-text:latest"));
        assert!(is_chat_model(ProviderKind::Openai, "o3-pro"));
        assert!(!is_chat_model(ProviderKind::Openai, "omni-moderation-latest"));
    }

    #[test]
    fn reasoning_models_take_an_effort_and_the_rest_a_temperature() {
        for m in ["gpt-5", "gpt-5.5", "gpt-5.5-pro", "gpt-5-mini", "gpt-5-codex", "o3", "o4-mini", "o1-pro"] {
            assert_eq!(tuning(ProviderKind::Openai, m), Tuning::Effort, "{m}");
        }
        for m in ["gpt-4.1", "gpt-4o", "chatgpt-4o-latest", "gpt-5-chat-latest", "gpt-test"] {
            assert_eq!(tuning(ProviderKind::Openai, m), Tuning::Temperature, "{m}");
        }
        assert_eq!(tuning(ProviderKind::Anthropic, "claude-opus-5-5"), Tuning::Effort);
        assert_eq!(tuning(ProviderKind::Anthropic, "claude-haiku-4-5"), Tuning::Neither);
        assert_eq!(tuning(ProviderKind::OpenaiCompatible, "o3-local"), Tuning::Temperature, "a cluster engine samples");
    }
}
