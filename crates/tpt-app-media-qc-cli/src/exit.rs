//! Exit-code contract ([spec § 16]).

/// All rules passed.
pub const EXIT_OK: i32 = 0;
/// Passed with warnings / inconclusive findings.
pub const EXIT_WARN: i32 = 1;
/// Failed (findings at/above `fail_on` severity).
pub const EXIT_FAIL: i32 = 2;
/// Execution error: probe failure, I/O error, invalid input.
pub const EXIT_ERROR: i32 = 3;
/// Asset path not found.
pub const EXIT_PATH: i32 = 4;
/// Profile missing or invalid.
pub const EXIT_PROFILE: i32 = 5;
/// No usable inspector available for the requested depth.
pub const EXIT_NO_INSPECTOR: i32 = 6;

/// Map a pipeline verdict onto the exit-code contract.
pub fn exit_code_for_verdict(verdict: tpt_app_media_qc_model::severity::VerdictDecision) -> i32 {
    match verdict {
        tpt_app_media_qc_model::severity::VerdictDecision::Pass => EXIT_OK,
        tpt_app_media_qc_model::severity::VerdictDecision::Warn => EXIT_WARN,
        tpt_app_media_qc_model::severity::VerdictDecision::Fail => EXIT_FAIL,
        tpt_app_media_qc_model::severity::VerdictDecision::Inconclusive => EXIT_WARN,
    }
}
