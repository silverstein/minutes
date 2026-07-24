#[path = "support/live_sidekick_engine_harness.rs"]
mod harness;

#[test]
fn deterministic_engine_eval_passes_and_repeats() {
    let report = harness::run_live_sidekick_engine_eval();
    assert!(report.passed, "{report:#?}");
    assert!(report.reproducible);
    assert_eq!(report.summary.scenarios_passed, 8);
    assert_eq!(
        report.summary.assertions_passed,
        report.summary.assertions_total
    );
    assert!(!report.coverage.release_ready_from_this_report_alone);
    assert!(
        serde_json::to_vec(&report)
            .expect("report serialization")
            .len()
            < 64 * 1024,
        "the machine-readable artifact must remain bounded"
    );
}
