use std::time::{SystemTime};

use regex::Regex;

use crate::{folder, test_framework::{run, TestDesc}};
use crate::test_framework::*;
use map_macro::map;

/// Dest has three files which need deleting. The expected behaviour is controlled by a command-line argument, 
/// which in this case we set to "prompt", and choose "skip" and then "delete", then "skip" again.
#[test]
fn prompt_skip_then_delete() {
    let src = empty_folder();
    let dest = folder! {
        "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents4", SystemTime::UNIX_EPOCH),
        "c3" => file_with_modified("contents7", SystemTime::UNIX_EPOCH),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-entry-needs-deleting".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Skip (just this occurence)"),
            String::from("Delete (just this occurence)"),
            String::from("Skip (just this occurence)"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new("dest file .*c1.* needs deleting").unwrap()),
            (1, Regex::new("dest file .*c2.* needs deleting").unwrap()),
            // Note that we need this last check, to make sure that the second prompt response only affects one file, not all remaining files
            (1, Regex::new("dest file .*c3.* needs deleting").unwrap()), 
            (1, Regex::new(&regex::escape("Deleted 1 file(s)")).unwrap()), // 1 file deleted the other two skipped
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&folder! { // First and last file skipped, the other deleted
                "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
                "c3" => file_with_modified("contents7", SystemTime::UNIX_EPOCH),
            })),
        ],
        ..Default::default()
    });
}

/// Dest has two files which need deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "skip all" on the first prompt, so both files should be skipped.
#[test]
fn prompt_skip_all() {
    let src = empty_folder();
    let dest = folder! {
        "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents4", SystemTime::UNIX_EPOCH),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-entry-needs-deleting".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Skip (all occurences)"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new("dest file .*c\\d.* needs deleting").unwrap()), // We can't be sure which file we'll be prompted about, because the order is non-deterministic, we just need to make sure there's only one
            (1, Regex::new(&regex::escape("Nothing to do")).unwrap()), // Both files skipped
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Dest has two files which need deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "delete all" on the first prompt, so both files should be deleted.
#[test]
fn prompt_delete_all() {
    let src = empty_folder();
    let dest = folder! {
        "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents4", SystemTime::UNIX_EPOCH),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-entry-needs-deleting".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Delete (all occurences)"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new("dest file .*c\\d.* needs deleting").unwrap()), // We can't be sure which file we'll be prompted about, because the order is non-deterministic, we just need to make sure there's only one
            (1, Regex::new(&regex::escape("Deleted 2 file(s)")).unwrap()), // Both files deleted
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&src)), // All files deleted, so same as src
        ],
        ..Default::default()
    });
}

/// Dest has a file which need deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose to cancel the prompt, so the sync should stop.
#[test]
fn prompt_cancel() {
    let src = empty_folder();
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-entry-needs-deleting".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Cancel sync"),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            // We actaully get this message twice - once for the prompt and once in the error message after the prompt is cancelled
            (2, Regex::new("dest file .*c1.* needs deleting").unwrap()),
            (1, Regex::new(&regex::escape("Will not delete")).unwrap()), // Cancelled
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Dest has a file which needs deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to produce an error.
#[test]
fn error() {
    let src = empty_folder();
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-entry-needs-deleting".to_string(),
            "error".to_string(),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("Will not delete")).unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Dest has a file which needs deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to skip.
#[test]
fn skip() {
    let src = empty_folder();
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-entry-needs-deleting".to_string(),
            "skip".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("Nothing to do")).unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Dest has a file which needs deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to delete.
#[test]
fn delete() {
    let src = empty_folder();
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-entry-needs-deleting".to_string(),
            "delete".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("Deleted 1 file(s)")).unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&src)), // Same as source
        ],
        ..Default::default()
    });
}
