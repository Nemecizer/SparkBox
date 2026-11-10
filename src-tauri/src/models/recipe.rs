//! Recipes: what a model needs to load, described rather than scripted (P2.1). A recipe holds which
//! folders it's for, the image, the splits it allows, its environment, its vLLM options, its
//! defaults and its parsers. The built-ins are the JSON files in recipes/; a bad one fails a test.
//! Nothing here says a recipe works: that's recorded when a load passes readiness.
//!
//! A recipe fills in the spec at deploy (`apply`), never from the UI, so a Restart rebuilds the same
//! files. A placeholder holds a model's place until it can really be loaded, and refuses with its
//! note.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fit::{self, ArchFacts};
use super::plan::{self, DeploySpec, TopologyOffer};

const BUILT_INS: [&str; 5] = [
    include_str!("recipes/qwen3-1.7b.json"),
    include_str!("recipes/qwen3.8-27b-fp8.json"),
    include_str!("recipes/gemma-3-27b-it.json"),
    include_str!("recipes/glm-5.3-flash.json"),
    include_str!("recipes/generic.json"),
];

/// The id of the recipe that catches every folder nothing else matches.
pub const GENERIC: &str = "generic";

const PLACEHOLDER: &str = "placeholder";

/// One vLLM option of a recipe: a flag, its value if it takes one, or a JSON value validated here
/// and passed on as one argv word.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecipeArg {
    pub flag: String,
    pub value: Option<String>,
    pub json: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    /// A share this recipe has a reason for: the Load page keeps it instead of sizing the share to
    /// the model. None of the built-ins has one.
    pub share: Option<f64>,
    pub context: Option<u64>,
    pub seqs: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recipe {
    pub id: String,
    pub title: String,
    /// Words of a repo or folder name, lower case. The longest that fits wins.
    #[serde(rename = "match")]
    pub matches: Vec<String>,
    pub image: String,
    /// "native", or "placeholder": it holds a place and refuses to load.
    pub backend: String,
    /// `plan::topology_id`'s words, and "replicas", offered from P2.4 on.
    pub topologies: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub args: Vec<RecipeArg>,
    pub mounts: Vec<String>,
    pub defaults: Defaults,
    pub tool_parser: Option<String>,
    pub reasoning_parser: Option<String>,
    pub multi_node_docker: String,
    pub note: Option<String>,
}

/// A load that passed readiness with a recipe (`<data>/recipes-verified.json`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Verified {
    pub image_id: String,
    pub topology: String,
    pub node_count: u64,
    pub at_ms: u64,
}

/// A recipe another row of the menu could use.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecipeChoice {
    pub id: String,
    pub title: String,
}

/// A recipe as the UI shows it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecipeView {
    pub id: String,
    pub title: String,
    pub image: String,
    pub topologies: Vec<String>,
    pub defaults: Defaults,
    pub tool_parser: Option<String>,
    pub reasoning_parser: Option<String>,
    pub note: Option<String>,
    /// It refuses to load: `note` says why.
    pub placeholder: bool,
    /// Every recipe that fits this folder, this one first (`view_for`). A menu when there are two.
    pub fits: Vec<RecipeChoice>,
    /// Filled in from `<data>/recipes-verified.json`; empty until a load passes readiness.
    pub verified_on: Vec<Verified>,
}

impl Recipe {
    pub fn placeholder(&self) -> bool {
        self.backend == PLACEHOLDER
    }

    /// The vLLM options, one argv word each.
    pub fn argv(&self) -> Vec<String> {
        let mut out = Vec::new();
        for a in &self.args {
            out.push(a.flag.clone());
            out.extend(a.value.clone());
            out.extend(a.json.as_ref().map(Value::to_string));
        }
        out
    }

    pub fn allows(&self, tp: u64, pp: u64) -> bool {
        self.topologies.contains(&plan::topology_id(tp, pp))
    }

    fn not_listed(&self) -> String {
        format!("The {} recipe doesn't list this split. Pick {} to try it.", self.title, generic().title)
    }

    /// `plan::topologies`, as this recipe has it: a split the model can't take keeps its own
    /// sentence, and one this recipe doesn't list is refused here. A placeholder refuses them all
    /// with its note: nothing else on its folder can load it, so no other choice is named.
    pub fn offers(&self, arch: &ArchFacts, nodes: usize) -> Vec<TopologyOffer> {
        let mut offers = plan::topologies(arch, nodes);
        let why = if self.placeholder() { self.note.clone() } else { Some(self.not_listed()) };
        for o in offers.iter_mut().filter(|o| self.placeholder() || (o.ok && (!self.allows(o.tp, o.pp) || (o.replicas > 1 && !self.topologies.iter().any(|t| t == "replicas"))))) {
            o.ok = false;
            o.unlisted = !self.placeholder();
            o.reason = why.clone();
        }
        offers
    }

    pub(super) fn view(&self, fits: Vec<RecipeChoice>) -> RecipeView {
        RecipeView {
            id: self.id.clone(),
            title: self.title.clone(),
            image: self.image.clone(),
            topologies: self.topologies.clone(),
            defaults: self.defaults.clone(),
            tool_parser: self.tool_parser.clone(),
            reasoning_parser: self.reasoning_parser.clone(),
            note: self.note.clone(),
            placeholder: self.placeholder(),
            fits,
            verified_on: Vec::new(),
        }
    }
}

/// Environment names a recipe may set besides NCCL_* and VLLM_*.
const ENV_NAMES: &[&str] = &["CUTE_DSL_ARCH", "B12X_POLICY_MODE", "B12X_W4A16_A4_PREFILL", "OMP_NUM_THREADS", "PYTORCH_CUDA_ALLOC_CONF", "TOKENIZERS_PARALLELISM", "TRANSFORMERS_VERBOSITY"];

/// What the NCCL_ and VLLM_ families mustn't set, with any name that has PLUGIN in it, as
/// sparkjob.py's ENV_CODE (its test compares the two): these load code or write a file.
const ENV_CODE: &[&str] = &["VLLM_LOGGING_CONFIG_PATH", "NCCL_DEBUG_FILE"];

/// A recipe's environment: no secrets, nothing `plan::rank_env` sets itself, and names from the
/// list. The name is the caller's own text, so the words can quote it; a value is never quoted.
pub fn check_env(env: &BTreeMap<String, String>) -> Result<(), String> {
    for (name, value) in env {
        let secret = name == plan::KEY_ENV || name == "HF_TOKEN" || ["_KEY", "_TOKEN", "_SECRET", "_PASSWORD"].iter().any(|s| name.ends_with(s));
        if secret {
            return Err(format!("{name} looks like a secret, and secrets never go in a recipe."));
        }
        if plan::RANK_ENV.contains(&name.as_str()) {
            return Err(format!("{name} is set by SparkBox for each node."));
        }
        if name.contains("PLUGIN") || ENV_CODE.contains(&name.as_str()) {
            return Err(format!("{name} loads code or writes a file, so a recipe can't set it."));
        }
        if !(ENV_NAMES.contains(&name.as_str()) || name.starts_with("NCCL_") || name.starts_with("VLLM_")) {
            return Err(format!("{name} isn't on SparkBox's list of environment names."));
        }
        if !name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
            return Err(format!("{name} isn't a plain environment name."));
        }
        if value.contains(['\0', '\n', '\r']) {
            return Err(format!("{name} has a line break in its value."));
        }
    }
    Ok(())
}

fn builtin() -> &'static [Recipe] {
    static LOADED: OnceLock<Vec<Recipe>> = OnceLock::new();
    LOADED.get_or_init(|| BUILT_INS.iter().map(|text| serde_json::from_str(text).expect("a built-in recipe parses (see its test)")).collect())
}

fn generic() -> &'static Recipe {
    builtin().iter().find(|r| r.id == GENERIC).expect("the generic recipe is built in (see its test)")
}

pub fn by_id(id: &str) -> Option<&'static Recipe> {
    builtin().iter().find(|r| r.id == id)
}

/// The names to match against: the repo if there is one, and the folder. A Hugging Face cache
/// snapshot is named by its models--org--name folder, not the hash at its end.
fn names(path: &str, repo: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = repo.iter().map(|r| r.to_lowercase()).collect();
    let root = plan::mount_root(path);
    let folder = root.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    out.push(folder.strip_prefix("models--").unwrap_or(folder).replace("--", "/").to_lowercase());
    out
}

/// `word` sits in `name` as a whole piece: gemma-3-27b-it fits gemma-3-27b-it-abliterated, but
/// qwen3-1.7b doesn't fit qwen3-1.75b.
fn fits_name(word: &str, name: &str) -> bool {
    let edge = |c: Option<char>| c.is_none_or(|c| !c.is_ascii_alphanumeric());
    name.match_indices(word).any(|(i, _)| edge(name[..i].chars().next_back()) && edge(name[i + word.len()..].chars().next()))
}

/// How well a recipe fits: the length of its longest word that does, None when none does.
fn specificity(r: &Recipe, names: &[String]) -> Option<usize> {
    r.matches.iter().filter(|m| names.iter().any(|n| fits_name(m, n))).map(|m| m.len()).max()
}

/// The recipes of `all` that fit, most specific first.
fn ranked<'a>(all: &'a [Recipe], names: &[String]) -> Vec<&'a Recipe> {
    let mut found: Vec<(usize, &'a Recipe)> = all.iter().filter_map(|r| specificity(r, names).map(|s| (s, r))).collect();
    found.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    found.into_iter().map(|(_, r)| r).collect()
}

/// Every recipe that fits a folder, most specific first, ending with the generic one. A folder
/// held by a placeholder gets only that: nothing else may load it.
pub fn recipes_for(path: &str, repo: Option<&str>) -> Vec<&'static Recipe> {
    let mut out = ranked(builtin(), &names(path, repo));
    if out.first().is_some_and(|r| r.placeholder()) {
        out.truncate(1);
    } else {
        out.push(generic());
    }
    out
}

/// The recipe picked for a folder: most specific first, generic for the rest.
pub fn recipe_for(path: &str, repo: Option<&str>) -> &'static Recipe {
    recipes_for(path, repo)[0]
}

/// The picked recipe as the Load page shows it, with what else fits.
pub fn view_for(path: &str, repo: Option<&str>) -> RecipeView {
    let all = recipes_for(path, repo);
    all[0].view(all.iter().map(|r| RecipeChoice { id: r.id.clone(), title: r.title.clone() }).collect())
}

/// Every built-in recipe, for Settings.
pub fn recipes() -> Vec<RecipeView> {
    builtin().iter().map(|r| r.view(Vec::new())).collect()
}

/// Fills in what a load takes from its recipe (C-SP): at deploy, never from the UI. `spec.recipe`
/// is the one picked on the Load page; empty, the folder decides, and it stays empty for the
/// generic one. Refuses a placeholder with its note, an unknown recipe, and a split it doesn't list.
/// SGLang loads have no recipes.
pub fn apply(spec: &mut DeploySpec) -> Result<(), String> {
    if !plan::is_native(spec) {
        return Ok(());
    }
    let for_folder = recipe_for(&spec.model_path, None);
    let picked = if let Some(imported) = &spec.imported_recipe {
        validate(imported)?;
        if imported.backend != "native" { return Err("An imported recipe must use the native backend.".into()); }
        imported
    } else if spec.recipe.is_empty() { for_folder } else { by_id(&spec.recipe).ok_or_else(|| format!("There's no recipe called {}.", spec.recipe))? };
    if let Some(held) = [for_folder, picked].into_iter().find(|r| r.placeholder()) {
        return Err(held.note.clone().unwrap_or_default());
    }
    if !picked.allows(spec.tensor_parallel, spec.pipeline_parallel) {
        return Err(picked.not_listed());
    }
    if (spec.recipe.is_empty() || spec.imported_recipe.is_some()) && picked.id != GENERIC {
        spec.recipe = picked.id.clone();
    }
    spec.env = picked.env.clone();
    spec.recipe_args = picked.argv();
    spec.docker_access = if spec.nodes.len() > 1 { picked.multi_node_docker.clone() } else { "standard".into() };
    if spec.image_ref.is_empty() && spec.image.is_empty() {
        spec.image_ref = picked.image.clone();
    }
    Ok(())
}

/// The checks every recipe passes, built-in or not.
pub fn validate(r: &Recipe) -> Result<(), String> {
    let id = &r.id;
    if r.title.is_empty() || !plan::is_plain_word(id) {
        return Err(format!("{id:?}: a recipe needs a title and a plain id"));
    }
    if !["native", PLACEHOLDER].contains(&r.backend.as_str()) {
        return Err(format!("{id}: backend {:?} isn't one SparkBox has", r.backend));
    }
    if r.placeholder() && r.note.as_deref().unwrap_or("").is_empty() {
        return Err(format!("{id}: a placeholder refuses with its note, so it needs one"));
    }
    if r.placeholder() != r.topologies.is_empty() {
        return Err(format!("{id}: only a placeholder has no topologies"));
    }
    if let Some(bad) = r.matches.iter().find(|m| m.is_empty() || m.to_lowercase() != **m) {
        return Err(format!("{id}: match {bad:?} must be lower case"));
    }
    if id != GENERIC && r.matches.is_empty() {
        return Err(format!("{id}: only the generic recipe matches nothing"));
    }
    if !plan::is_image_ref(&r.image) {
        return Err(format!("{id}: bad image {:?}", r.image));
    }
    if let Some(bad) = r.topologies.iter().find(|t| !known_topology(t)) {
        return Err(format!("{id}: topology {bad:?} isn't one SparkBox knows"));
    }
    // C-L2 mounts all three, and the exit admits exactly that.
    if r.mounts != ["vllm", "flashinfer", "triton"] {
        return Err(format!("{id}: mounts must be vllm, flashinfer and triton"));
    }
    if !["standard", "privileged"].contains(&r.multi_node_docker.as_str()) {
        return Err(format!("{id}: multiNodeDocker must be standard or privileged"));
    }
    for a in &r.args {
        if a.value.is_some() && a.json.is_some() {
            return Err(format!("{id}: {} has a value and JSON", a.flag));
        }
    }
    plan::check_extra_args(&r.argv()).map_err(|e| format!("{id}: {e}"))?;
    check_env(&r.env).map_err(|e| format!("{id}: {e}"))?;
    for parser in [&r.tool_parser, &r.reasoning_parser].into_iter().flatten() {
        if parser.is_empty() || !parser.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) {
            return Err(format!("{id}: bad parser {parser:?}"));
        }
    }
    if r.defaults.share.is_some_and(|s| !(fit::MIN_SHARE..=0.95).contains(&s)) {
        return Err(format!("{id}: the share must be between {} and 0.95", fit::MIN_SHARE));
    }
    Ok(())
}

/// "single", "replicas", "tp2", "pp3", "tp2pp2".
fn known_topology(t: &str) -> bool {
    let number = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if t == "single" || t == "replicas" {
        return true;
    }
    let Some(rest) = t.strip_prefix("tp").or(t.strip_prefix("pp")) else { return false };
    match rest.split_once("pp") {
        Some((tp, pp)) => t.starts_with("tp") && number(tp) && number(pp),
        None => number(rest),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn gemma_arch() -> ArchFacts {
        ArchFacts { layers: 42, heads: 32, kv_heads: 16, ..Default::default() }
    }

    fn spec(path: &str, nodes: &[&str], tp: u64, pp: u64) -> DeploySpec {
        DeploySpec {
            id: "d1".into(),
            served_name: "m".into(),
            model_path: path.into(),
            nodes: nodes.iter().map(|n| n.to_string()).collect(),
            tensor_parallel: tp,
            pipeline_parallel: pp,
            image: plan::DEFAULT_IMAGE.into(),
            ..Default::default()
        }
    }

    #[test]
    fn every_built_in_loads_and_passes_its_checks() {
        assert_eq!(builtin().len(), BUILT_INS.len());
        for r in builtin() {
            assert_eq!(validate(r), Ok(()), "{}", r.id);
        }
        let ids: Vec<&str> = builtin().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["qwen3-1.7b", "qwen3.8-27b-fp8", "gemma-3-27b-it", "glm-5.3-flash", "generic"]);
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "ids are unique");
        assert_eq!(recipes().len(), BUILT_INS.len());
        assert!(recipes().iter().all(|v| v.verified_on.is_empty()), "nothing claims verified until a load passes readiness");
    }

    #[test]
    fn the_built_ins_say_what_the_contract_says() {
        let gemma = by_id("gemma-3-27b-it").unwrap();
        assert_eq!(gemma.topologies, ["single", "tp2", "pp3", "tp4", "replicas"]);
        assert_eq!((gemma.defaults.share, gemma.defaults.context, gemma.defaults.seqs), (None, Some(32768), None));
        assert_eq!((gemma.multi_node_docker.as_str(), gemma.image.as_str()), ("privileged", plan::DEFAULT_IMAGE));
        let tiny = by_id("qwen3-1.7b").unwrap();
        assert_eq!((tiny.tool_parser.as_deref(), tiny.reasoning_parser.as_deref(), tiny.topologies.as_slice()), (Some("hermes"), Some("qwen3"), &["single".to_string(), "replicas".to_string()][..]));
        let qwen = by_id("qwen3.8-27b-fp8").unwrap();
        assert_eq!((qwen.tool_parser.as_deref(), qwen.reasoning_parser.as_deref(), qwen.topologies.as_slice()), (None, Some("qwen3"), &["single".to_string(), "replicas".to_string()][..]));
        let generic = by_id(GENERIC).unwrap();
        assert!(generic.matches.is_empty() && generic.tool_parser.is_none() && generic.reasoning_parser.is_none());
        for t in ["single", "tp2", "pp3", "tp4", "pp2", "pp4", "tp2pp2", "tp3"] {
            assert!(generic.topologies.iter().any(|x| x == t), "{t}");
        }
        assert_eq!(by_id("glm-5.3-flash").unwrap().note.as_deref(), Some("Needs the FP4-Local checkpoint and its image, see What's coming"));
    }

    fn broken(change: impl Fn(&mut Value)) -> Result<(), String> {
        let mut v: Value = serde_json::from_str(BUILT_INS[2]).unwrap();
        change(&mut v);
        validate(&serde_json::from_value::<Recipe>(v).map_err(|e| e.to_string())?)
    }

    /// A bad built-in fails a test, so these are the mistakes one can make.
    #[test]
    fn a_bad_recipe_fails_its_checks() {
        assert_eq!(broken(|_| {}), Ok(()), "the control passes");
        assert!(broken(|v| v["unknown"] = json!(1)).is_err(), "a typo in a key");
        assert!(broken(|v| v["id"] = json!("a b")).is_err());
        assert!(broken(|v| v["backend"] = json!("ray")).is_err());
        assert!(broken(|v| v["match"] = json!(["Gemma"])).unwrap_err().contains("lower case"));
        assert!(broken(|v| v["image"] = json!("x;reboot")).unwrap_err().contains("bad image"));
        assert!(broken(|v| v["topologies"] = json!(["tp"])).unwrap_err().contains("topology"));
        assert!(broken(|v| v["mounts"] = json!(["vllm"])).unwrap_err().contains("mounts"));
        assert!(broken(|v| v["multiNodeDocker"] = json!("host")).is_err());
        assert!(broken(|v| v["toolParser"] = json!("x; rm -rf /")).unwrap_err().contains("bad parser"));
        assert!(broken(|v| v["defaults"]["share"] = json!(0.99)).is_err());
        assert!(broken(|v| v["env"] = json!({ "HF_TOKEN": "x" })).unwrap_err().contains("secret"));
        assert!(broken(|v| v["args"] = json!([{ "flag": "--api-key", "value": "x" }])).unwrap_err().contains("API key"));
        assert!(broken(|v| v["args"] = json!([{ "flag": "--host", "value": "0.0.0.0" }])).unwrap_err().contains("set by SparkBox"));
        assert!(broken(|v| v["args"] = json!([{ "flag": "--block-size", "value": "256", "json": {} }])).is_err());
        assert!(broken(|v| {
            v["backend"] = json!("placeholder");
            v["topologies"] = json!([]);
            v["note"] = Value::Null;
        })
        .unwrap_err()
        .contains("needs one"));
    }

    #[test]
    fn a_recipes_args_become_one_argv_word_each() {
        let r: Recipe = serde_json::from_value(json!({
            "id": "x", "title": "X", "match": ["x"], "image": "a/b:c", "backend": "native", "topologies": ["single"], "env": {},
            "args": [ { "flag": "--block-size", "value": "256" }, { "flag": "--enforce-eager" }, { "flag": "--speculative-config", "json": { "method": "mtp", "n": 5 } } ],
            "mounts": ["vllm", "flashinfer", "triton"], "defaults": { "share": 0.75, "context": null, "seqs": null },
            "toolParser": null, "reasoningParser": null, "multiNodeDocker": "standard", "note": null
        }))
        .unwrap();
        assert_eq!(validate(&r), Ok(()));
        assert_eq!(r.argv(), ["--block-size", "256", "--enforce-eager", "--speculative-config", r#"{"method":"mtp","n":5}"#]);
    }

    #[test]
    fn folders_match_case_insensitively_most_specific_first() {
        let id = |path: &str, repo: Option<&str>| recipe_for(path, repo).id.as_str();
        assert_eq!(id("/home/sparkuser/models/Qwen/Qwen3.8-27B-FP8", None), "qwen3.8-27b-fp8");
        assert_eq!(id("/home/sparkuser/models/google/gemma-3-27b-it", None), "gemma-3-27b-it");
        assert_eq!(id("/home/sparkuser/models/GEMMA-3-27B-IT", None), "gemma-3-27b-it");
        assert_eq!(id("/home/sparkuser/.cache/huggingface/hub/models--google--gemma-3-27b-it/snapshots/005ad3404e59d6023443cb575daa05336842228a", None), "gemma-3-27b-it");
        assert_eq!(id("/home/sparkuser/.cache/huggingface/hub/models--Qwen--Qwen3-1.7B/snapshots/abc", None), "qwen3-1.7b");
        assert_eq!(id("/home/sparkuser/models/renamed", Some("Qwen/Qwen3-1.7B")), "qwen3-1.7b", "the repo counts too");
        assert_eq!(id("/home/sparkuser/models/GLM-5.3-Flash-NVFP4", None), "glm-5.3-flash");
        assert_eq!(id("/home/sparkuser/models/some-other-model", None), GENERIC);
        assert_eq!(id("/home/sparkuser/models/qwen3-1.75b", None), GENERIC, "a number that only starts the same isn't a match");
        assert_eq!(id("/home/sparkuser/models/not-gemma-3-27b-items", None), GENERIC);
        assert_eq!(id("/home/sparkuser/models/gemma-3-27b-it-abliterated", None), "gemma-3-27b-it", "a variant fits");
    }

    #[test]
    fn the_longest_match_wins_and_the_menu_lists_what_else_fits() {
        // No two built-ins overlap yet, so this is a wide recipe beside a narrow one, either way round.
        let wide = Recipe { id: "wide".into(), matches: vec!["qwen3".into()], ..by_id("qwen3-1.7b").unwrap().clone() };
        let narrow = by_id("qwen3.8-27b-fp8").unwrap().clone();
        let names = names("/m/Qwen3.8-27B-FP8", None);
        let order = |all: &[Recipe]| ranked(all, &names).iter().map(|r| r.id.clone()).collect::<Vec<_>>();
        assert_eq!(order(&[wide.clone(), narrow.clone()]), ["qwen3.8-27b-fp8", "wide"]);
        assert_eq!(order(&[narrow, wide]), ["qwen3.8-27b-fp8", "wide"]);
        let v = view_for("/home/sparkuser/models/google/gemma-3-27b-it", None);
        assert_eq!(v.fits.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["gemma-3-27b-it", GENERIC]);
        assert!(!v.placeholder && v.verified_on.is_empty());
        let g = view_for("/home/sparkuser/models/other", None);
        assert_eq!((g.id.as_str(), g.fits.len()), (GENERIC, 1), "one fit, no menu");
    }

    #[test]
    fn apply_fills_in_what_the_recipe_says_and_leaves_the_rest() {
        let mut s = spec("/home/sparkuser/models/google/gemma-3-27b-it", &["cell1", "cell2"], 2, 1);
        apply(&mut s).unwrap();
        assert_eq!((s.recipe.as_str(), s.docker_access.as_str()), ("gemma-3-27b-it", "privileged"));
        assert!(s.env.is_empty() && s.recipe_args.is_empty());
        assert_eq!(s.image, plan::DEFAULT_IMAGE, "what was picked stays");
        assert!(plan::validate(&s).is_ok(), "{:?}", plan::validate(&s));

        // One node never runs privileged, whatever the recipe says for several.
        let mut one = spec("/home/sparkuser/models/google/gemma-3-27b-it", &["cell1"], 1, 1);
        apply(&mut one).unwrap();
        assert_eq!(one.docker_access, "standard");

        // Generic stays empty; picked by hand, it stays picked, so a reload keeps the same choice.
        let mut other = spec("/home/sparkuser/models/other", &["cell1", "cell2"], 1, 2);
        apply(&mut other).unwrap();
        assert_eq!((other.recipe.as_str(), other.docker_access.as_str()), ("", "privileged"));
        let mut chosen = DeploySpec { recipe: GENERIC.into(), ..spec("/home/sparkuser/models/google/gemma-3-27b-it", &["cell1"], 1, 1) };
        apply(&mut chosen).unwrap();
        assert_eq!(chosen.recipe, GENERIC);

        // No image picked: the recipe's.
        let mut bare = DeploySpec { image: String::new(), ..spec("/m/other", &["cell1"], 1, 1) };
        apply(&mut bare).unwrap();
        assert_eq!(bare.image_ref, plan::DEFAULT_IMAGE);

        // A stored env and args from a hand-edited file don't survive: the recipe's replace them.
        let mut stale = spec("/m/other", &["cell1"], 1, 1);
        stale.env.insert("VLLM_LOGGING_LEVEL".into(), "DEBUG".into());
        stale.recipe_args = vec!["--seed".into(), "1".into()];
        apply(&mut stale).unwrap();
        assert!(stale.env.is_empty() && stale.recipe_args.is_empty());

        // SGLang has no recipes.
        let mut sg = DeploySpec { engine: "sglang".into(), recipe: "nonsense".into(), ..spec("/m/GLM-5.3-Flash", &["cell1"], 1, 1) };
        assert!(apply(&mut sg).is_ok());
    }

    #[test]
    fn the_glm_placeholder_refuses_with_its_note_however_it_is_reached() {
        let note = "Needs the FP4-Local checkpoint and its image, see What's coming";
        let path = "/home/sparkuser/models/GLM-5.3-Flash-NVFP4";
        assert_eq!(apply(&mut spec(path, &["cell1", "cell2", "cell3", "cell4"], 4, 1)).unwrap_err(), note);
        assert_eq!(apply(&mut DeploySpec { recipe: "glm-5.3-flash".into(), ..spec("/m/other", &["cell1"], 1, 1) }).unwrap_err(), note);
        assert_eq!(apply(&mut DeploySpec { recipe: GENERIC.into(), ..spec(path, &["cell1"], 1, 1) }).unwrap_err(), note, "the menu is no way round it");
        let v = view_for(path, None);
        assert!(v.placeholder);
        assert_eq!(v.note.as_deref(), Some(note));
        assert_eq!(v.fits.len(), 1, "nothing else is offered for this folder");
        // The plan offers nothing that loads, and every refusal is the note, on any node count and
        // whatever the model could take: Generic can't load this folder, and neither can a split.
        let recipe = recipe_for(path, None);
        for arch in [gemma_arch(), ArchFacts::default()] {
            for nodes in 1..=4 {
                let offers = recipe.offers(&arch, nodes);
                assert!(!offers.is_empty());
                for o in offers {
                    assert!(!o.ok, "{o:?}");
                    assert_eq!(o.reason.as_deref(), Some(note), "{o:?}");
                }
            }
        }
    }

    #[test]
    fn a_recipe_refuses_a_split_it_doesnt_list() {
        let mut s = spec("/home/sparkuser/models/Qwen/Qwen3.8-27B-FP8", &["cell1", "cell2"], 2, 1);
        assert_eq!(apply(&mut s).unwrap_err(), "The Qwen3.8 27B FP8 recipe doesn't list this split. Pick Generic vLLM to try it.");
        let mut s = spec("/home/sparkuser/models/google/gemma-3-27b-it", &["a", "b", "c", "d"], 2, 2);
        assert!(apply(&mut s).unwrap_err().contains("doesn't list this split"));
        assert!(apply(&mut DeploySpec { recipe: "nothing-like-it".into(), ..s.clone() }).unwrap_err().contains("no recipe called"));
        assert!(apply(&mut DeploySpec { recipe: GENERIC.into(), ..s }).is_ok(), "generic takes every split");
    }

    #[test]
    fn gemmas_offers_follow_its_recipe_and_its_config() {
        let gemma = by_id("gemma-3-27b-it").unwrap();
        let row = |nodes: usize| -> Vec<(String, bool)> { gemma.offers(&gemma_arch(), nodes).iter().map(|o| (o.id.clone(), o.ok)).collect() };
        assert_eq!(row(1), [("single".to_string(), true)]);
        assert_eq!(row(2), [("tp2".to_string(), true), ("pp2".to_string(), false), ("replicas2".to_string(), true)]);
        assert_eq!(row(3), [("tp3".to_string(), false), ("pp3".to_string(), true), ("replicas3".to_string(), true)]);
        assert!(gemma.offers(&gemma_arch(), 3)[0].reason.as_deref().unwrap().contains("32 heads don't divide by 3"), "its own sentence stays");
        assert_eq!(row(4)[0], ("tp4".to_string(), true));
        // The Split menu words each from its cause: the model can take what's only unlisted.
        let unlisted = |nodes: usize| -> Vec<String> { gemma.offers(&gemma_arch(), nodes).iter().filter(|o| o.unlisted).map(|o| o.id.clone()).collect() };
        assert_eq!(unlisted(2), ["pp2"]);
        assert!(unlisted(3).is_empty(), "TP=3 is the model's refusal, not the recipe's");
        assert_eq!(unlisted(4), ["pp4", "tp2pp2"]);
    }

    #[test]
    fn a_recipes_environment_is_an_allowlist_with_no_secrets() {
        let one = |k: &str| check_env(&BTreeMap::from([(k.to_string(), "1".to_string())]));
        for ok in ["CUTE_DSL_ARCH", "NCCL_BUFFSIZE", "VLLM_USE_V2_MODEL_RUNNER", "NCCL_MAX_CTAS", "B12X_POLICY_MODE"] {
            assert_eq!(one(ok), Ok(()), "{ok}");
        }
        for bad in ["VLLM_API_KEY", "HF_TOKEN", "OPENAI_API_KEY", "VLLM_SOME_SECRET", "NCCL_PASSWORD", "VLLM_FOO_TOKEN"] {
            assert!(one(bad).unwrap_err().contains("secret"), "{bad}");
        }
        for own in plan::RANK_ENV {
            assert!(one(own).unwrap_err().contains("set by SparkBox"), "{own}");
        }
        for other in ["PATH", "LD_PRELOAD", "PYTHONPATH", "HOME", "vllm_lower", "NCCL_a;b"] {
            assert!(one(other).is_err(), "{other}");
        }
        assert!(check_env(&BTreeMap::from([("NCCL_DEBUG_SUBSYS".to_string(), "a\nb".to_string())])).unwrap_err().contains("line break"));
        for code in ["NCCL_NET_PLUGIN", "NCCL_TUNER_PLUGIN", "NCCL_PROFILER_PLUGIN", "VLLM_PLUGINS", "VLLM_LOGGING_CONFIG_PATH", "NCCL_DEBUG_FILE"] {
            assert!(one(code).unwrap_err().contains("loads code"), "{code}");
        }
    }

    /// Each of these once said 0.75, the first-load share, with nothing measured behind it. The
    /// Load page sizes the share to the model unless a recipe says otherwise.
    #[test]
    fn no_built_in_holds_a_share_of_its_own() {
        for r in builtin() {
            assert_eq!(r.defaults.share, None, "{}", r.id);
        }
    }
}
