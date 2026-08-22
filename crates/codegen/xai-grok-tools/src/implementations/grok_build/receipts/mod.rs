//! Execution receipts — audit artifacts for mutating tool calls (Phase 5 v1).
//!
//! Each successful [`crate::implementations::grok_build::search_replace`]
//! (edit) or foreground `bash` completion records a receipt under
//! `<session>/receipts/<receipt_id>.json`. Edit receipts additionally keep an
//! undo payload (`<id>.before`) so the `rollback` tool can revert exactly that
//! edit after verifying the file has not changed since.
//!
//! V1 honesty notes:
//! - Bash receipts are audit-only (no undo payload).
//! - Receipts larger than [`MAX_UNDO_BYTES`] are recorded but marked
//!   `undoable=false`.
//! - Receipt metadata passes through the secrets sanitizer before hitting disk.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use xai_grok_secrets::redact_json_string_values;

use crate::types::requirements::{Expr, ToolRequirement};

/// Name of the receipt listing/inspection tool.
pub const RECEIPTS_TOOL_NAME: &str = "receipts";
/// Name of the single-receipt rollback tool.
pub const ROLLBACK_TOOL_NAME: &str = "rollback";

/// Do not keep undo payloads for files above this size (bytes).
pub(crate) const MAX_UNDO_BYTES: u64 = 5 * 1024 * 1024;

/// One recorded mutating tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReceipt {
    pub receipt_id: String,
    /// RFC 3339 timestamp of the recorded call.
    pub ts_rfc3339: String,
    /// Builtin tool that produced the mutation (`search_replace`, `bash`,
    /// or `rollback` for entries created by undoing another receipt).
    pub tool: String,
    /// `edit`, `bash`, or `rollback`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Whether an undo payload exists and rollback is possible.
    pub undoable: bool,
    /// For `rollback` receipts: the receipt id that was undone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolled_back: Option<String>,
}

/// Errors surfaced by the receipts store.
#[derive(Debug)]
pub enum ReceiptError {
    /// Session folder unavailable — receipts are disabled in this context.
    NoSession,
    Io(std::io::Error),
}

impl std::fmt::Display for ReceiptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSession => f.write_str("no session folder; receipts unavailable"),
            Self::Io(e) => write!(f, "receipt io error: {e}"),
        }
    }
}

fn receipts_dir(session_folder: &std::path::Path) -> PathBuf {
    session_folder.join("receipts")
}

/// Generate the next receipt id (`rcpt-<uuidv7>`; lexicographic order is
/// time order).
pub(crate) fn new_receipt_id() -> String {
    format!("rcpt-{}", Uuid::now_v7())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Hash helper exposed for hook sites: SHA-256 hex of bytes.
pub fn hash_bytes(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

/// Write with a short retry loop: Windows AV / indexer / editor processes
/// transiently hold files open (ERROR_SHARING_VIOLATION), which would
/// otherwise silently drop audit records.
async fn write_with_retry(path: &std::path::Path, bytes: &[u8]) -> Result<(), ReceiptError> {
    let mut attempt = 0;
    loop {
        match tokio::fs::write(path, bytes).await {
            Ok(()) => return Ok(()),
            Err(e) if attempt < 3 && is_transient_windows_lock(&e) => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(25 * attempt as u64)).await;
            }
            Err(e) => return Err(ReceiptError::Io(e)),
        }
    }
}

fn is_transient_windows_lock(e: &std::io::Error) -> bool {
    // 32 = ERROR_SHARING_VIOLATION, 33 = ERROR_LOCK_VIOLATION.
    e.raw_os_error().is_some_and(|code| code == 32 || code == 33)
}

/// Record a receipt (and optional raw undo payload) under the session folder.
///
/// Returns the receipt id. Metadata is secret-scrubbed; failures are returned
/// so callers can decide whether to degrade gracefully.
pub async fn record_receipt(
    session_folder: &std::path::Path,
    mut receipt: ToolReceipt,
    undo_payload: Option<&[u8]>,
) -> Result<String, ReceiptError> {
    // Oversized payloads are rejected up front so `undoable` never lies about
    // a `.before` file that will not exist.
    let stored_payload = match undo_payload {
        Some(payload) if (payload.len() as u64) > MAX_UNDO_BYTES => {
            receipt.undoable = false;
            None
        }
        other => other,
    };
    let dir = receipts_dir(session_folder);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(ReceiptError::Io)?;
    let id = receipt.receipt_id.clone();
    let mut value = serde_json::to_value(&receipt)
        .map_err(|e| ReceiptError::Io(std::io::Error::other(e.to_string())))?;
    redact_json_string_values(&mut value);
    let json = serde_json::to_vec_pretty(&value)
        .map_err(|e| ReceiptError::Io(std::io::Error::other(e.to_string())))?;
    write_with_retry(&dir.join(format!("{id}.json")), &json).await?;
    if let Some(payload) = stored_payload {
        write_with_retry(&dir.join(format!("{id}.before")), payload).await?;
    }
    Ok(id)
}

async fn load_receipt(
    session_folder: &std::path::Path,
    receipt_id: &str,
) -> Result<Option<ToolReceipt>, ReceiptError> {
    if !receipt_id.starts_with("rcpt-") || receipt_id.contains(['/', '\\', ':']) {
        return Ok(None);
    }
    let path = receipts_dir(session_folder).join(format!("{receipt_id}.json"));
    match tokio::fs::read(&path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| ReceiptError::Io(std::io::Error::other(e.to_string()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ReceiptError::Io(e)),
    }
}

async fn load_undo_payload(
    session_folder: &std::path::Path,
    receipt_id: &str,
) -> Result<Option<Vec<u8>>, ReceiptError> {
    let path = receipts_dir(session_folder).join(format!("{receipt_id}.before"));
    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ReceiptError::Io(e)),
    }
}

/// List recent receipts, newest first, capped at `limit`.
pub async fn list_receipts(
    session_folder: &std::path::Path,
    limit: usize,
) -> Result<Vec<ToolReceipt>, ReceiptError> {
    let dir = receipts_dir(session_folder);
    let mut ids: Vec<String> = Vec::new();
    let mut rd = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ReceiptError::Io(e)),
    };
    while let Some(entry) = rd
        .next_entry()
        .await
        .map_err(ReceiptError::Io)?
    {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = name.strip_suffix(".json") {
            ids.push(id.to_owned());
        }
    }
    ids.sort_unstable();
    ids.reverse();
    ids.truncate(limit);
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(receipt) = load_receipt(session_folder, &id).await? {
            out.push(receipt);
        }
    }
    Ok(out)
}

/// Outcome of a rollback attempt.
#[derive(Debug)]
pub enum RollbackOutcome {
    Restored {
        receipt: Box<ToolReceipt>,
        new_receipt_id: String,
        restored_bytes: u64,
    },
    NotUndoable(String),
    ChangedSinceReceipt(String),
    NotFound(String),
}

/// Undo one edit receipt: restore the exact prior bytes, refusing when the
/// current file no longer matches the receipt's `hash_after` (someone edited
/// it in between). Records a follow-up `rollback` receipt.
pub async fn rollback_receipt(
    session_folder: &std::path::Path,
    fs: &dyn crate::computer::types::AsyncFileSystem,
    receipt_id: &str,
) -> Result<RollbackOutcome, ReceiptError> {
    let Some(receipt) = load_receipt(session_folder, receipt_id).await? else {
        return Ok(RollbackOutcome::NotFound(receipt_id.to_owned()));
    };
    if receipt.kind != "edit" || !receipt.undoable {
        return Ok(RollbackOutcome::NotUndoable(format!(
            "{receipt_id} is a {} receipt; only undoable edit receipts can roll back",
            receipt.kind
        )));
    }
    let (Some(file), Some(after)) = (&receipt.file, &receipt.hash_after) else {
        return Ok(RollbackOutcome::NotUndoable(format!(
            "{receipt_id} lacks file/hash metadata"
        )));
    };
    let path = PathBuf::from(file);
    let current = match fs.read_file(&path).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok(RollbackOutcome::ChangedSinceReceipt(format!(
                "{file} is gone; cannot roll back safely"
            )))
        }
    };
    if &hash_bytes(&current) != after {
        return Ok(RollbackOutcome::ChangedSinceReceipt(format!(
            "{file} changed since {receipt_id}; refusing to overwrite"
        )));
    }
    // NOTE (accepted v1 TOCTOU): between this hash check and the restore
    // below, a concurrent writer can still land an edit that the restore
    // would clobber. The AsyncFileSystem abstraction has no compare-and-swap;
    // closing this needs a file lock or CAS primitive on the fs backend.
    // Mitigation for now: the refusal check above catches every *completed*
    // prior write; only writes racing inside this window are at risk.
    let Some(prior) = load_undo_payload(session_folder, receipt_id).await? else {
        return Ok(RollbackOutcome::NotUndoable(format!(
            "{receipt_id} has no undo payload"
        )));
    };
    let restored_len = prior.len() as u64;
    fs.write_file(&path, &prior).await.map_err(|e| {
        ReceiptError::Io(std::io::Error::other(format!(
            "restore failed for {file}: {e}"
        )))
    })?;
    let new_id = new_receipt_id();
    let entry = ToolReceipt {
        receipt_id: new_id.clone(),
        ts_rfc3339: chrono::Utc::now().to_rfc3339(),
        tool: "rollback".into(),
        kind: "rollback".into(),
        file: Some(file.clone()),
        hash_before: Some(hash_bytes(&current)),
        hash_after: Some(hash_bytes(&prior)),
        command: None,
        exit_code: None,
        undoable: false,
        rolled_back: Some(receipt_id.to_owned()),
    };
    record_receipt(session_folder, entry, None).await?;
    Ok(RollbackOutcome::Restored {
        receipt: Box::new(receipt),
        new_receipt_id: new_id,
        restored_bytes: restored_len,
    })
}

/// Conservative detector for bash commands whose effects receipts should
/// advertise inline. Misses stay recorded in the store regardless.
pub fn looks_mutating(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    const PATTERNS: &[&str] = &[
        "git commit",
        "git checkout ",
        "git switch ",
        "git reset",
        "git restore",
        "git clean",
        "git stash",
        "git push",
        "git rebase",
        "git merge",
        "git cherry-pick",
        "git revert",
        "git rm",
        "git mv",
        "rm ",
        "rmdir ",
        "del ",
        "erase ",
        "move ",
        "copy ",
        "rename ",
        "remove-item",
        "move-item",
        "copy-item",
        "new-item",
        "set-content",
        "add-content",
        "out-file",
        "mkdir ",
        "md ",
        "touch ",
        "truncate ",
        "tee ",
        "cargo publish",
        "npm publish",
        "pip install",
        "npm install",
        "pnpm install",
        "yarn add",
        "dotnet publish",
        "docker rm",
        "docker rmi",
        "docker stop",
        "kubectl delete",
        "gh pr create",
        "gh pr merge",
        "gh release create",
        "turbo disk clean",
        "turbo subagent discard",
    ];
    PATTERNS.iter().any(|p| lower.contains(p))
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// `receipts` — inspect recorded execution receipts.
#[derive(Debug, Default)]
pub struct ReceiptsTool;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct ReceiptsInput {
    #[schemars(
        description = "Receipt id to show (rcpt-...). Omit to list recent receipts."
    )]
    pub receipt_id: Option<String>,
    #[schemars(description = "Max entries when listing (default 20, max 100).")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptsOutput {
    pub mode: String,
    pub text: String,
}

impl xai_tool_runtime::ToolOutput for ReceiptsOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.text.clone(),
        }]
    }
}

impl crate::types::tool_metadata::ToolMetadata for ReceiptsTool {
    fn kind(&self) -> crate::types::tool::ToolKind {
        crate::types::tool::ToolKind::Read
    }

    fn tool_namespace(&self) -> crate::types::tool::ToolNamespace {
        crate::types::tool::ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Inspect execution receipts recorded by mutating tool calls (edits and mutating shell commands). Call with no arguments to list recent receipt ids, or pass a `receipt_id` to see one receipt's full metadata (tool, paths, hashes, exit code). Pair with `rollback` to undo a specific edit."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for ReceiptsTool {
    type Args = ReceiptsInput;
    type Output = ReceiptsOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(RECEIPTS_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            RECEIPTS_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: ReceiptsInput,
    ) -> Result<ReceiptsOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;
        let resources = shared_resources(&ctx)?;
        let session = {
            let res = resources.lock().await;
            res.get::<crate::types::resources::SessionFolder>()
                .map(|s| s.0.clone())
        };
        let Some(session) = session else {
            return Ok(ReceiptsOutput {
                mode: "unavailable".into(),
                text: "No session folder in this context; receipts are unavailable.".into(),
            });
        };
        if let Some(id) = input.receipt_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let receipt = load_receipt(&session, id)
                .await
                .map_err(|e| xai_tool_runtime::ToolError::custom("receipt_io", e.to_string()))?;
            let text = match receipt {
                Some(r) => serde_json::to_string_pretty(&r)
                    .unwrap_or_else(|_| format!("{r:?}")),
                None => format!("Receipt {id} not found."),
            };
            return Ok(ReceiptsOutput {
                mode: if text.starts_with("Receipt ") { "not_found" } else { "show" }.into(),
                text,
            });
        }
        let limit = input.limit.unwrap_or(20).clamp(1, 100);
        let receipts = list_receipts(&session, limit)
            .await
            .map_err(|e| xai_tool_runtime::ToolError::custom("receipt_io", e.to_string()))?;
        if receipts.is_empty() {
            return Ok(ReceiptsOutput {
                mode: "empty".into(),
                text: "No receipts recorded in this session yet.".into(),
            });
        }
        let mut text = format!("{} recent receipt(s), newest first:\n", receipts.len());
        for r in &receipts {
            let target = r
                .file
                .as_deref()
                .or(r.command.as_deref())
                .unwrap_or_default();
            text.push_str(&format!(
                "- {} [{}] {} {}{}\n",
                r.receipt_id,
                r.kind,
                r.tool,
                target,
                if r.undoable { " (undoable)" } else { "" }
            ));
        }
        Ok(ReceiptsOutput { mode: "list".into(), text })
    }
}

/// `rollback` — restore the pre-edit contents captured by an edit receipt.
#[derive(Debug, Default)]
pub struct RollbackTool;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct RollbackInput {
    #[schemars(description = "The receipt id (rcpt-...) to undo.")]
    pub receipt_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackOutput {
    pub status: String,
    pub message: String,
}

impl xai_tool_runtime::ToolOutput for RollbackOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.message.clone(),
        }]
    }
}

impl crate::types::tool_metadata::ToolMetadata for RollbackTool {
    fn kind(&self) -> crate::types::tool::ToolKind {
        crate::types::tool::ToolKind::Edit
    }

    fn tool_namespace(&self) -> crate::types::tool::ToolNamespace {
        crate::types::tool::ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Undo one recorded edit by restoring the exact file contents captured before that edit. Refuses closed when the file changed since the receipt (verify with `git diff` first) or when the receipt has no undo payload (bash receipts and oversized files are audit-only)."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

impl xai_tool_runtime::Tool for RollbackTool {
    type Args = RollbackInput;
    type Output = RollbackOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(ROLLBACK_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            ROLLBACK_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities::default()
    }

    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: RollbackInput,
    ) -> Result<RollbackOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;
        let resources = shared_resources(&ctx)?;
        let (session, fs) = {
            let res = resources.lock().await;
            (
                res.get::<crate::types::resources::SessionFolder>()
                    .map(|s| s.0.clone()),
                res.require::<crate::types::resources::FileSystem>()
                    .ok()
                    .map(|f| f.0.clone()),
            )
        };
        let (Some(session), Some(fs)) = (session, fs) else {
            return Ok(RollbackOutput {
                status: "unavailable".into(),
                message: "Receipts require a session folder and filesystem context.".into(),
            });
        };
        let id = input.receipt_id.trim().to_owned();
        match rollback_receipt(&session, fs.as_ref(), &id)
            .await
            .map_err(|e| xai_tool_runtime::ToolError::custom("receipt_io", e.to_string()))?
        {
            RollbackOutcome::Restored {
                receipt,
                new_receipt_id,
                restored_bytes,
            } => Ok(RollbackOutput {
                status: "restored".into(),
                message: format!(
                    "Rolled back {}: restored {} bytes of {}. Follow-up receipt: {new_receipt_id}.",
                    receipt.receipt_id, restored_bytes,
                    receipt.file.as_deref().unwrap_or_default()
                ),
            }),
            RollbackOutcome::NotUndoable(msg) | RollbackOutcome::ChangedSinceReceipt(msg) => {
                Ok(RollbackOutput { status: "refused".into(), message: msg })
            }
            RollbackOutcome::NotFound(id) => Ok(RollbackOutput {
                status: "not_found".into(),
                message: format!("Receipt {id} not found."),
            }),
        }
    }
}

/// Convenience wrapper used by tool hooks: build + persist a receipt,
/// degrading to a warn-log on storage failure.
pub(crate) async fn try_record(
    session: Option<&std::path::Path>,
    tool: &str,
    kind: &str,
    file: Option<String>,
    hash_before: Option<String>,
    hash_after: Option<String>,
    command: Option<String>,
    exit_code: Option<i32>,
    undo_payload: Option<Vec<u8>>,
) -> Option<String> {
    let session = session?;
    let undoable = matches!(kind, "edit") && undo_payload.is_some();
    let receipt = ToolReceipt {
        receipt_id: new_receipt_id(),
        ts_rfc3339: chrono::Utc::now().to_rfc3339(),
        tool: tool.into(),
        kind: kind.into(),
        file,
        hash_before,
        hash_after,
        command,
        exit_code,
        undoable,
        rolled_back: None,
    };
    let id = receipt.receipt_id.clone();
    match record_receipt(session, receipt, undo_payload.as_deref()).await {
        Ok(recorded) => Some(recorded),
        Err(e) => {
            tracing::warn!(receipt_id = %id, error = %e, "failed to persist tool receipt");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::local::LocalFs;
    use std::sync::Arc;

    fn temp_session() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn sample(kind: &str) -> ToolReceipt {
        ToolReceipt {
            receipt_id: new_receipt_id(),
            ts_rfc3339: chrono::Utc::now().to_rfc3339(),
            tool: if kind == "edit" { "search_replace" } else { "bash" }.into(),
            kind: kind.into(),
            file: (kind == "edit").then(|| "src/lib.rs".to_owned()),
            hash_before: (kind == "edit").then(|| "aa".repeat(32)),
            hash_after: (kind == "edit").then(|| "bb".repeat(32)),
            command: (kind == "bash")
                .then(|| "git commit -m 'tok ghp_fakefakefakefakefakefakefake'".to_owned()),
            exit_code: (kind == "bash").then_some(0),
            undoable: kind == "edit",
            rolled_back: None,
        }
    }

    #[tokio::test]
    async fn round_trips_receipt_and_scrubs_embedded_tokens() {
        let dir = temp_session();
        let receipt = sample("bash");
        let id =
            record_receipt(dir.path(), receipt.clone(), None).await.expect("record");
        assert_eq!(id, receipt.receipt_id);
        let loaded = load_receipt(dir.path(), &id).await.unwrap().expect("exists");
        assert_eq!(loaded.command.as_deref(), Some("git commit -m 'tok [REDACTED_SECRET]'"));
    }

    #[tokio::test]
    async fn lists_newest_first_with_limit() {
        let dir = temp_session();
        for _ in 0..3 {
            record_receipt(dir.path(), sample("edit"), Some(b"old".as_slice()))
                .await
                .expect("record");
        }
        let listed = list_receipts(dir.path(), 2).await.unwrap();
        assert_eq!(listed.len(), 2);
        // uuid v7: later ids sort after earlier ones; newest-first ordering
        // puts the last-recorded id first.
        assert!(listed[0].receipt_id > listed[1].receipt_id);
    }

    #[tokio::test]
    async fn rejects_path_traversal_ids() {
        let dir = temp_session();
        assert!(load_receipt(dir.path(), "../escape").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rollback_restores_prior_bytes_and_records_followup() {
        let dir = temp_session();
        let fs: Arc<dyn crate::computer::types::AsyncFileSystem> = Arc::new(LocalFs);
        let file = dir.path().join("code.txt");
        tokio::fs::write(&file, b"before").await.unwrap();

        let before = tokio::fs::read(&file).await.unwrap();
        tokio::fs::write(&file, b"after-content").await.unwrap();
        let after = tokio::fs::read(&file).await.unwrap();
        let receipt = ToolReceipt {
            receipt_id: new_receipt_id(),
            ts_rfc3339: chrono::Utc::now().to_rfc3339(),
            tool: "search_replace".into(),
            kind: "edit".into(),
            file: Some(file.to_string_lossy().to_string()),
            hash_before: Some(hash_bytes(&before)),
            hash_after: Some(hash_bytes(&after)),
            command: None,
            exit_code: None,
            undoable: true,
            rolled_back: None,
        };
        let id = record_receipt(dir.path(), receipt, Some(&before)).await.unwrap();

        match rollback_receipt(dir.path(), fs.as_ref(), &id).await.unwrap() {
            RollbackOutcome::Restored { new_receipt_id, .. } => {
                let now = tokio::fs::read(&file).await.unwrap();
                assert_eq!(now, b"before");
                let followup =
                    load_receipt(dir.path(), &new_receipt_id).await.unwrap().unwrap();
                assert_eq!(followup.kind, "rollback");
                assert_eq!(followup.rolled_back.as_deref(), Some(id.as_str()));
            }
            other => panic!("expected restored, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rollback_refuses_when_file_changed_since_receipt() {
        let dir = temp_session();
        let fs: Arc<dyn crate::computer::types::AsyncFileSystem> = Arc::new(LocalFs);
        let file = dir.path().join("drift.txt");
        tokio::fs::write(&file, b"v1").await.unwrap();
        let receipt = ToolReceipt {
            receipt_id: new_receipt_id(),
            ts_rfc3339: chrono::Utc::now().to_rfc3339(),
            tool: "search_replace".into(),
            kind: "edit".into(),
            file: Some(file.to_string_lossy().to_string()),
            hash_before: Some(hash_bytes(b"v0")),
            hash_after: Some(hash_bytes(b"v2")),
            command: None,
            exit_code: None,
            undoable: true,
            rolled_back: None,
        };
        let id = record_receipt(dir.path(), receipt, Some(b"v0")).await.unwrap();
        tokio::fs::write(&file, b"drifted").await.unwrap();
        match rollback_receipt(dir.path(), fs.as_ref(), &id).await.unwrap() {
            RollbackOutcome::ChangedSinceReceipt(_) => {}
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn oversized_undo_payload_marks_not_undoable() {
        let dir = temp_session();
        let big = vec![b'x'; (MAX_UNDO_BYTES + 1) as usize];
        let mut receipt = sample("edit");
        receipt.undoable = true;
        let id = record_receipt(dir.path(), receipt, Some(&big)).await.unwrap();
        let loaded = load_receipt(dir.path(), &id).await.unwrap().unwrap();
        assert!(!loaded.undoable, "oversized payloads must downgrade undoability");
    }

    #[test]
    fn mutating_detector_covers_common_cases_without_false_positives() {
        assert!(looks_mutating("git commit -m x"));
        assert!(looks_mutating("Remove-Item foo.txt"));
        assert!(looks_mutating("rm -rf build"));
        assert!(!looks_mutating("cargo test -p xai-grok-tools"));
        assert!(!looks_mutating("git status --short"));
        assert!(!looks_mutating("echo hello"));
    }
}
