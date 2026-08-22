//! Export packs for Feature Request Log.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;

use super::schema::FeatureRequest;
use super::store::{FeatureRequestStore, FrIndexEntry, FrListFilter, FrStoreError};

/// Options for building a feature-request export pack.
#[derive(Debug, Clone, Default)]
pub struct FrExportOptions {
    pub filter: FrListFilter,
    pub out_dir: Option<PathBuf>,
}

/// Result of an export.
#[derive(Debug, Clone)]
pub struct FrExportResult {
    pub out_dir: PathBuf,
    pub request_count: usize,
    pub summary_path: PathBuf,
    pub ndjson_path: PathBuf,
    pub manifest_path: PathBuf,
}

/// Write a maintainer-ready pack: summary.md, requests.ndjson, manifest.json,
/// fingerprints.csv, and per-request JSON under evidence/.
pub fn export_feature_requests(
    store: &FeatureRequestStore,
    options: FrExportOptions,
) -> Result<FrExportResult, FrStoreError> {
    let entries = store.list(&options.filter)?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let out_dir = options
        .out_dir
        .unwrap_or_else(|| store.bundles_dir().join(format!("export-{stamp}")));
    fs::create_dir_all(&out_dir)?;
    let evidence_dir = out_dir.join("evidence");
    fs::create_dir_all(&evidence_dir)?;

    let mut requests: Vec<FeatureRequest> = Vec::with_capacity(entries.len());
    let mut skipped: Vec<String> = Vec::new();
    for entry in &entries {
        match store.get(&entry.request_id) {
            Ok(fr) => {
                let clean = super::store::sanitize_request_doc(fr);
                let mut value = serde_json::to_value(&clean)?;
                xai_grok_secrets::redact_json_string_values(&mut value);
                let path = evidence_dir.join(format!("{}.json", clean.request_id));
                let pretty = serde_json::to_string_pretty(&value)?;
                fs::write(&path, pretty)?;
                requests.push(clean);
            }
            Err(e) => {
                tracing::warn!(
                    request_id = %entry.request_id,
                    error = %e,
                    "export skipped unreadable feature request"
                );
                skipped.push(entry.request_id.clone());
            }
        }
    }

    let loaded_ids: std::collections::HashSet<&str> =
        requests.iter().map(|r| r.request_id.as_str()).collect();
    let loaded_entries: Vec<_> = entries
        .iter()
        .filter(|e| loaded_ids.contains(e.request_id.as_str()))
        .cloned()
        .collect();

    let ndjson_path = out_dir.join("requests.ndjson");
    {
        let mut body = String::new();
        for fr in &requests {
            body.push_str(&serde_json::to_string(fr)?);
            body.push('\n');
        }
        fs::write(&ndjson_path, body)?;
    }

    let csv_path = out_dir.join("fingerprints.csv");
    {
        let mut csv = String::from(
            "request_id,fingerprint,priority,status,request_class,occurrence_count,title\n",
        );
        for e in &loaded_entries {
            csv.push_str(&format!(
                "{},{},{},{},{},{},\"{}\"\n",
                e.request_id,
                e.fingerprint,
                e.priority.as_str(),
                e.status.as_str(),
                e.request_class,
                e.occurrence_count,
                e.title.replace('"', "'")
            ));
        }
        fs::write(&csv_path, csv)?;
    }

    let summary_path = out_dir.join("summary.md");
    fs::write(
        &summary_path,
        render_summary(&loaded_entries, &requests, &skipped, store.root()),
    )?;

    let manifest_path = out_dir.join("manifest.json");
    let manifest = serde_json::json!({
        "kind": "feature_request_log_export",
        "schema_version": 1,
        "exported_at": Utc::now().to_rfc3339(),
        "store_root": store.root().display().to_string(),
        "request_count": requests.len(),
        "skipped": skipped,
        "files": [
            "summary.md",
            "requests.ndjson",
            "fingerprints.csv",
            "manifest.json",
            "evidence/"
        ]
    });
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    Ok(FrExportResult {
        out_dir,
        request_count: requests.len(),
        summary_path,
        ndjson_path,
        manifest_path,
    })
}

fn render_summary(
    entries: &[FrIndexEntry],
    requests: &[FeatureRequest],
    skipped: &[String],
    root: &Path,
) -> String {
    let mut md = String::new();
    md.push_str("# Feature Request Log — Export\n\n");
    md.push_str(&format!("**Store:** `{}`\n\n", root.display()));
    md.push_str(&format!("**Count:** {}\n\n", requests.len()));
    if !skipped.is_empty() {
        md.push_str(&format!("**Skipped:** {}\n\n", skipped.join(", ")));
    }
    md.push_str("| Pri | Status | Count | Class | Title |\n");
    md.push_str("|-----|--------|------:|-------|-------|\n");
    for e in entries {
        md.push_str(&format!(
            "| {} | {} | {} | `{}` | {} |\n",
            e.priority.as_str(),
            e.status.as_str(),
            e.occurrence_count,
            e.request_class,
            e.title.replace('|', "/")
        ));
    }
    md.push('\n');
    for fr in requests {
        md.push_str(&format!("## {} — {}\n\n", fr.request_id, fr.title));
        md.push_str(&format!(
            "- **class:** `{}` · **priority:** {} · **status:** {} · **occurrences:** {}\n",
            fr.request_class.as_str(),
            fr.priority.as_str(),
            fr.status.as_str(),
            fr.occurrence_count
        ));
        md.push_str(&format!("- **summary:** {}\n", fr.summary));
        if let Some(ref u) = fr.use_case {
            md.push_str(&format!("- **use_case:** {u}\n"));
        }
        if let Some(ref w) = fr.current_workaround {
            md.push_str(&format!("- **workaround:** {w}\n"));
        }
        if let Some(ref p) = fr.proposed_behavior {
            md.push_str(&format!("- **proposed:** {p}\n"));
        }
        md.push('\n');
    }
    md
}
