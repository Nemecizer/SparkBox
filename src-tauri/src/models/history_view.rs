//! Inert projections of local evidence. No DeploySpec, provider, ownership or resume path.
use std::{collections::HashSet, fs::{self, OpenOptions}, io::Read, os::unix::fs::OpenOptionsExt, path::Path};
use serde::{Deserialize, Serialize};
use super::UnloadReport;

pub const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOTAL: usize = 16 * 1024 * 1024;
const MAX_FILES: usize = 128;
const MAX_RECORDS: usize = 256;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub model: Option<String>,
    pub nodes: Vec<String>,
    pub saved_state: String,
    pub started_at: Option<u64>,
    pub generation: Option<u64>,
    pub message: Option<String>,
    pub failure: Option<String>,
    pub unload_report: Option<UnloadReport>,
    pub report_only: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub name: String,
    pub kind: String,
    pub sha256: String,
    pub bytes: usize,
    pub records: Vec<Record>,
    pub issues: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub demo: bool,
    pub folder: Option<String>,
    pub sources: Vec<Source>,
    pub issues: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    id: Option<String>,
    spec: Option<Spec>,
    state: String,
    demo: Option<bool>,
    started_at: Option<u64>,
    generation: Option<u64>,
    message: Option<String>,
    failure: Option<Failure>,
    unload_report: Option<UnloadReport>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Spec { id: String, served_name: String, nodes: Vec<String> }
#[derive(Deserialize)]
struct Failure { cause: String }

/// `bytes` come from an explicitly selected browser File, never a caller-supplied path.
/// Parsing does not save, repair, migrate, quarantine or import anything.
pub fn preview(name: String, bytes: &[u8]) -> Result<Source, String> {
    if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) || name.contains(['/', '\\']) {
        return Err("Choose a JSON report file with a plain filename.".into());
    }
    if bytes.len() > MAX_BYTES { return Err("This file exceeds the 4 MiB viewing limit. The file was not changed.".into()); }
    Ok(parse(name, "file", bytes))
}

fn parse(name: String, kind: &str, bytes: &[u8]) -> Source {
    let mut source = Source { name, kind: kind.into(), sha256: crate::live::sha256_hex(bytes), bytes: bytes.len(), records: vec![], issues: vec![] };
    let values: Vec<serde_json::Value> = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => { source.issues.push(format!("Cannot read this JSON report: {e}. The file was not changed.")); return source; }
    };
    if values.len() > MAX_RECORDS {
        source.issues.push(format!("This file has {} records; the viewing limit is {MAX_RECORDS}. No records were displayed or changed.", values.len()));
        return source;
    }
    let mut ids = HashSet::new();
    for (i, value) in values.into_iter().enumerate() {
        let parsed = (|| -> Result<Record, String> {
            let saved: Saved = serde_json::from_value(value).map_err(|e| e.to_string())?;
            if saved.demo == Some(true) { return Err("Demo record in a live evidence file".into()); }
            let report_only = saved.spec.is_none();
            let (id, model, nodes) = match saved.spec {
                Some(spec) => (spec.id, Some(spec.served_name), spec.nodes),
                None => (saved.id.ok_or("Missing load identity")?, None, saved.unload_report.as_ref().ok_or("Older report is missing its unload checks")?.nodes.iter().map(|n| n.node.clone()).collect()),
            };
            if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) || saved.state.is_empty() || saved.state.len() > 64 {
                return Err("Missing or invalid identity or saved state".into());
            }
            // Keep duplicate rows visible with a warning, never quietly choose a winner.
            if !ids.insert(id.clone()) { source.issues.push(format!("Repeated identity {id}: every saved version is shown. Nothing was merged.")); }
            Ok(Record { id, model, nodes, saved_state: saved.state, started_at: saved.started_at, generation: saved.generation,
                message: saved.message, failure: saved.failure.map(|f| f.cause), unload_report: saved.unload_report, report_only })
        })();
        match parsed {
            Ok(record) => source.records.push(record),
            Err(error) => source.issues.push(format!("Record {} could not be displayed: {error}. The file was not changed.", i + 1)),
        }
    }
    source
}

/// Reads only the fixed recovery directory. No locks, saves or authoritative-list reads.
/// Open without following links; nonblocking protects against a replaced FIFO/device.
pub fn read(dir: &Path) -> View {
    let mut view = View { demo: false, folder: Some(dir.display().to_string()), sources: vec![], issues: vec![] };
    let result = (|| -> Result<(), String> {
        match fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("Cannot read the recovery folder: {e}")),
            Ok(m) if !m.is_dir() || m.file_type().is_symlink() => return Err("The recovery folder is not a regular directory; no files were read.".into()),
            _ => {}
        }
        let mut paths = Vec::new();
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())?.take(MAX_FILES + 1) {
            paths.push(entry.map_err(|e| e.to_string())?.path());
        }
        if paths.len() > MAX_FILES { return Err(format!("The recovery folder exceeds the {MAX_FILES}-entry viewing limit. No files were read or changed. Open an individual JSON file to inspect it.")); }
        paths.sort();
        let mut total = 0;
        for path in paths {
            if path.extension().is_none_or(|e| e != "json") { continue; }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let digest = name.strip_suffix(".json").unwrap();
            let mut source = Source { name: name.clone(), kind: "recovery".into(), sha256: String::new(), bytes: 0, records: vec![], issues: vec![] };
            let read = (|| -> Result<Vec<u8>, String> {
                if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                    return Err("Unrecognized recovery filename; open this file separately to inspect it.".into());
                }
                let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(&path).map_err(|e| format!("Cannot read recovery copy: {e}"))?;
                let meta = file.metadata().map_err(|e| e.to_string())?;
                if !meta.is_file() { return Err("Recovery copy is not a regular file; it was not read.".into()); }
                if meta.len() > MAX_BYTES as u64 { return Err("Recovery copy exceeds the 4 MiB viewing limit.".into()); }
                if total + meta.len() as usize > MAX_TOTAL { return Err("Recovery copies exceed the 16 MiB total viewing limit; open this file separately.".into()); }
                let mut bytes = Vec::new();
                file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                if bytes.len() > MAX_BYTES || total + bytes.len() > MAX_TOTAL { return Err("Recovery copy grew beyond the viewing limit.".into()); }
                total += bytes.len();
                Ok(bytes)
            })();
            match read {
                Ok(bytes) => {
                    source.bytes = bytes.len();
                    source.sha256 = crate::live::sha256_hex(&bytes);
                    if source.sha256 != digest {
                        source.issues.push("The contents do not match the recovery filename's SHA-256. No records were displayed; the file was not changed. Open it separately to inspect the changed contents.".into());
                    } else { source = parse(name, "recovery", &bytes); }
                }
                Err(e) => source.issues.push(e),
            }
            view.sources.push(source);
        }
        Ok(())
    })();
    if let Err(e) = result { view.issues.push(e); }
    view
}

pub fn demo() -> View {
    let mut reports = parse("older-reports-demo.json".into(), "demo", include_bytes!("../../../src/test/fixtures/deployment-history-reports.json"));
    // Fixtures share the parser and preserve the two warning outcomes.
    reports.kind = "demo".into();
    let recovery = parse("recovery-copy-demo.json".into(), "demo", include_bytes!("../../../src/test/fixtures/deployment-history-recovery.json"));
    View { demo: true, folder: None, sources: vec![recovery, reports], issues: vec![] }
}

#[cfg(test)]
mod tests;
