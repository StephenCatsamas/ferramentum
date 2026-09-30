use super::*;
use serde_json::json;
use std::fs;
use std::io::Write;
use tempfile::{NamedTempFile, tempdir};

fn header(source: Value) -> String {
    format!(
        "{}\n",
        json!({"type":"session_meta", "payload":{
            "id":"thread", "source":source, "cwd":"/workspace"
        }})
    )
}

fn event(kind: &str, turn: &str) -> String {
    format!(
        "{}\n",
        json!({"timestamp":"2026-09-30T12:00:00Z", "type":"event_msg",
        "payload":{"type":kind, "turn_id":turn}})
    )
}

fn append(file: &mut NamedTempFile, value: &str, transcript: &mut Transcript) {
    file.write_all(value.as_bytes()).unwrap();
    transcript.refresh(file.path()).unwrap();
}

#[test]
fn root_and_subagent_logs_are_distinguished() {
    for (source, is_main) in [
        (json!("cli"), true),
        (json!({"subagent":{"thread_spawn":{}}}), false),
    ] {
        let mut file = NamedTempFile::new().unwrap();
        let mut transcript = Transcript::default();
        append(&mut file, &header(source), &mut transcript);
        assert_eq!(transcript.is_main(), is_main);
        assert_eq!(transcript.view().state, TurnState::Unknown);
    }
}

#[test]
fn lifecycle_rejects_old_completion_and_retains_the_previous_finish_time() {
    let mut file = NamedTempFile::new().unwrap();
    let mut transcript = Transcript::default();
    append(&mut file, &header(json!("cli")), &mut transcript);
    append(&mut file, &event("task_started", "first"), &mut transcript);
    assert_eq!(transcript.view().state, TurnState::Working);
    append(&mut file, &event("task_complete", "first"), &mut transcript);
    assert_eq!(transcript.view().state, TurnState::Ready);
    let finished = transcript.view().last_finished_at;
    assert!(finished.is_some());
    append(&mut file, &event("turn_started", "next"), &mut transcript);
    append(&mut file, &event("task_complete", "first"), &mut transcript);
    assert_eq!(transcript.view().state, TurnState::Working);
    assert_eq!(transcript.view().last_finished_at, finished);
    append(&mut file, &event("turn_aborted", "next"), &mut transcript);
    assert_eq!(transcript.view().state, TurnState::Interrupted);
    append(
        &mut file,
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_complete\",\"error\":{\"message\":\"private\"},\"completed_at\":10}}\n",
        &mut transcript,
    );
    assert_eq!(transcript.view().state, TurnState::Error);
    assert_eq!(transcript.view().last_finished_at, Some(10));
    assert!(transcript.view().detail.is_none());
}

#[test]
fn only_synchronous_input_requests_block_and_matching_responses_resume() {
    let mut file = NamedTempFile::new().unwrap();
    let mut transcript = Transcript::default();
    append(&mut file, &header(json!("cli")), &mut transcript);
    append(&mut file, &event("task_started", "one"), &mut transcript);
    let call = |name, id| {
        format!(
            "{}\n",
            json!({"type":"response_item", "payload":{
                "type":"function_call", "name":name, "call_id":id, "arguments":"private"
            }})
        )
    };
    let output = |id| {
        format!(
            "{}\n",
            json!({"type":"response_item", "payload":{
                "type":"function_call_output", "call_id":id, "output":"private"
            }})
        )
    };
    append(
        &mut file,
        &call("functions.request_user_input_async", "async"),
        &mut transcript,
    );
    assert_eq!(transcript.view().state, TurnState::Working);
    append(
        &mut file,
        &call("functions.request_user_input", "sync"),
        &mut transcript,
    );
    assert_eq!(transcript.view().state, TurnState::NeedsInput);
    append(&mut file, &output("async"), &mut transcript);
    assert_eq!(transcript.view().state, TurnState::NeedsInput);
    append(&mut file, &output("sync"), &mut transcript);
    assert_eq!(transcript.view().state, TurnState::Working);
}

#[test]
fn partial_appends_are_retried_and_invalid_records_do_not_claim_completion() {
    let mut file = NamedTempFile::new().unwrap();
    let mut transcript = Transcript::default();
    file.write_all(b"{\"type\":").unwrap();
    assert!(transcript.refresh(file.path()).is_err());
    assert!(transcript.refresh(file.path()).is_err());
    file.as_file().set_len(0).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    append(&mut file, &header(json!("cli")), &mut transcript);
    append(&mut file, &event("task_started", "one"), &mut transcript);
    let complete = event("task_complete", "one");
    append(&mut file, &complete[..20], &mut transcript);
    assert_eq!(transcript.view().state, TurnState::Working);
    append(&mut file, &complete[20..], &mut transcript);
    assert_eq!(transcript.view().state, TurnState::Ready);
    append(&mut file, "not json\n", &mut transcript);
    assert_eq!(transcript.view().state, TurnState::Unknown);
    append(&mut file, &event("task_started", "two"), &mut transcript);
    assert_eq!(transcript.view().state, TurnState::Working);
}

#[test]
fn replacement_and_truncation_reset_cached_state_and_symlinks_are_rejected() {
    let root = tempdir().unwrap();
    let path = root.path().join("rollout-test.jsonl");
    let replacement = root.path().join("replacement");
    let mut transcript = Transcript::default();
    fs::write(&path, header(json!("cli")) + &event("task_started", "one")).unwrap();
    transcript.refresh(&path).unwrap();
    fs::write(
        &replacement,
        header(json!("cli")) + &event("task_complete", "two"),
    )
    .unwrap();
    fs::rename(&replacement, &path).unwrap();
    transcript.refresh(&path).unwrap();
    assert_eq!(transcript.view().state, TurnState::Ready);
    fs::write(&path, header(json!("cli"))).unwrap();
    transcript.refresh(&path).unwrap();
    assert_eq!(transcript.view().state, TurnState::Unknown);
    assert_eq!(transcript.view().last_finished_at, None);
    std::os::unix::fs::symlink(&path, &replacement).unwrap();
    assert!(transcript.refresh(&replacement).is_err());
}

#[test]
fn oversized_records_and_long_history_are_bounded_and_recover_at_lifecycle_events() {
    let mut file = NamedTempFile::new().unwrap();
    let mut transcript = Transcript::default();
    append(&mut file, &header(json!("cli")), &mut transcript);
    append(&mut file, &event("task_started", "one"), &mut transcript);
    append(
        &mut file,
        &"x".repeat(MAX_LINE_BYTES as usize + 100),
        &mut transcript,
    );
    assert_eq!(transcript.view().state, TurnState::Unknown);
    append(
        &mut file,
        &("\n".to_owned() + &event("task_complete", "one")),
        &mut transcript,
    );
    assert_eq!(transcript.view().state, TurnState::Ready);
    let huge = "x".repeat(MAX_SCAN_BYTES as usize + 100) + "\n" + &event("turn_aborted", "two");
    append(&mut file, &huge, &mut transcript);
    assert_eq!(transcript.view().state, TurnState::Interrupted);
}

#[test]
fn run_time_uses_turn_timestamps_and_freezes_after_completion() {
    let mut file = NamedTempFile::new().unwrap();
    let mut transcript = Transcript::default();
    append(&mut file, &header(json!("cli")), &mut transcript);
    let timed = |kind, turn, start, end, duration| {
        format!(
            "{}\n",
            json!({
                "type":"event_msg", "timestamp":"2026-09-30T12:00:00Z", "payload":{
                    "type":kind, "turn_id":turn, "started_at":start, "completed_at":end, "duration_ms":duration
                }
            })
        )
    };
    append(
        &mut file,
        &timed("task_started", "one", Some(100), None::<i64>, None::<i64>),
        &mut transcript,
    );
    assert_eq!(transcript.view().run_time_ms(112), Some(12000));
    append(
        &mut file,
        &timed("task_complete", "one", Some(100), Some(113), Some(12500)),
        &mut transcript,
    );
    assert_eq!(transcript.view().run_time_ms(999), Some(12500));
    append(
        &mut file,
        &timed("task_started", "two", Some(200), None, None),
        &mut transcript,
    );
    assert_eq!(transcript.view().run_time_ms(210), Some(10000));
    // Older builds omit duration_ms; interruption falls back to recorded timestamps.
    append(
        &mut file,
        &timed("turn_aborted", "two", None, Some(220), None),
        &mut transcript,
    );
    assert_eq!(transcript.view().run_time_ms(999), Some(20000));
    append(&mut file, "invalid\n", &mut transcript);
    assert_eq!(transcript.view().run_time_ms(999), None);
}

#[test]
fn large_compaction_and_tool_payloads_preserve_state_and_lifecycle_records_still_apply() {
    let mut file = NamedTempFile::new().unwrap();
    let mut transcript = Transcript::default();
    append(&mut file, &header(json!("cli")), &mut transcript);
    append(&mut file, &event("task_started", "one"), &mut transcript);
    let started = transcript.view.started_at;
    let large = "x".repeat(MAX_LINE_BYTES as usize + 200);
    for record in [
        json!({"type":"compacted", "payload":{"message":large}}),
        json!({"type":"response_item", "payload":{"type":"function_call_output", "output":large}}),
        json!({"type":"event_msg", "payload":{"type":"item_completed", "item":{"text":large}}}),
    ] {
        append(&mut file, &(record.to_string() + "\n"), &mut transcript);
        assert_eq!(transcript.view.state, TurnState::Working);
        assert_eq!(transcript.view.started_at, started);
        assert!(transcript.view.detail.is_none());
    }
    // Payload precedes type; record classification must not depend on key order.
    let ending = format!(
        "{{\"payload\":{{\"output\":\"{large}\",\"turn_id\":\"one\",\"type\":\"task_complete\"}},\"type\":\"event_msg\"}}\n"
    );
    append(&mut file, &ending, &mut transcript);
    assert_eq!(transcript.view.state, TurnState::Ready);
    append(&mut file, &event("task_started", "two"), &mut transcript);
    assert_eq!(transcript.view.state, TurnState::Working);
}

#[test]
fn partial_large_records_are_retried_and_malformed_ones_cannot_preserve_a_false_state() {
    let mut file = NamedTempFile::new().unwrap();
    let mut transcript = Transcript::default();
    append(&mut file, &header(json!("cli")), &mut transcript);
    append(&mut file, &event("task_started", "one"), &mut transcript);
    let start = transcript.offset;
    let prefix = format!(
        "{{\"type\":\"compacted\",\"payload\":{{\"message\":\"{}",
        "x".repeat(MAX_LINE_BYTES as usize + 20)
    );
    append(&mut file, &prefix, &mut transcript);
    assert_eq!(transcript.offset, start);
    assert_eq!(transcript.view.state, TurnState::Working);
    append(&mut file, "\"}}\n", &mut transcript);
    assert!(transcript.offset > start);
    assert_eq!(transcript.view.state, TurnState::Working);
    append(&mut file, &(prefix + "\"} invalid}\n"), &mut transcript);
    assert_eq!(transcript.view.state, TurnState::Unknown);
    append(&mut file, &event("task_complete", "one"), &mut transcript);
    assert_eq!(transcript.view.state, TurnState::Ready);
}

#[test]
fn subagents_read_their_own_turns_and_ignore_inherited_activity_and_input_calls() {
    let mut file = NamedTempFile::new().unwrap();
    let mut transcript = Transcript::default();
    let meta = json!({"type":"session_meta","payload":{
        "id":"child", "session_id":"root", "subagent_history_start_ordinal":3,
        "source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}
    }})
    .to_string()
        + "\n";
    append(&mut file, &meta, &mut transcript);
    assert!(transcript.is_subagent());
    assert!(!transcript.is_main());
    assert_eq!(transcript.view.parent_id.as_deref(), Some("parent"));
    assert_eq!(transcript.view.root_id.as_deref(), Some("root"));
    append(
        &mut file,
        &(event("task_started", "inherited") + &event("task_complete", "inherited")),
        &mut transcript,
    );
    assert_eq!(transcript.view.state, TurnState::Unknown);
    append(&mut file, &event("task_started", "own"), &mut transcript);
    assert_eq!(transcript.view.state, TurnState::Working);
    append(
        &mut file,
        "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"request_user_input\",\"call_id\":\"denied\"}}\n",
        &mut transcript,
    );
    assert_eq!(transcript.view.state, TurnState::Working);
    append(&mut file, &event("turn_aborted", "own"), &mut transcript);
    assert_eq!(transcript.view.state, TurnState::Interrupted);
}
