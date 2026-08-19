//! Integration test: the shell re-parks `exit_plan_mode` on
//! resume, so approval chrome reappears after quit/`--continue` and approving
//! leaves plan mode + starts the implement turn.
//!
//! CI stages the pager binary via `PAGER_BINARY`. Also runs under plain cargo
//! (which builds the pager on demand):
//!
//! ```bash
//! cargo test -p xai-grok-pager-pty-harness --test plan_approval_resume -- --nocapture
//! ```

// IGNORED: hangs indefinitely on Windows. Once the pager child exits, the
// ConPTY `conhost.exe` enters a busy-spin (measured at 6.6 CPU-hours in one
// run) and the harness blocks forever — the scenario's 5-30s timeouts wrap only
// `wait_for_text`, not spawn or teardown, so nothing bounds it. That wedges
// `cargo test --workspace` for every other crate too.
//
// Run it explicitly once the teardown path is bounded:
//   cargo test -p xai-grok-pager-pty-harness --test plan_approval_resume -- --ignored
#[cfg_attr(windows, ignore = "ConPTY teardown spin wedges the run; see comment")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_approval_restored_after_resume() {
    xai_grok_pager_pty_harness::scenarios::plan_approval_resume::assert_plan_approval_restored_after_resume()
        .await
        .expect("shell must re-park exit_plan_mode on resume so approval chrome returns");
}
