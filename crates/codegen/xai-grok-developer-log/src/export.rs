//! Export packs for the Turbo Development Team.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::redact::sanitize_incident;
use crate::schema::{Incident, Severity};
use crate::store::{DeveloperLogStore, IndexEntry, ListFilter, StoreError};

/// Options for building an export pack.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub filter: ListFilter,
    /// Directory to write the pack into (created). When `None`, uses
    /// `bundles/export-<timestamp>/` under the store root.
    pub out_dir: Option<PathBuf>,
}

/// Result of an export.
#[derive(Debug, Clone)]
pub struct ExportResult {
    pub out_dir: PathBuf,
    pub incident_count: usize,
    pub summary_path: PathBuf,
    pub ndjson_path: PathBuf,
    pub manifest_path: PathBuf,
}

/// Write a maintainer-ready pack: summary.md, incidents.ndjson, manifest.json,
/// fingerprints.csv, and per-incident JSON copies under evidence/.
pub fn export_pack(
    store: &DeveloperLogStore,
    options: ExportOptions,
) -> Result<ExportResult, StoreError> {
    let entries = store.list(&options.filter)?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let out_dir = options
        .out_dir
        .unwrap_or_else(|| store.bundles_dir().join(format!("export-{stamp}")));
    fs::create_dir_all(&out_dir)?;
    let evidence_dir = out_dir.join("evidence");
    fs::create_dir_all(&evidence_dir)?;

    let mut incidents: Vec<Incident> = Vec::with_capacity(entries.len());
    let mut skipped: Vec<String> = Vec::new();
    for entry in &entries {
        match store.get(&entry.incident_id) {
            Ok(inc) => {
                let clean = sanitize_incident(inc);
                let path = evidence_dir.join(format!("{}.json", clean.incident_id));
                let pretty = serde_json::to_string_pretty(&clean)?;
                fs::write(&path, pretty)?;
                incidents.push(clean);
            }
            Err(e) => {
                tracing::warn!(
                    incident_id = %entry.incident_id,
                    error = %e,
                    "export skipped unreadable incident"
                );
                skipped.push(entry.incident_id.clone());
            }
        }
    }

    // Only list successfully loaded incidents in tables/csv (avoid false counts).
    let loaded_ids: std::collections::HashSet<&str> =
        incidents.iter().map(|i| i.incident_id.as_str()).collect();
    let loaded_entries: Vec<_> = entries
        .iter()
        .filter(|e| loaded_ids.contains(e.incident_id.as_str()))
        .cloned()
        .collect();

    let ndjson_path = out_dir.join("incidents.ndjson");
    {
        let mut f = fs::File::create(&ndjson_path)?;
        for inc in &incidents {
            let line = serde_json::to_string(inc)?;
            writeln!(f, "{line}")?;
        }
    }

    let csv_path = out_dir.join("fingerprints.csv");
    write_fingerprints_csv(&csv_path, &loaded_entries)?;

    let summary_path = out_dir.join("summary.md");
    fs::write(
        &summary_path,
        render_summary_md(&incidents, &loaded_entries, &skipped),
    )?;

    let manifest = serde_json::json!({
        "schema_version": crate::schema::SCHEMA_VERSION,
        "exported_at": Utc::now().to_rfc3339(),
        "product_version": xai_grok_version::installed(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "incident_count": incidents.len(),
        "store_root": store.root().display().to_string(),
        "redaction": "secrets_and_user_paths",
        "files": [
            "summary.md",
            "incidents.ndjson",
            "fingerprints.csv",
            "manifest.json",
            "evidence/"
        ]
    });
    let manifest_path = out_dir.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    Ok(ExportResult {
        out_dir,
        incident_count: incidents.len(),
        summary_path,
        ndjson_path,
        manifest_path,
    })
}

fn write_fingerprints_csv(path: &Path, entries: &[IndexEntry]) -> Result<(), StoreError> {
    let mut f = fs::File::create(path)?;
    writeln!(
        f,
        "fingerprint,incident_id,severity,status,error_class,occurrence_count,title,last_seen"
    )?;
    for e in entries {
        writeln!(
            f,
            "{},{},{},{},{},{},\"{}\",{}",
            csv_escape(&e.fingerprint),
            csv_escape(&e.incident_id),
            e.severity.as_str(),
            e.status.as_str(),
            csv_escape(&e.error_class),
            e.occurrence_count,
            csv_escape(&e.title),
            csv_escape(&e.last_seen),
        )?;
    }
    Ok(())
}

fn csv_escape(s: &str) -> String {
    s.replace('"', "\"\"")
}

fn render_summary_md(incidents: &[Incident], entries: &[IndexEntry], skipped: &[String]) -> String {
    let mut p0 = 0u32;
    let mut p1 = 0u32;
    let mut p2 = 0u32;
    let mut p3 = 0u32;
    for e in entries {
        match e.severity {
            Severity::P0 => p0 += 1,
            Severity::P1 => p1 += 1,
            Severity::P2 => p2 += 1,
            Severity::P3 => p3 += 1,
        }
    }

    let mut out = String::new();
    out.push_str("# Turbo Auto Developer Log — Export Summary\n\n");
    out.push_str(&format!("- **Exported at:** {}\n", Utc::now().to_rfc3339()));
    out.push_str(&format!(
        "- **Turbo version:** {}\n",
        xai_grok_version::installed()
    ));
    out.push_str(&format!("- **Incident count:** {}\n", incidents.len()));
    out.push_str(&format!(
        "- **By severity:** P0={p0} · P1={p1} · P2={p2} · P3={p3}\n"
    ));
    if !skipped.is_empty() {
        out.push_str(&format!(
            "- **Unreadable (skipped):** {} — {}\n",
            skipped.len(),
            skipped.join(", ")
        ));
    }
    out.push('\n');
    out.push_str("## Top incidents\n\n");
    out.push_str("| Sev | Count | Class | Title | Id |\n");
    out.push_str("|-----|------:|-------|-------|----|\n");
    for e in entries.iter().take(50) {
        out.push_str(&format!(
            "| {} | {} | `{}` | {} | `{}` |\n",
            e.severity.as_str().to_ascii_uppercase(),
            e.occurrence_count,
            e.error_class,
            escape_md_cell(&e.title),
            e.incident_id,
        ));
    }
    out.push_str("\n## Detail sketches\n\n");
    for inc in incidents.iter().take(20) {
        out.push_str(&format!(
            "### {} — {}\n\n",
            inc.severity.as_str().to_ascii_uppercase(),
            inc.title
        ));
        out.push_str(&format!("- **Id:** `{}`\n", inc.incident_id));
        out.push_str(&format!("- **Fingerprint:** `{}`\n", inc.fingerprint));
        out.push_str(&format!("- **Class:** `{}`\n", inc.error_class));
        out.push_str(&format!("- **Occurrences:** {}\n", inc.occurrence_count));
        out.push_str(&format!("- **Summary:** {}\n", inc.summary));
        if let Some(ref fix) = inc.suggested_fix {
            out.push_str(&format!("- **Suggested fix:** {fix}\n"));
        }
        out.push('\n');
    }
    out.push_str(
        "\n---\n*Generated by Turbo Auto Developer Log. Fields are redacted for secrets and user paths.*\n",
    );
    out
}

fn escape_md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ErrorClass, ReportRequest};

    #[test]
    fn export_writes_expected_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeveloperLogStore::new(dir.path().join("adl"));
        store
            .report(ReportRequest {
                title: "Export me".into(),
                summary: "for maintainers".into(),
                error_class: ErrorClass::FeatureGap,
                component: vec!["export".into()],
                ..Default::default()
            })
            .unwrap();
        let out = dir.path().join("pack");
        let result = export_pack(
            &store,
            ExportOptions {
                out_dir: Some(out.clone()),
                filter: ListFilter {
                    include_closed: true,
                    ..Default::default()
                },
            },
        )
        .unwrap();
        assert_eq!(result.incident_count, 1);
        assert!(result.summary_path.is_file());
        assert!(result.ndjson_path.is_file());
        assert!(result.manifest_path.is_file());
        assert!(out.join("fingerprints.csv").is_file());
        assert!(out.join("evidence").is_dir());
    }
}
