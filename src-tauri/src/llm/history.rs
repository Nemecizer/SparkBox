//! Saved conversations, one JSON file each under the app's data folder. Each belongs to the
//! data source it was started under (Demo or Live), and the list shows the current one's only.
//! A file from before sources were kept apart is placed by its provider: a chat with Claude or
//! OpenAI is the same whatever the cluster shows, so it's Live; one with the demo model or a
//! cluster engine is Demo (Live was first used on Oct 29).

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub ttft_ms: Option<u64>,
    pub total_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Only when the stream was long enough to measure (throughput.rs).
    pub tokens_per_sec: Option<f32>,
    /// Streamed pieces of output, and the time from the first to the last.
    #[serde(default)]
    pub deltas: Option<u64>,
    #[serde(default)]
    pub decode_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// "user" or "assistant", or "system" for a system prompt changed after the conversation's
    /// first reply: it sits after the question it applies from, so the top-level one never changes.
    pub role: String,
    /// The visible text.
    pub content: String,
    /// Reasoning summary, shown collapsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Claude's content blocks exactly as returned. They're sent back unchanged on
    /// later turns: Claude's API ties its reasoning to the conversation, so
    /// history must stay append-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_blocks: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<Stats>,
    pub created_at: u64,
    /// Ids for branching: an edited question or a regenerated reply is a sibling under the same parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatParams {
    /// Claude: low | medium | high | xhigh | max.
    #[serde(default)]
    pub effort: Option<String>,
    /// Other providers only; Claude's current API rejects sampling parameters.
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub system: String,
    #[serde(default)]
    pub params: ChatParams,
    /// Every message of every branch; the shown thread runs from `leaf` up through parent ids.
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf: Option<String>,
    /// Started while the app showed Demo. The UI doesn't send it: `History::save` keeps the
    /// stored value, or stamps the current source on a new conversation. None in a file saved
    /// before sources were kept apart (`is_demo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demo: Option<bool>,
}

impl Conversation {
    /// Whether it belongs to Demo. A conversation saved before it carried a source is Live when
    /// it was with Claude or OpenAI (they answer the same with either source), else Demo.
    pub fn is_demo(&self) -> bool {
        self.demo.unwrap_or(!matches!(self.provider.as_str(), "anthropic" | "openai"))
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    pub provider: String,
    pub model: String,
    pub message_count: usize,
}

type SourceFn = Box<dyn Fn() -> bool + Send + Sync>;

pub struct History {
    dir: PathBuf,
    /// Whether the app shows Demo right now (set once it runs; Demo until then).
    source: Mutex<Option<SourceFn>>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl History {
    pub fn new(dir: PathBuf) -> History {
        History { dir, source: Mutex::new(None) }
    }

    /// Where the list learns which source to show: true while the app shows Demo.
    pub fn set_source(&self, demo_now: impl Fn() -> bool + Send + Sync + 'static) {
        *self.source.lock().unwrap() = Some(Box::new(demo_now));
    }

    fn demo_now(&self) -> bool {
        self.source.lock().unwrap().as_ref().is_none_or(|f| f())
    }

    fn read(&self, id: &str) -> Result<Conversation, String> {
        let text = std::fs::read_to_string(self.file(id)?).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    fn file(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err("bad conversation id".into());
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    /// The current source's conversations, newest first.
    pub fn list(&self) -> Vec<ConversationSummary> {
        let demo = self.demo_now();
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut out: Vec<ConversationSummary> = entries
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|s| serde_json::from_str::<Conversation>(&s).ok())
            .filter(|c| c.is_demo() == demo)
            .map(|c| ConversationSummary {
                message_count: c.messages.iter().filter(|m| m.role != "system").count(),
                id: c.id,
                title: c.title,
                updated_at: c.updated_at,
                provider: c.provider,
                model: c.model,
            })
            .collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    /// A conversation of the current source (another source's isn't shown, by id or otherwise).
    pub fn get(&self, id: &str) -> Result<Conversation, String> {
        let c = self.read(id)?;
        if c.is_demo() != self.demo_now() {
            return Err(format!("That conversation was made with {} data.", if c.is_demo() { "Demo" } else { "Live" }));
        }
        Ok(c)
    }

    /// Saves a conversation under the source it was started in (a new one: the current source).
    pub fn save(&self, c: &Conversation) -> Result<(), String> {
        let demo = self.read(&c.id).map(|old| old.is_demo()).unwrap_or_else(|_| self.demo_now());
        crate::util::save_json(&self.file(&c.id)?, &Conversation { demo: Some(demo), ..c.clone() })
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        match std::fs::remove_file(self.file(id)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversations_round_trip_and_ids_are_checked() {
        let dir = std::env::temp_dir().join(format!("sparkbox-history-{}", std::process::id()));
        let h = History::new(dir.clone());
        let c = Conversation {
            id: "abc-123".into(),
            title: "Hello".into(),
            created_at: 1,
            updated_at: 2,
            provider: "demo".into(),
            model: "echo".into(),
            system: String::new(),
            params: ChatParams::default(),
            messages: vec![],
            leaf: None,
            demo: Some(true),
        };
        h.save(&c).unwrap();
        assert_eq!(h.list()[0].title, "Hello");
        assert_eq!(h.get("abc-123").unwrap().model, "echo");
        assert!(h.get("../etc/passwd").is_err());
        h.delete("abc-123").unwrap();
        assert!(h.list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn each_source_lists_its_own_conversations() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let dir = std::env::temp_dir().join(format!("sparkbox-history-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A conversation from round 3 has no source: it's Demo.
        std::fs::write(dir.join("old-1.json"), r#"{"id":"old-1","title":"llama-3.3-70b test","createdAt":1,"updatedAt":5,"provider":"vllm-cell1","model":"llama-3.3-70b","messages":[]}"#).unwrap();
        let h = History::new(dir.clone());
        let live = Arc::new(AtomicBool::new(true));
        let l = live.clone();
        h.set_source(move || !l.load(Ordering::SeqCst));
        assert!(h.list().is_empty(), "Live shows no Demo chats");
        assert!(h.get("old-1").unwrap_err().contains("Demo data"));
        // The UI never sends the source: a new conversation takes the current one, and keeps it.
        let c: Conversation = serde_json::from_str(r#"{"id":"new-1","title":"Hi","createdAt":9,"updatedAt":9,"provider":"anthropic","model":"claude-opus-5-5","messages":[]}"#).unwrap();
        h.save(&c).unwrap();
        assert_eq!(h.list().iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["new-1"]);
        live.store(false, Ordering::SeqCst);
        h.save(&Conversation { title: "Hi again".into(), ..c.clone() }).unwrap();
        assert_eq!(h.list().iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["old-1"], "Demo shows only the Demo one");
        live.store(true, Ordering::SeqCst);
        assert_eq!(h.get("new-1").unwrap().title, "Hi again", "still Live after a save from Demo");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_earlier_chat_with_claude_or_openai_is_live_and_one_with_the_demo_model_is_demo() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let dir = std::env::temp_dir().join(format!("sparkbox-history-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // As the owner's Oct 27-28 chats were saved: no source.
        let legacy = |id: &str, provider: &str, model: &str| {
            let text = format!(
                r#"{{"id":"{id}","title":"{id}","createdAt":1,"updatedAt":2,"provider":"{provider}","model":"{model}","system":"","params":{{}},"messages":[{{"role":"user","content":"Hi","createdAt":1}}]}}"#
            );
            std::fs::write(dir.join(format!("{id}.json")), &text).unwrap();
            text
        };
        let claude = legacy("claude-oct27", "anthropic", "claude-opus-5-5");
        legacy("gpt-oct28", "openai", "gpt-5.5");
        legacy("echo-oct26", "demo", "echo");
        legacy("llama-oct26", "vllm-cell1", "llama-3.3-70b");
        let h = History::new(dir.clone());
        let live = Arc::new(AtomicBool::new(true));
        let l = live.clone();
        h.set_source(move || !l.load(Ordering::SeqCst));
        let ids = |h: &History| {
            let mut v: Vec<String> = h.list().into_iter().map(|s| s.id).collect();
            v.sort();
            v
        };
        assert_eq!(ids(&h), ["claude-oct27", "gpt-oct28"], "Live lists the cloud chats");
        assert_eq!(h.get("claude-oct27").unwrap().messages[0].content, "Hi");
        assert!(h.get("echo-oct26").unwrap_err().contains("Demo data"));
        live.store(false, Ordering::SeqCst);
        assert_eq!(ids(&h), ["echo-oct26", "llama-oct26"], "Demo keeps the demo-model and demo-cluster chats");
        assert!(h.get("claude-oct27").unwrap_err().contains("Live data"));
        // Reading never rewrites the file; a save from Live stamps it Live.
        assert_eq!(std::fs::read_to_string(dir.join("claude-oct27.json")).unwrap(), claude);
        live.store(true, Ordering::SeqCst);
        let c = h.get("claude-oct27").unwrap();
        h.save(&Conversation { demo: None, ..c }).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("claude-oct27.json")).unwrap()).unwrap();
        assert_eq!(saved["demo"], false);
        let _ = std::fs::remove_dir_all(dir);
    }
}
