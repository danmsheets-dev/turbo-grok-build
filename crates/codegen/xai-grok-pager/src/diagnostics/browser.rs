//! Agent WebView probes for `turbo doctor` / `/doctor`.

use std::io::Write;
use std::path::Path;

use super::model::{BROWSER_AGENT_PROFILE_ID, BROWSER_WEBVIEW2_RUNTIME_ID};
use super::{DiagnosticFinding, DiagnosticReport, FindingDisposition, ProbeNote, ProbeStatus};

/// Result of a window-less WebView2 runtime check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WebView2Probe {
    /// Evergreen runtime is installed.
    Present,
    /// Windows host is missing the Evergreen runtime.
    Missing,
    /// Agent WebView host is Windows-only in v1.
    WindowsOnly,
    /// Runtime query failed for another reason.
    Error(String),
}

/// Append Agent WebView findings (runtime + profile dir).
pub fn apply_browser_probe(report: &mut DiagnosticReport) {
    apply_browser_probe_with(
        report,
        probe_webview2_runtime(),
        probe_profile_writable(&xai_grok_browser::agent_browser_user_data_dir()),
    )
}

/// Map-injectable core of [`apply_browser_probe`].
pub(crate) fn apply_browser_probe_with(
    report: &mut DiagnosticReport,
    runtime: WebView2Probe,
    profile: Result<(), String>,
) {
    match runtime {
        WebView2Probe::Present => {}
        WebView2Probe::Missing => report.findings.push(DiagnosticFinding {
            id: BROWSER_WEBVIEW2_RUNTIME_ID,
            disposition: FindingDisposition::Recommendation,
            message: "WebView2 runtime is not installed; Agent WebView tools need it.".to_owned(),
            remediation: None,
            automatic_remediation: None,
            note: Some(
                "Install the Evergreen WebView2 Runtime from \
                 https://developer.microsoft.com/microsoft-edge/webview2/"
                    .to_owned(),
            ),
        }),
        WebView2Probe::WindowsOnly => report.probe_notes.push(ProbeNote {
            probe: "browser.webview2-runtime",
            status: ProbeStatus::Unsupported,
            message: Some("Agent WebView is Windows-only in v1".to_owned()),
        }),
        WebView2Probe::Error(error) => report.findings.push(DiagnosticFinding {
            id: BROWSER_WEBVIEW2_RUNTIME_ID,
            disposition: FindingDisposition::Recommendation,
            message: format!("Could not probe the WebView2 runtime: {error}"),
            remediation: None,
            automatic_remediation: None,
            note: Some(
                "Install the Evergreen WebView2 Runtime from \
                 https://developer.microsoft.com/microsoft-edge/webview2/"
                    .to_owned(),
            ),
        }),
    }

    if let Err(error) = profile {
        report.findings.push(DiagnosticFinding {
            id: BROWSER_AGENT_PROFILE_ID,
            disposition: FindingDisposition::Recommendation,
            message: error,
            remediation: None,
            automatic_remediation: None,
            note: Some(
                "Ensure $GROK_HOME/agent-browser exists and is writable \
                 (typically ~/.grok/agent-browser)."
                    .to_owned(),
            ),
        });
    }
}

fn probe_webview2_runtime() -> WebView2Probe {
    match xai_grok_browser::host::probe_webview2_runtime() {
        Ok(()) => WebView2Probe::Present,
        Err(xai_grok_browser::host::HostError::WindowsOnly) => WebView2Probe::WindowsOnly,
        Err(xai_grok_browser::host::HostError::RuntimeMissing) => WebView2Probe::Missing,
        Err(err) => WebView2Probe::Error(err.to_string()),
    }
}

/// Create `dir` if needed, write `.doctor-write-test`, then delete it.
pub(crate) fn probe_profile_writable(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create agent-browser profile {}: {e}", dir.display()))?;
    let probe = dir.join(".doctor-write-test");
    let write_err = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&probe)?;
        file.write_all(b"ok")
    })();
    match write_err {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&probe);
            Err(format!(
                "agent-browser profile {} is not writable: {e}",
                dir.display()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{
        ClipboardFacts, ColorFacts, DataControlFact, DiagnosticFacts, DiagnosticReport,
        RuntimeFact, TmuxColorPassthrough, TmuxFacts, TmuxOptionFact, TmuxSupportFact,
    };
    use crate::host::DisplayServer;
    use crate::terminal::{MultiplexerKind, TerminalName};
    use crate::theme::color_support::ColorLevel;

    fn empty_report() -> DiagnosticReport {
        DiagnosticReport {
            facts: DiagnosticFacts {
                terminal: TerminalName::Ghostty,
                xtversion: RuntimeFact::NoReply,
                multiplexer: MultiplexerKind::Undetected,
                byobu: None,
                ssh: false,
                tmux: TmuxFacts {
                    extended_keys: TmuxOptionFact::Unavailable,
                    set_clipboard: TmuxOptionFact::Unavailable,
                    allow_passthrough_support: TmuxSupportFact::Unavailable,
                    allow_passthrough: TmuxOptionFact::Unavailable,
                    color_passthrough: TmuxColorPassthrough::Unknown,
                },
                color: ColorFacts {
                    level: RuntimeFact::Available(ColorLevel::TrueColor),
                    available_themes: Vec::new(),
                    total_themes: 0,
                },
                keyboard: None,
                newline: None,
                clipboard: ClipboardFacts {
                    native_route: true,
                    native_tool: "pbcopy".to_owned(),
                    native_preflight: crate::clipboard::NativeClipboardPreflight::LocalAvailable,
                    tmux_route: false,
                    osc52_route: false,
                    osc52_capability: crate::clipboard::Osc52Capability::Supported,
                    wrap_sink: false,
                    display_server: DisplayServer::Unknown,
                    container_no_display: false,
                    data_control: DataControlFact::NotApplicable,
                    delivery: crate::clipboard::ClipboardDelivery::Confirmed,
                    fix: None,
                },
                voice: None,
            },
            findings: Vec::new(),
            probe_notes: Vec::new(),
        }
    }

    #[test]
    fn doctor_browser_runtime_missing_is_recommendation() {
        let mut report = empty_report();
        apply_browser_probe_with(&mut report, WebView2Probe::Missing, Ok(()));
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].id, BROWSER_WEBVIEW2_RUNTIME_ID);
        assert_eq!(
            report.findings[0].disposition,
            FindingDisposition::Recommendation
        );
        assert!(report.probe_notes.is_empty());
    }

    #[test]
    fn doctor_browser_non_windows_runtime_is_not_an_issue() {
        let mut report = empty_report();
        apply_browser_probe_with(&mut report, WebView2Probe::WindowsOnly, Ok(()));
        assert!(
            report
                .findings
                .iter()
                .all(|finding| finding.id != BROWSER_WEBVIEW2_RUNTIME_ID)
        );
        assert_eq!(
            report.probe_notes,
            vec![ProbeNote {
                probe: "browser.webview2-runtime",
                status: ProbeStatus::Unsupported,
                message: Some("Agent WebView is Windows-only in v1".to_owned()),
            }]
        );
    }

    #[test]
    fn doctor_browser_profile_unwritable_is_recommendation() {
        let tmp = std::env::temp_dir().join(format!(
            "turbo-doctor-browser-blocked-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let blocker = tmp.join("not-a-dir");
        std::fs::write(&blocker, b"x").unwrap();
        let profile = blocker.join("agent-browser");
        let err = probe_profile_writable(&profile).expect_err("file parent cannot be a profile");
        let mut report = empty_report();
        apply_browser_probe_with(&mut report, WebView2Probe::Present, Err(err));
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].id, BROWSER_AGENT_PROFILE_ID);
        assert_eq!(
            report.findings[0].disposition,
            FindingDisposition::Recommendation
        );
        let _ = std::fs::remove_file(&blocker);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn doctor_browser_profile_writable_leaves_no_probe_file() {
        let dir =
            std::env::temp_dir().join(format!("turbo-doctor-browser-ok-{}", std::process::id()));
        probe_profile_writable(&dir).expect("temp profile should be writable");
        assert!(!dir.join(".doctor-write-test").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
