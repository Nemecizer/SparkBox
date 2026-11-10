//! "Did it come back?" for operations that touch serving: a restart of containers or
//! nodes, a deployment restart, a supervisor's retry. `affected_in` names the endpoints an
//! operation reaches; `wait_on` follows them through the endpoint rounds until each
//! answers and generates (Ready, with how long it took) or stays down (with the reason).
//! Only a test token sent after the operation started counts, so a result is never read
//! from the state before it, nor from an answer to /health or the model list alone. The one
//! exception is an endpoint no token can go to (it wants a key SparkBox doesn't have, look-only
//! holds test tokens, or it's an Ollama at rest with nothing in memory): an answer in a round
//! that began after the operation is all there is, and its step says it wasn't tested.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::endpoints::{Endpoint, Health};
use super::{now_ms, Models};

/// What an operation touched.
#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    /// Node ids (a reboot, a shutdown).
    Nodes(Vec<String>),
    /// (node, container id or name) pairs (a container restart, stop or kill).
    Containers(Vec<(String, String)>),
    /// A SparkBox load's id.
    Deployment(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointRef {
    pub id: String,
    /// What the operation's step says: "llama-3.3-70b".
    pub model: String,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VerifyOutcome {
    /// `untested`: why no test token could go to it (it answered its model list only).
    Ready {
        after_s: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        untested: Option<String>,
    },
    Down { reason: String },
}

impl VerifyOutcome {
    /// The step's detail: "llama-3.3-70b answered after 2m 10s", "… answered after 45s (its model
    /// list; not tested: …)" or "llama-3.3-70b is down: …".
    pub fn sentence(&self, r: &EndpointRef) -> String {
        match self {
            VerifyOutcome::Ready { after_s, untested: None } => format!("{} answered after {}", r.model, duration(*after_s)),
            VerifyOutcome::Ready { after_s, untested: Some(why) } => format!("{} answered after {} (its model list; not tested: {why})", r.model, duration(*after_s)),
            VerifyOutcome::Down { reason } => format!("{} is down: {reason}", r.model),
        }
    }
}

/// "45s", "2m 10s".
pub fn duration(s: u64) -> String {
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

/// How long a Down (or vanished) endpoint is given to start coming back before it counts as down.
const SETTLE: Duration = Duration::from_secs(20);

fn reference(e: &Endpoint) -> EndpointRef {
    EndpointRef { id: e.id.clone(), model: super::endpoints::label_of(e), nodes: e.nodes.clone() }
}

/// The endpoints in `list` an operation on `scope` reaches.
pub fn affected_in(list: &[Endpoint], scope: &Scope) -> Vec<EndpointRef> {
    list.iter()
        .filter(|e| match scope {
            Scope::Nodes(nodes) => e.nodes.iter().chain(&e.expected_nodes).any(|n| nodes.contains(n)),
            Scope::Containers(cs) => cs.iter().any(|(node, c)| {
                e.members.iter().any(|m| &m.node == node && (&m.id == c || &m.name == c || (c.len() >= 12 && m.id.starts_with(c.as_str()))))
                    || (&e.head_node == node && (e.container_ids.contains(c) || e.container_names.contains(c)))
            }),
            Scope::Deployment(id) => e.deployment_id.as_deref() == Some(id.as_str()),
        })
        .map(reference)
        .collect()
}

/// Waits until every endpoint answers again or `timeout` runs out, polling Models every `poll`
/// (the endpoint rounds run elsewhere). `on_progress` sees each endpoint as each round finds it
/// (for a task step: "Loading weights 42%").
pub async fn wait_on(models: &Arc<Models>, refs: &[EndpointRef], timeout: Duration, poll: Duration, on_progress: impl Fn(&EndpointRef, &Endpoint)) -> Vec<(EndpointRef, VerifyOutcome)> {
    let started = Instant::now();
    let started_ms = now_ms();
    let first_round = models.round();
    // The rounds send each one a test token after this round, live traffic or not.
    let mark = |on: bool| {
        let mut st = models.eps.lock().unwrap();
        for r in refs {
            let t = st.tracks.entry(r.id.clone()).or_default();
            if on {
                t.verify_after = Some(t.verify_after.map_or(first_round, |v| v.max(first_round)));
            } else if t.verify_after == Some(first_round) {
                t.verify_after = None;
            }
        }
    };
    mark(true);
    let mut done: Vec<Option<VerifyOutcome>> = vec![None; refs.len()];
    let mut down_since: Vec<Option<Instant>> = vec![None; refs.len()];
    loop {
        // Only rounds that ran after the operation began say anything about its outcome.
        if models.round() > first_round {
            let list = models.endpoints();
            for (i, r) in refs.iter().enumerate() {
                if done[i].is_some() {
                    continue;
                }
                match list.iter().find(|e| e.id == r.id) {
                    Some(e) => {
                        on_progress(r, e);
                        // It generated since: a test token sent after a round that began after the
                        // operation (an answer to /health or its model list alone isn't enough)…
                        let answered = e.canary.as_ref().is_some_and(|c| c.ok && c.after_round.is_some_and(|r| r > first_round));
                        // …unless no token can go to it (it wants a key SparkBox doesn't have, or
                        // look-only holds them): then an answer in a round that began after it is
                        // all there is, as the Ready card's note says.
                        let untested = models.eps.lock().unwrap().tracks.get(&r.id).and_then(|t| {
                            if t.canary_held {
                                Some("look-only mode holds test tokens")
                            } else if t.untestable {
                                Some("it wants an API key SparkBox doesn't have")
                            } else {
                                None
                            }
                        });
                        let by_answer = !answered && untested.is_some() && e.last_ok_ms.is_some_and(|t| t >= started_ms);
                        // An Ollama at rest never reads Ready and has nothing to test: an answer
                        // in a round that began after the operation is all it can give.
                        let idle = e.health == Health::Idle && e.last_ok_ms.is_some_and(|t| t >= started_ms);
                        if e.health == Health::Ready && (answered || by_answer) {
                            let untested = by_answer.then(|| untested.unwrap_or_default().to_string());
                            done[i] = Some(VerifyOutcome::Ready { after_s: started.elapsed().as_secs(), untested });
                        } else if idle {
                            done[i] = Some(VerifyOutcome::Ready { after_s: started.elapsed().as_secs(), untested: Some("nothing in memory to test".into()) });
                        } else if matches!(e.health, Health::Down | Health::Stopped) {
                            let since = *down_since[i].get_or_insert_with(Instant::now);
                            if since.elapsed() >= SETTLE {
                                done[i] = Some(VerifyOutcome::Down { reason: e.health_reason.clone().unwrap_or_else(|| "not answering".into()) });
                            }
                        } else {
                            down_since[i] = None;
                        }
                    }
                    None => {
                        let since = *down_since[i].get_or_insert_with(Instant::now);
                        if since.elapsed() >= SETTLE {
                            done[i] = Some(VerifyOutcome::Down { reason: "It isn't running on the cluster any more".into() });
                        }
                    }
                }
            }
        }
        if done.iter().all(Option::is_some) {
            break;
        }
        if started.elapsed() >= timeout {
            let list = models.endpoints();
            for (i, r) in refs.iter().enumerate() {
                if done[i].is_none() {
                    let why = list.iter().find(|e| e.id == r.id).and_then(|e| e.health_reason.clone());
                    let reason = match why {
                        Some(w) => format!("Didn't answer within {} ({w})", duration(timeout.as_secs())),
                        None => format!("Didn't answer within {}", duration(timeout.as_secs())),
                    };
                    done[i] = Some(VerifyOutcome::Down { reason });
                }
            }
            break;
        }
        tokio::time::sleep(poll).await;
    }
    mark(false);
    refs.iter().cloned().zip(done.into_iter().map(|d| d.unwrap())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::endpoints::{Member, Parallel};
    use crate::models::now_ms;

    fn llama() -> Endpoint {
        Endpoint {
            id: "cell1-vllm-8000".into(),
            models: vec!["llama-3.3-70b".into()],
            nodes: vec!["cell1".into(), "cell2".into(), "cell3".into()],
            expected_nodes: vec!["cell1".into(), "cell2".into(), "cell3".into()],
            head_node: "cell1".into(),
            parallel: Some(Parallel { tp: 1, pp: 3 }),
            container_ids: vec!["a1b2c3d4e5f6a7b8".into()],
            container_names: vec!["vllm-head".into()],
            members: vec![
                Member { node: "cell1".into(), head: true, id: "a1b2c3d4e5f6a7b8".into(), name: "vllm-head".into(), state: "running".into(), ..Default::default() },
                Member { node: "cell3".into(), id: "ffff00001111".into(), name: "vllm-worker".into(), state: "running".into(), ..Default::default() },
            ],
            ..Default::default()
        }
    }

    fn ollama() -> Endpoint {
        Endpoint { id: "gx10-0000-ollama-11434".into(), models: vec!["gpt-oss:20b".into()], nodes: vec!["gx10-0000".into()], head_node: "gx10-0000".into(), ..Default::default() }
    }

    #[test]
    fn affected_finds_the_endpoints_an_operation_reaches() {
        let list = vec![llama(), ollama(), Endpoint { deployment_id: Some("m1".into()), id: "x".into(), models: vec!["qwen".into()], ..Default::default() }];
        let ids = |s: Scope| affected_in(&list, &s).into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(Scope::Nodes(vec!["cell3".into()])), ["cell1-vllm-8000"], "a pipeline stage's node");
        assert_eq!(ids(Scope::Nodes(vec!["gx10-0000".into(), "cell1".into()])), ["cell1-vllm-8000", "gx10-0000-ollama-11434"]);
        assert_eq!(ids(Scope::Containers(vec![("cell3".into(), "vllm-worker".into())])), ["cell1-vllm-8000"], "a worker container");
        assert_eq!(ids(Scope::Containers(vec![("cell1".into(), "a1b2c3d4e5f6".into())])), ["cell1-vllm-8000"], "a short id");
        assert!(ids(Scope::Containers(vec![("cell2".into(), "vllm-worker".into())])).is_empty(), "same name, other node");
        assert_eq!(ids(Scope::Deployment("m1".into())), ["x"]);
        let r = &affected_in(&list, &Scope::Nodes(vec!["cell2".into()]))[0];
        assert_eq!(r.model, "llama-3.3-70b");
        assert_eq!(VerifyOutcome::Ready { after_s: 130, untested: None }.sentence(r), "llama-3.3-70b answered after 2m 10s");
        assert_eq!(
            VerifyOutcome::Ready { after_s: 45, untested: Some("it wants an API key SparkBox doesn't have".into()) }.sentence(r),
            "llama-3.3-70b answered after 45s (its model list; not tested: it wants an API key SparkBox doesn't have)"
        );
        assert_eq!(VerifyOutcome::Down { reason: "Head exited (code 1) just now".into() }.sentence(r), "llama-3.3-70b is down: Head exited (code 1) just now");
    }

    /// Models with nothing behind it: the test sets its endpoint list and round by hand.
    fn bare() -> Arc<Models> {
        let dir = crate::fake_node::temp("verify-bare");
        let vpn = crate::vpn::Vpn::new(dir.join("vpn.json"), dir.join("no-helper"), |_| {});
        let live = crate::live::Live::new(vpn.clone(), dir.join("known_hosts.json"), || Ok(None));
        Models::new(dir.clone(), live, crate::llm::Llm::new(dir.join("providers.json"), vpn), None)
    }

    fn round(models: &Models, e: Endpoint) {
        let mut st = models.eps.lock().unwrap();
        st.list = vec![e];
        st.round += 1;
    }

    #[tokio::test]
    async fn only_a_test_token_sent_after_the_operation_makes_it_ready() {
        use crate::models::endpoints::Canary;
        let models = bare();
        let refs = vec![EndpointRef { id: "cell2-vllm-8000".into(), model: "gemma-3-27b-it".into(), nodes: vec!["cell2".into()] }];
        let ready = |canary: Canary| Endpoint { id: "cell2-vllm-8000".into(), health: Health::Ready, last_ok_ms: Some(now_ms() + 60_000), canary: Some(canary), ..Default::default() };
        let token = |after_round: Option<u64>| Canary { at_ms: now_ms(), ok: true, latency_ms: Some(80), error: None, after_round };
        // Before the operation: round 3 has run, and its last token went out then.
        round(&models, ready(token(Some(2))));
        round(&models, ready(token(Some(2))));
        round(&models, ready(token(Some(2))));
        let first = models.round();
        let m = models.clone();
        let feeder = tokio::spawn(async move {
            // Rounds that answer /health and the model list, with only the old token, or one sent
            // while the round that was running when the operation began hadn't ended.
            for i in 0..6 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                round(&m, ready(token(Some(if i < 3 { 2 } else { first }))));
            }
            round(&m, ready(token(Some(first + 1))));
        });
        let seen = std::sync::Mutex::new(0);
        let marked = std::sync::Mutex::new(None);
        let out = wait_on(&models, &refs, Duration::from_secs(5), Duration::from_millis(10), |r, _| {
            *seen.lock().unwrap() += 1;
            *marked.lock().unwrap() = models.eps.lock().unwrap().tracks.get(&r.id).and_then(|t| t.verify_after);
        })
        .await;
        feeder.await.unwrap();
        assert_eq!(*marked.lock().unwrap(), Some(first), "while it waits, the rounds owe it a test token");
        assert_eq!(models.eps.lock().unwrap().tracks.get("cell2-vllm-8000").and_then(|t| t.verify_after), None, "and not once it's done");
        assert!(matches!(out[0].1, VerifyOutcome::Ready { .. }), "{:?}", out[0].1);
        assert!(*seen.lock().unwrap() >= 6, "it waited through the rounds without a fresh token");

        // Without a fresh token it isn't Ready, however recently it answered: it times out.
        let m = models.clone();
        let stale = models.round();
        let feeder = tokio::spawn(async move {
            for _ in 0..20 {
                round(&m, ready(token(Some(stale))));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let out = wait_on(&models, &refs, Duration::from_millis(300), Duration::from_millis(10), |_, _| {}).await;
        feeder.await.unwrap();
        assert!(matches!(&out[0].1, VerifyOutcome::Down { reason } if reason.starts_with("Didn't answer within")), "{:?}", out[0].1);
        // A token whose round isn't known (one parsed but never sent by the rounds) never counts.
        let m = models.clone();
        let feeder = tokio::spawn(async move {
            for _ in 0..10 {
                round(&m, ready(token(None)));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let out = wait_on(&models, &refs, Duration::from_millis(150), Duration::from_millis(10), |_, _| {}).await;
        feeder.await.unwrap();
        assert!(matches!(out[0].1, VerifyOutcome::Down { .. }), "{:?}", out[0].1);
    }

    /// An endpoint no test token can go to (it wants a key SparkBox doesn't have, or look-only
    /// holds test tokens) is Ready on an answer in a round that began after the operation, never
    /// on one from before it.
    #[tokio::test]
    async fn an_endpoint_that_cant_be_tested_is_verified_by_an_answer_after_the_operation() {
        use crate::models::endpoints::{canary_due, Track};
        for held in [false, true] {
            let models = bare();
            let id = "cell1-vllm-8000";
            let refs = vec![EndpointRef { id: id.into(), model: "llama-3.3-70b".into(), nodes: vec!["cell1".into()] }];
            let note = if held { "Not tested: look-only mode holds test tokens" } else { "Not tested: it wants an API key SparkBox doesn't have" };
            let ready = move |last_ok_ms: u64| Endpoint { id: id.into(), health: Health::Ready, health_note: Some(note.into()), last_ok_ms: Some(last_ok_ms), canary: None, ..Default::default() };
            let tried = now_ms() - 1_000;
            models.eps.lock().unwrap().tracks.insert(id.into(), Track { untestable: !held, canary_held: held, canary_tried_ms: Some(tried), ..Default::default() });
            if !held {
                // No token is due for it within the minute, verify or not: none can come back.
                assert!(!canary_due(&Track { untestable: true, canary_tried_ms: Some(tried), verify_after: Some(3), ..Default::default() }, now_ms()));
            }
            // Its last answer was before the operation: that's no answer since.
            let before = now_ms() - 5_000;
            let m = models.clone();
            let feeder = tokio::spawn(async move {
                for _ in 0..20 {
                    round(&m, ready(before));
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            });
            let out = wait_on(&models, &refs, Duration::from_millis(300), Duration::from_millis(10), |_, _| {}).await;
            feeder.await.unwrap();
            assert!(matches!(&out[0].1, VerifyOutcome::Down { reason } if reason.starts_with("Didn't answer within")), "held {held}: {:?}", out[0].1);
            // It answers in the rounds after the operation: Ready.
            let m = models.clone();
            let feeder = tokio::spawn(async move {
                for _ in 0..20 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    round(&m, ready(now_ms()));
                }
            });
            let out = wait_on(&models, &refs, Duration::from_secs(5), Duration::from_millis(10), |_, _| {}).await;
            feeder.await.unwrap();
            let why = if held { "look-only mode holds test tokens" } else { "it wants an API key SparkBox doesn't have" };
            assert!(matches!(&out[0].1, VerifyOutcome::Ready { untested: Some(w), .. } if w == why), "held {held}: {:?}", out[0].1);
            // A testable one that answers but sends no token back still isn't.
            models.eps.lock().unwrap().tracks.insert(id.into(), Track::default());
            let m = models.clone();
            let feeder = tokio::spawn(async move {
                for _ in 0..20 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    round(&m, ready(now_ms()));
                }
            });
            let out = wait_on(&models, &refs, Duration::from_millis(300), Duration::from_millis(10), |_, _| {}).await;
            feeder.await.unwrap();
            assert!(matches!(out[0].1, VerifyOutcome::Down { .. }), "held {held}: {:?}", out[0].1);
        }
    }
    /// An Ollama with nothing in memory never reads Ready and has nothing to test: an answer after
    /// the operation is all it can give. One from before it isn't.
    #[tokio::test]
    async fn an_idle_ollama_that_answered_after_the_operation_is_verified_untested() {
        let models = bare();
        let id = "cell3-ollama-11434";
        let refs = vec![EndpointRef { id: id.into(), model: "Ollama on cell3".into(), nodes: vec!["cell3".into()] }];
        let idle = move |last_ok_ms: u64| Endpoint { id: id.into(), engine: crate::models::endpoints::Engine::Ollama, health: Health::Idle, health_reason: Some("Nothing in memory".into()), last_ok_ms: Some(last_ok_ms), ..Default::default() };
        let before = now_ms() - 5_000;
        let m = models.clone();
        let feeder = tokio::spawn(async move {
            for _ in 0..20 {
                round(&m, idle(before));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let out = wait_on(&models, &refs, Duration::from_millis(300), Duration::from_millis(10), |_, _| {}).await;
        feeder.await.unwrap();
        assert!(matches!(&out[0].1, VerifyOutcome::Down { reason } if reason.starts_with("Didn't answer within")), "{:?}", out[0].1);

        let m = models.clone();
        let feeder = tokio::spawn(async move {
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                round(&m, idle(now_ms()));
            }
        });
        let out = wait_on(&models, &refs, Duration::from_secs(5), Duration::from_millis(10), |_, _| {}).await;
        feeder.await.unwrap();
        assert!(matches!(&out[0].1, VerifyOutcome::Ready { untested: Some(w), .. } if w == "nothing in memory to test"), "{:?}", out[0].1);
        assert!(out[0].1.sentence(&refs[0]).ends_with("(its model list; not tested: nothing in memory to test)"), "{}", out[0].1.sentence(&refs[0]));
    }
}
