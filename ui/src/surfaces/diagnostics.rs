//! Preserve the evidence preceding the loader's generic acknowledgement timeout.
use serde_json::Value;

pub(crate) fn failure_summary(records: &[Value]) -> String {
    let preceding = records
        .iter()
        .filter(|record| {
            record["stage"].as_str().is_some_and(|stage| {
                !matches!(
                    stage,
                    "acknowledgement_timeout" | "consumer_acknowledged" | "detach"
                )
            })
        })
        .collect::<Vec<_>>();
    let cause = preceding
        .iter()
        .rev()
        .find(|record| record["hresult"].as_u64().is_some_and(|code| code != 0))
        .or_else(|| {
            preceding.iter().rev().find(|record| {
                matches!(
                    record["stage"].as_str(),
                    Some(
                        "primary_target_ambiguous_or_missing"
                            | "secondary_target_skipped"
                            | "primary_layout_not_ready"
                            | "candidate_expired"
                    )
                )
            })
        })
        .or_else(|| preceding.last());
    let cause = cause
        .map(|record| {
            format!(
                "{} (0x{:08X})",
                record["stage"].as_str().unwrap_or("unknown"),
                record["hresult"].as_u64().unwrap_or(0)
            )
        })
        .unwrap_or_else(|| "no native progress record before timeout".into());
    let stages = records
        .iter()
        .filter_map(|r| r["stage"].as_str())
        .collect::<Vec<_>>()
        .join(" → ");
    format!("preceding evidence: {cause}; stages: {stages}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn timeout_does_not_hide_a_primary_monitor_rejection() {
        let text = failure_summary(&[
            json!({"stage":"primary_target_ambiguous_or_missing","hresult":0}),
            json!({"stage":"acknowledgement_timeout","hresult":2147942658u32}),
        ]);
        assert!(text.starts_with("preceding evidence: primary_target_ambiguous_or_missing"));
    }
    #[test]
    fn native_error_survives_detach_and_timeout() {
        let text = failure_summary(&[
            json!({"stage":"candidate_dispatch_failed","hresult":2147942405u32}),
            json!({"stage":"detach","hresult":0}),
            json!({"stage":"acknowledgement_timeout","hresult":2147942658u32}),
        ]);
        assert!(text.starts_with("preceding evidence: candidate_dispatch_failed (0x80070005)"));
    }
    #[test]
    fn timeout_alone_does_not_invent_a_root_cause() {
        assert!(
            failure_summary(&[json!({"stage":"acknowledgement_timeout","hresult":2147942658u32})])
                .starts_with("preceding evidence: no native progress record")
        );
    }
}
