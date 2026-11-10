//! A narrow earlyoom edit: retain its existing avoid expression and all other options.
//! Complex shell configuration is refused for manual review, never evaluated.
use serde::Serialize;
use tauri::{AppHandle, State};
use crate::{admin, models_ctx, AppState};
const PATH: &str = "/etc/default/earlyoom";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview { pub before: String, pub after: String, pub sha256: String }

pub fn configure(before: &str, names: &[String]) -> Result<String, String> {
    if names.is_empty() || names.len() > 8 || names.iter().any(|n| !n.starts_with("VLLM::") || !(9..=15).contains(&n.len()) || !n.bytes().all(|c| c.is_ascii_alphanumeric() || b":_".contains(&c))) {
        return Err("Use one to eight exact VLLM:: process comm names recorded on these nodes (9–15 ASCII characters). Broad python3 patterns are refused.".into());
    }
    if before.len() > 65536 { return Err("The earlyoom configuration is too large to edit safely.".into()); }
    let lines: Vec<_> = before.lines().collect();
    let indices: Vec<_> = lines.iter().enumerate().filter(|(_, l)| l.trim_start().starts_with("EARLYOOM_ARGS=")).map(|(i, _)| i).collect();
    if indices.len() != 1 { return Err("Expected one EARLYOOM_ARGS assignment; review the file in the editor.".into()); }
    let index = indices[0];
    let value = lines[index].trim().strip_prefix("EARLYOOM_ARGS=").unwrap_or("");
    let args = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).ok_or("Expected a simple double-quoted EARLYOOM_ARGS value; review it in the editor.")?;
    if args.contains(['"', '`', '\\']) || regex::Regex::new(r"\$[A-Za-z0-9_{(]").unwrap().is_match(args) {
        return Err("Shell expansion or escaped quotes need manual review; nothing was evaluated.".into());
    }
    let mut names = names.to_vec(); names.sort(); names.dedup();
    let added = format!("^({})$", names.iter().map(|s| regex::escape(s)).collect::<Vec<_>>().join("|"));
    let avoid = regex::Regex::new(r"(?:^|\s)--avoid(?:=|\s+)'([^']*)'").unwrap();
    let matches: Vec<_> = avoid.captures_iter(args).collect();
    let updated = if args.contains("--avoid") {
        if matches.len() != 1 { return Err("The existing --avoid expression needs manual review; nothing was changed.".into()); }
        let m = &matches[0];
        let old = m.get(1).unwrap();
        if old.as_str().contains(&added) { return Ok(before.into()); }
        format!("{}({})|{}{}", &args[..old.start()], old.as_str(), added, &args[old.end()..])
    } else { format!("{args} --avoid '{added}'") };
    // Unknown quotes outside the supported avoid option must not be reinterpreted.
    if avoid.replace_all(&updated, "").contains('\'') { return Err("Quoted earlyoom options need manual review.".into()); }
    let mut result = lines.iter().enumerate().map(|(i, l)| if i == index { format!("EARLYOOM_ARGS=\"{updated}\"") } else { l.to_string() }).collect::<Vec<_>>().join("\n");
    if before.ends_with('\n') { result.push('\n'); }
    Ok(result)
}

#[tauri::command]
pub async fn earlyoom_preview(state: State<'_, AppState>, node: String, names: Vec<String>) -> Result<Preview, String> {
    let ctx = models_ctx(&state);
    if !ctx.node_ids().contains(&node) { return Err("Unknown inventory node.".into()); }
    if !ctx.live {
        let before = "# Demo earlyoom configuration\nEARLYOOM_ARGS=\"-r 3600 --avoid '^(init|sshd)$'\"\n".to_string();
        return Ok(Preview { after: configure(&before, &names)?, sha256: crate::live::sha256_hex(before.as_bytes()), before });
    }
    let file = state.files.read(!ctx.live, &node, PATH).await?;
    let after = configure(&file.content, &names)?;
    Ok(Preview { before: file.content, after, sha256: file.sha256 })
}

#[tauri::command]
pub async fn earlyoom_apply(app: AppHandle, state: State<'_, AppState>, node: String, names: Vec<String>, sha256: String) -> Result<String, String> {
    admin::require(&app)?;
    let ctx = models_ctx(&state);
    if !ctx.live { return Err("Demo previews the configuration only; it does not apply it.".into()); }
    if !ctx.node_ids().contains(&node) { return Err("Unknown inventory node.".into()); }
    apply_on(&state.files, &state.ops, &node, PATH, &names, &sha256).await
}

async fn apply_on(files: &crate::files::Files, ops: &crate::ops::Ops, node: &str, path: &str, names: &[String], sha256: &str) -> Result<String, String> {
    let file = files.read(false, node, path).await?;
    if file.sha256 != sha256 { return Err("The configuration changed after preview; read it again.".into()); }
    let after = configure(&file.content, names)?;
    if after == file.content { return Ok("The exact avoid rule is already present; nothing changed.".into()); }
    let saved = files.write(false, node, path, &after, true, Some(sha256)).await.map_err(|e| serde_json::to_string(&e).unwrap_or_default())?;
    if let Err(e) = ops.service_action(false, node, "earlyoom.service", "restart").await {
        // Restore only if our exact new contents remain; never overwrite a subsequent edit.
        let rollback = files.write(false, node, path, &file.content, true, Some(&saved.sha256)).await;
        return Err(format!("earlyoom restart failed: {}. Configuration rollback {}. Previous backup: {}", e.message, if rollback.is_ok() { "succeeded; review the service state" } else { "was refused or failed; review the file before restarting" }, saved.backup.as_deref().unwrap_or("unknown")));
    }
    Ok(format!("Saved and requested earlyoom restart. Previous configuration: {}. --avoid is a preference, not protection against every OOM kill.", saved.backup.as_deref().unwrap_or("backup unavailable")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_names_preserve_options_existing_avoid_and_comments() {
        let before = "# keep\nEARLYOOM_ARGS=\"-r 60 --avoid '^(init|sshd)$' -m 4\"\n";
        let names = vec!["VLLM::EngineCor".into(), "VLLM::Worker_TP".into()];
        let after = configure(before, &names).unwrap();
        assert!(after.contains("--avoid '(^(init|sshd)$)|^(VLLM::EngineCor|VLLM::Worker_TP)$' -m 4"));
        assert_eq!(configure(&after, &names).unwrap(), after);
        for n in ["python3", "VLLM::.*", "VLLM::EngineCoreTooLong"] { assert!(configure(before, &[n.into()]).is_err()); }
        assert!(configure("EARLYOOM_ARGS=\"$(touch /tmp/x)\"", &names).is_err());
        assert!(configure("EARLYOOM_ARGS=\"--avoid foo\"", &names).is_err());
    }

    #[tokio::test]
    async fn fake_node_apply_keeps_backup_and_failed_restart_restores_exact_old_file() {
        use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
        use crate::fake_node::{fake_node, connect, temp};
        let home = temp("earlyoom-p2-home"); std::fs::create_dir_all(&home).unwrap();
        let path = home.join("earlyoom"); let before = "EARLYOOM_ARGS=\"-r 60\"\n";
        std::fs::write(&path, before).unwrap();
        let fail = Arc::new(AtomicBool::new(false)); let f = fail.clone();
        let (node, port) = fake_node(&home, Arc::new(move |cmd, _| {
            if cmd.contains("systemctl restart -- 'earlyoom.service'") { return Some((if f.load(Ordering::SeqCst) { "!stderr:fixture restart failure" } else { "" }.into(), if f.load(Ordering::SeqCst) {1} else {0})); } None
        })).await;
        let dir = temp("earlyoom-p2-app"); let (live, inv) = connect(&dir, &[("cell1", port)]).await;
        let files = crate::files::Files::new(live.clone(), dir.join("backups"));
        let ops = crate::ops::Ops::new(live.clone(), Arc::new(Mutex::new(crate::demo::DemoCluster::new(&inv))), crate::tasks::Tasks::new(None));
        let path = path.to_str().unwrap(); let hash = crate::live::sha256_hex(before.as_bytes()); let names = ["VLLM::EngineCor".into()];
        assert!(apply_on(&files, &ops, "cell1", path, &names, "stale").await.unwrap_err().contains("changed"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), before);
        let result = apply_on(&files, &ops, "cell1", path, &names, &hash).await.unwrap();
        assert!(result.contains("Previous configuration:")); assert!(std::fs::read_to_string(path).unwrap().contains("--avoid"));
        std::fs::write(path, before).unwrap(); fail.store(true, Ordering::SeqCst);
        let error = apply_on(&files, &ops, "cell1", path, &names, &hash).await.unwrap_err();
        assert!(error.contains("rollback succeeded"), "{error}"); assert_eq!(std::fs::read_to_string(path).unwrap(), before);
        assert!(node.log.lock().unwrap().iter().any(|c| c.contains("commit")));
        live.stop();
    }
}
