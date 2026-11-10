import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ArrowUp, Brain, ChevronLeft, ChevronRight, Copy, KeyRound, MessageSquare, Pencil, RotateCcw, ScrollText, SquarePen, Trash2 } from "lucide-react";
import { Icon } from "../components/Icon";
import { EmptyState, HelpTip, Notice, Spacer, StopIcon, Toolbar, ToolButton } from "../components/ui";
import { LOOK_ONLY_TIP, useAdminState } from "../lib/admin";
import { api } from "../lib/api";
import { takePendingChat, usePendingChat } from "../lib/endpoints";
import { copyText, onMarkdownClick, renderMarkdown } from "../lib/markdown";
import { chatSend, modelsApi, type ChatMessage, type Conversation, type ProviderView } from "../lib/modelsApi";
import { confirmAction } from "../lib/session";
import { toast } from "../lib/toasts";
import type { ChatParams, ConversationSummary } from "../lib/types";
import { useCloseGuard, useWM, useWindowTitle, type Params } from "../wm/WindowManager";
import { ModelPicker, rememberChoice, recentChoices, type ModelChoice } from "./chat/ModelPicker";
import { currentSource, useDataSource } from "../lib/dataSource";
import { statsLine } from "./chat/stats";
import { append, ensureTree, latestLeaf, newId, thread } from "./chat/thread";
import { Ago, errText } from "./models/shared";
import "./ai.css";

const CLAUDE_EFFORTS = ["low", "medium", "high", "xhigh", "max"];
/** What OpenAI's reasoning models (gpt-5, the o-series) all take. */
const OPENAI_EFFORTS = ["low", "medium", "high"];
/** A system prompt changed after the conversation's first reply is kept in it where it applies
 * from (llm/history.rs takes this role). */
const SYSTEM = "system" as string;
const isSystem = (m: ChatMessage) => (m.role as string) === SYSTEM;
/** The composer grows with what's typed, up to this many lines, then scrolls. */
const COMPOSER_LINES = 8;
/** Claude's notice that it left out a reply's reasoning because another model wrote it (llm/chat.rs). */
const OTHER_MODEL = /was left out: it came from another model\.$/;

interface Live {
  requestId: string;
  text: string;
  thinking: string;
  notices: string[];
  /** The message being answered (the reply's parent). */
  parentId: string;
  started: number;
}

const nowS = () => Math.floor(Date.now() / 1000);

/** Once a conversation has a reply its top-level system prompt stays as it was: Claude ties its
 * reasoning to everything before it, so a later change is added where it's made instead. */
const answered = (c: Conversation) => c.messages.some((m) => m.role === "assistant");

/** The system prompt in effect at `upTo` on its branch: the conversation's own, or the latest change before it. */
function systemAt(c: Conversation, upTo: string | null | undefined): string {
  let s = c.system;
  for (const m of upTo ? thread(c, upTo) : []) if (isSystem(m)) s = m.content;
  return s;
}

/** Other versions of `m`: questions with the same parent, or the answers to one question (its
 * replies, and replies after a system prompt change). */
function versions(c: Conversation, m: ChatMessage): ChatMessage[] {
  const same = m.role === "user" ? (x: ChatMessage) => x.role === "user" : (x: ChatMessage) => x.role === "assistant" || isSystem(x);
  return c.messages.filter((x) => (x.parentId ?? null) === (m.parentId ?? null) && same(x)).sort((a, b) => a.createdAt - b.createdAt);
}

function Markdown({ text }: { text: string }) {
  const html = useMemo(() => renderMarkdown(text), [text]);
  return <div className="md" onClick={onMarkdownClick} dangerouslySetInnerHTML={{ __html: html }} />;
}

/** "‹ 2 / 3 ›" for a message with siblings (other replies, or edited versions of a question). */
function BranchSwitch({ conv, m, onPick }: { conv: Conversation; m: ChatMessage; onPick: (id: string) => void }) {
  const sibs = versions(conv, m);
  if (sibs.length < 2) return null;
  const i = sibs.findIndex((x) => x.id === m.id);
  const go = (d: number) => {
    const next = sibs[i + d];
    if (next?.id) onPick(next.id);
  };
  return (
    <span className="ai-branch" aria-label={`Version ${i + 1} of ${sibs.length}`}>
      <button type="button" className="ai-icon-btn" disabled={i === 0} onClick={() => go(-1)} aria-label="Previous version"><Icon icon={ChevronLeft} size={12} /></button>
      <span className="num">{i + 1} / {sibs.length}</span>
      <button type="button" className="ai-icon-btn" disabled={i === sibs.length - 1} onClick={() => go(1)} aria-label="Next version"><Icon icon={ChevronRight} size={12} /></button>
    </span>
  );
}

function AssistantMessage({ m, providerName, live, conv, onPick, onRegenerate, busy, held }: {
  m: Partial<ChatMessage> & { content: string };
  providerName: string;
  live?: boolean;
  conv?: Conversation;
  onPick?: (id: string) => void;
  onRegenerate?: () => void;
  busy?: boolean;
  /** Look-only: nothing goes to a model. */
  held?: boolean;
}) {
  const thinking = (m.thinking ?? "").trim();
  return (
    <div className={`msg assistant${live ? " live" : ""}`}>
      <div className="msg-meta">
        <span className="strong">{m.model}</span> <span className="muted">· {providerName}</span>
      </div>
      {thinking && (
        <details className="thinking" open={live && !m.content}>
          <summary><Icon icon={Brain} size={12} /> Thinking</summary>
          <div className="plain">{thinking}</div>
        </details>
      )}
      {m.stopReason === "refusal" && <div className="msg-note">Claude declined this request, so nothing from this turn is sent back next time.</div>}
      {m.content ? <Markdown text={m.content} /> : live && !thinking && <div className="muted ai-waiting">Waiting for the first token…</div>}
      {!live && (
        <div className="msg-foot">
          <span className="num">{statsLine(m.stats, m.stopReason)}</span>
          {conv && onPick && <BranchSwitch conv={conv} m={m as ChatMessage} onPick={onPick} />}
          <button className="tool" onClick={() => copyText(m.content)} title="Copy the reply as Markdown"><Icon icon={Copy} size={12} /> Copy</button>
          {onRegenerate && (
            <button className="tool" onClick={onRegenerate} disabled={busy || held} title={held ? LOOK_ONLY_TIP : "Ask again; the current reply stays as another version"}>
              <Icon icon={RotateCcw} size={12} /> Regenerate
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** Where the system prompt changed: from here on the model follows the new one. */
function SystemChange({ m, conv, onPick }: { m: ChatMessage; conv: Conversation; onPick: (id: string) => void }) {
  const text = m.content.trim();
  const first = text.split("\n")[0];
  return (
    <div className="msg-note" title={text || undefined}>
      {text ? `System prompt changed from here: ${first.length > 80 ? `${first.slice(0, 80)}…` : first}` : "System prompt removed from here"}
      {" "}<BranchSwitch conv={conv} m={m} onPick={onPick} />
    </div>
  );
}

function UserMessage({ m, conv, onPick, onEdit, busy, held }: {
  m: ChatMessage;
  conv: Conversation;
  onPick: (id: string) => void;
  onEdit: (text: string) => void;
  busy: boolean;
  /** Look-only: nothing goes to a model. */
  held: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(m.content);
  if (editing) {
    return (
      <div className="msg user editing">
        <textarea value={text} onChange={(e) => setText(e.target.value)} rows={Math.min(10, text.split("\n").length + 1)} autoFocus
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              if (text.trim() && !busy && !held) {
                setEditing(false);
                onEdit(text.trim());
              }
            } else if (e.key === "Escape") {
              e.stopPropagation();
              setEditing(false);
              setText(m.content);
            }
          }} />
        <div className="ai-edit-actions">
          <button className="btn small" onClick={() => { setEditing(false); setText(m.content); }}>Cancel</button>
          <button className="btn small primary" disabled={!text.trim() || busy || held} title={held ? LOOK_ONLY_TIP : undefined} onClick={() => { setEditing(false); onEdit(text.trim()); }}>Send</button>
        </div>
      </div>
    );
  }
  return (
    <div className="msg user">
      <div className="plain">{m.content}</div>
      <div className="ai-user-foot">
        <BranchSwitch conv={conv} m={m} onPick={onPick} />
        <button className="ai-icon-btn" onClick={() => setEditing(true)} disabled={busy || held} title={held ? LOOK_ONLY_TIP : "Edit and send again (the original stays as another version)"} aria-label="Edit and send again">
          <Icon icon={Pencil} size={12} />
        </button>
      </div>
    </div>
  );
}

export function ChatWindow({ params }: { params: Params }) {
  const wm = useWM();
  const pending = usePendingChat();
  // Look-only holds inference (admin.rs LOOK_ONLY_HOLDS): Send is off, and what's typed stays.
  const held = !!useAdminState().lookOnly;
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [list, setList] = useState<ConversationSummary[]>([]);
  const [conv, setConv] = useState<Conversation | null>(null);
  const [choice, setChoice] = useState<ModelChoice | null>(null);
  const [effort, setEffort] = useState("");
  const [temperature, setTemperature] = useState("");
  const [system, setSystem] = useState("");
  const [showSystem, setShowSystem] = useState(false);
  const [input, setInput] = useState("");
  const [live, setLive] = useState<Live | null>(null);
  const [error, setError] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  /** The conversation and model the "another model" notice was last shown for ("c1|anthropic|claude-sonnet-5"). */
  const switchNoted = useRef("");
  const provider = choice?.provider ?? "";
  const model = choice?.model ?? "";

  // Conversations belong to the data source they were started in: another source shows its own.
  const source = useDataSource();
  const firstSource = useRef(source);
  useEffect(() => {
    if (source === firstSource.current) return;
    firstSource.current = source;
    setConv(null);
    setError(null);
    refreshList();
  }, [source]);

  useCloseGuard(() => (live ? "A reply is still streaming" : input.trim() ? "The message you're writing hasn't been sent" : null));
  useWindowTitle(conv?.title ? `Chat — ${conv.title}` : null);

  const refreshList = () => api.conversationsList().then(setList, () => {});
  const refreshProviders = () => modelsApi.providersList().then(setProviders, () => {});

  const open = async (id: string) => {
    try {
      const c = ensureTree(await modelsApi.conversationGet(id));
      setConv(c);
      setChoice({ provider: c.provider, model: c.model });
      setSystem(systemAt(c, c.leaf));
      setEffort(c.params.effort ?? "");
      setTemperature(c.params.temperature != null ? String(c.params.temperature) : "");
      setError(null);
    } catch (e) {
      setError(errText(e));
    }
  };

  useEffect(() => {
    modelsApi.providersList().then((ps) => {
      setProviders(ps);
      if (params.id || pending) return;
      const usable = ps.filter((p) => p.enabled && !p.notAnswering);
      const recent = recentChoices().find((r) => usable.some((p) => p.id === r.provider && p.visible.includes(r.model)));
      if (recent) return setChoice(recent);
      // With Live data the simulator's model is never the default.
      const pick = usable.find((p) => p.hasKey) ?? (currentSource() === "demo" ? usable.find((p) => p.kind === "demo") : undefined);
      if (pick) setChoice({ provider: pick.id, model: pick.effectiveDefault ?? pick.visible[0] ?? "" });
    });
    refreshList();
    if (params.id) open(params.id);
    // Cluster endpoints come and go: keep the list current.
    const t = window.setInterval(refreshProviders, 10_000);
    return () => window.clearInterval(t);
  }, []);

  // "Chat with this" from Endpoints, Models or the Dashboard: a new chat on that model.
  useEffect(() => {
    if (!pending || live) return;
    takePendingChat(pending.seq);
    refreshProviders();
    setConv(null);
    setSystem("");
    setError(null);
    setChoice({ provider: pending.providerId, model: pending.model });
    rememberChoice({ provider: pending.providerId, model: pending.model });
  }, [pending?.seq]);

  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [conv?.messages.length, conv?.leaf, live?.text, live?.thinking]);

  // The composer fits what's typed, from one line up to eight, then scrolls inside.
  useLayoutEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    const style = getComputedStyle(el);
    const line = parseFloat(style.lineHeight) || 18;
    const pad = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, line * COMPOSER_LINES + pad)}px`;
    el.style.overflowY = el.scrollHeight > line * COMPOSER_LINES + pad ? "auto" : "hidden";
  }, [input]);

  const current = providers.find((p) => p.id === provider);
  const isClaude = current?.kind === "anthropic";
  // Claude and OpenAI's reasoning models take an effort and refuse a temperature (catalog.rs).
  const takesEffort = !!current && (current.effortModels ? current.effortModels.includes(model) : isClaude);
  const takesTemperature = !isClaude && !takesEffort;
  const efforts = isClaude ? CLAUDE_EFFORTS : OPENAI_EFFORTS;
  const effortShown = takesEffort && efforts.includes(effort) ? effort : "";
  const missingKey = !!current && (current.kind === "anthropic" || current.kind === "openai") && !current.hasKey;
  const nameOf = (id?: string | null) => providers.find((p) => p.id === id)?.name ?? id ?? "";
  const shown = conv ? thread(conv) : [];

  const newChat = () => {
    setConv(null);
    setSystem("");
    setError(null);
  };

  /** Adds the system prompt change under `parentId` when the box differs from what's in effect
   * there; the reply then goes under the change. Before the first reply the box is the
   * conversation's own prompt, so nothing is added. */
  const withSystem = (base: Conversation, parentId: string): { conv: Conversation; parentId: string } => {
    if (!answered(base) || system === systemAt(base, parentId)) return { conv: base, parentId };
    const change: ChatMessage = { id: newId(), role: SYSTEM as ChatMessage["role"], content: system, createdAt: nowS() };
    return { conv: append(base, change, parentId), parentId: change.id! };
  };

  /** Streams a reply to `history` (ending in a user message, or a system prompt change after one) and adds it under that message. */
  const reply = async (base: Conversation, parentId: string) => {
    if (!current || !model) return;
    const params: ChatParams = {
      effort: effortShown || null,
      temperature: takesTemperature && temperature !== "" ? Number(temperature) : null,
      maxTokens: null,
    };
    // The top-level prompt is the one the first reply came under; later changes are in the history.
    const top = answered(base) ? base.system : system;
    const history = thread(base, parentId);
    const requestId = newId();
    const result: { done?: ChatMessage; failure?: string } = {};
    // After a switch to another model every request still carries the earlier model's reasoning,
    // which Claude leaves out each time and says so, once per reply it left out. That's said once
    // per switch: on the first reply on the new model, in one line.
    const before = [...history].reverse().find((m) => m.role === "assistant");
    const switchKey = `${base.id}|${provider}|${model}`;
    let sayOtherModel = !!before && (before.provider !== provider || before.model !== model) && switchNoted.current !== switchKey;
    setLive({ requestId, text: "", thinking: "", notices: [], parentId, started: Date.now() });
    try {
      await chatSend({ requestId, provider, model, system: top, params, messages: history }, (ev) => {
        if (ev.type === "text") setLive((l) => l && { ...l, text: l.text + ev.text });
        else if (ev.type === "thinking") setLive((l) => l && { ...l, thinking: l.thinking + ev.text });
        else if (ev.type === "notice" && OTHER_MODEL.test(ev.text) && !sayOtherModel) return;
        else if (ev.type === "notice") {
          if (OTHER_MODEL.test(ev.text)) {
            sayOtherModel = false;
            switchNoted.current = switchKey;
          }
          setLive((l) => l && { ...l, notices: [...l.notices, ev.text] });
        }
        else if (ev.type === "done") result.done = ev.message;
        else result.failure = ev.message;
      });
    } catch (e) {
      result.failure = errText(e);
    }
    const warnings = result.failure ? [result.failure] : [];
    if (result.done) {
      const finished = { ...append(base, { ...result.done, createdAt: nowS() }, parentId), updatedAt: nowS(), provider, model, params, system: top };
      setConv(finished);
      await modelsApi.conversationSave(finished).catch((e) => {
        warnings.push(`The reply is visible but could not be saved. Copy it before closing this conversation. ${errText(e)}`);
      });
    }
    if (warnings.length) setError((earlier) => [earlier, ...warnings].filter(Boolean).join(" "));
    setLive(null);
    refreshList();
  };

  const send = async () => {
    const text = input.trim();
    if (!text || live || !current || !model || held) return;
    const t = nowS();
    const base: Conversation = conv ?? {
      id: newId(), title: text.slice(0, 70), createdAt: t, updatedAt: t,
      provider, model, system, params: {}, messages: [], leaf: null,
    };
    const user: ChatMessage = { id: newId(), role: "user", content: text, createdAt: t };
    const asked = { ...append(base, user, base.leaf ?? null), provider, model, system: answered(base) ? base.system : system, updatedAt: t };
    const { conv: withUser, parentId } = withSystem(asked, user.id!);
    setConv(withUser);
    setInput("");
    setError(null);
    rememberChoice({ provider, model });
    // A new conversation shows in the list right away, before the save and the reply.
    if (!conv) {
      setList((l) => [{ id: withUser.id, title: withUser.title, updatedAt: t, provider, model, messageCount: 1 } as ConversationSummary, ...l.filter((c) => c.id !== withUser.id)]);
    }
    await modelsApi.conversationSave(withUser).catch((e) => setError(errText(e)));
    await reply(withUser, parentId);
  };

  /** Asks the same question again. With a changed system prompt the new reply comes after that change. */
  const regenerate = async (m: ChatMessage) => {
    if (!conv || !m.parentId || live) return;
    setError(null);
    const parent = conv.messages.find((x) => x.id === m.parentId);
    // A reply after a system prompt change hangs under the change, which hangs under the question.
    const question = parent && isSystem(parent) ? parent.parentId : m.parentId;
    if (!question) return;
    if (parent && isSystem(parent) && parent.content === system) return reply(conv, parent.id!);
    const { conv: next, parentId } = withSystem(conv, question);
    if (next !== conv) setConv(next);
    await reply(next, parentId);
  };

  const editAndResend = async (m: ChatMessage, text: string) => {
    if (!conv || live) return;
    const user: ChatMessage = { id: newId(), role: "user", content: text, createdAt: nowS() };
    const { conv: edited, parentId } = withSystem(append(conv, user, m.parentId ?? null), user.id!);
    setConv(edited);
    setError(null);
    await reply(edited, parentId);
  };

  const pickBranch = (id: string) => {
    if (!conv) return;
    const next = { ...conv, leaf: latestLeaf(conv, id) };
    setConv(next);
    setSystem(systemAt(next, next.leaf));
    modelsApi.conversationSave(next).catch(() => {});
  };

  const remove = async (c: ConversationSummary) => {
    const ok = await confirmAction({ title: "Delete this conversation?", body: c.title, confirmLabel: "Delete", danger: true });
    if (!ok) return;
    await api.conversationDelete(c.id);
    if (conv?.id === c.id) newChat();
    refreshList();
    toast({ kind: "success", title: "Conversation deleted" });
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      send();
    } else if (e.key === "Escape" && live) {
      api.chatStop(live.requestId);
    }
  };

  const lastAssistant = [...shown].reverse().find((m) => m.role === "assistant");
  const liveTarget = live ? shown.findIndex((m) => m.id === live.parentId) : -1;
  const visible = live && liveTarget >= 0 ? shown.slice(0, liveTarget + 1) : shown;

  return (
    <div className="col fill ai-chat">
      <Toolbar label="Chat">
        <ModelPicker providers={providers} value={choice} onChange={setChoice} disabled={!!live} />
        {takesEffort ? (
          <>
            <select value={effortShown} onChange={(e) => setEffort(e.target.value)} aria-label="Effort"
              title={isClaude ? "How hard Claude thinks (Claude doesn't take a temperature)" : "How hard the model reasons (reasoning models don't take a temperature)"}>
              <option value="">Default effort</option>
              {efforts.map((x) => <option key={x} value={x}>{x} effort</option>)}
            </select>
            <HelpTip topic="ai.chat.effort" />
          </>
        ) : takesTemperature ? (
          <>
            <input className="narrow" type="number" min={0} max={2} step={0.1} value={temperature} onChange={(e) => setTemperature(e.target.value)}
              placeholder="temp" aria-label="Temperature" title="Temperature (blank = the model's default)" />
            <HelpTip topic="ai.chat.temperature" />
          </>
        ) : null}
        <button type="button" className={`tool${showSystem || system ? " on" : ""}`} onClick={() => setShowSystem(!showSystem)}
          title={system ? "System prompt (set for this conversation)" : "Add a system prompt: how the model should behave"} aria-pressed={showSystem}>
          <Icon icon={ScrollText} size={14} /><span className="tool-label">System</span>
        </button>
        <HelpTip topic="ai.chat.system-prompt" />
        <Spacer />
        <ToolButton icon={SquarePen} label="New chat" onClick={newChat} disabled={!!live} />
      </Toolbar>

      <div className="chat">
        <aside className="chat-list" aria-label="Conversations">
          {list.length === 0 && <div className="muted pad">No conversations yet.</div>}
          {list.map((c) => (
            <div key={c.id} className={`chat-item${conv?.id === c.id ? " active" : ""}`} onClick={() => !live && open(c.id)}>
              <div className="chat-item-title">{c.title || "Untitled"}</div>
              <div className="chat-item-meta">{c.model} · {live && conv?.id === c.id ? "Sending…" : <Ago ts={c.updatedAt} />}</div>
              <button className="chat-item-del" title="Delete" aria-label={`Delete ${c.title}`} onClick={(e) => { e.stopPropagation(); remove(c); }}><Icon icon={Trash2} size={12} /></button>
            </div>
          ))}
        </aside>

        <section className="chat-main">
          {showSystem && (
            <div className="system-box">
              <textarea value={system} onChange={(e) => setSystem(e.target.value)} aria-label="System prompt" rows={3}
                placeholder={conv && answered(conv)
                  ? "System prompt: a change applies from your next message; earlier replies keep the one they had"
                  : "System prompt (optional): how the model should behave in this conversation"} />
            </div>
          )}
          {missingKey && (
            <Notice tone="warn" action={{ label: "Open Providers", onClick: () => wm.open("providers") }}>
              <Icon icon={KeyRound} size={12} /> Add your {current?.name} API key to chat with it.
            </Notice>
          )}
          {current?.notAnswering && <Notice tone="warn">{current.name} isn't answering right now. A message may fail until it's back.</Notice>}
          <div className="chat-scroll" ref={scrollRef}>
            {!conv && !live && (
              <EmptyState icon={MessageSquare} title="Chat with any model SparkBox can reach"
                body={source === "live"
                  ? "Pick a model from your providers or the cluster. Edit a question or regenerate a reply to compare; every version is kept."
                  : "Pick one above: your cluster's engines, OpenAI and Claude all work here, and the demo model works with no setup. Edit a question or regenerate a reply to compare; every version is kept."} />
            )}
            {conv && visible.map((m) =>
              m.role === "user" ? (
                <UserMessage key={m.id} m={m} conv={conv} onPick={pickBranch} onEdit={(t) => editAndResend(m, t)} busy={!!live} held={held} />
              ) : isSystem(m) ? (
                <SystemChange key={m.id} m={m} conv={conv} onPick={pickBranch} />
              ) : (
                <AssistantMessage key={m.id} m={m} conv={conv} providerName={nameOf(m.provider)} onPick={pickBranch}
                  onRegenerate={m.id === lastAssistant?.id ? () => regenerate(m) : undefined} busy={!!live} held={held} />
              ),
            )}
            {live && (
              <>
                {live.notices.map((n, i) => <div key={i} className="msg-note">{n}</div>)}
                <AssistantMessage live m={{ content: live.text, thinking: live.thinking, model }} providerName={nameOf(provider)} />
              </>
            )}
          </div>
          {error && <Notice tone="error" onDismiss={() => setError(null)}>{error}</Notice>}
          <div className="composer" onClick={() => inputRef.current?.focus()}>
            <textarea
              ref={inputRef}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={onKey}
              aria-label="Message"
              placeholder={missingKey ? "Add an API key first, or pick another model" : "Message (Enter to send, Shift+Enter for a new line)"}
              rows={1}
            />
            {live ? (
              <button type="button" className="composer-send stop" onClick={() => api.chatStop(live.requestId)} title="Stop (Esc)" aria-label="Stop">
                <Icon icon={StopIcon} size={14} />
              </button>
            ) : (
              <button type="button" className="composer-send" onClick={send} disabled={!input.trim() || !model.trim() || missingKey || held} title={held ? LOOK_ONLY_TIP : "Send (Enter)"} aria-label="Send">
                <Icon icon={ArrowUp} size={16} />
              </button>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
