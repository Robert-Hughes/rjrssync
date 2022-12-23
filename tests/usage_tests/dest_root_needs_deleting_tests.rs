use std::time::{SystemTime};

use regex::Regex;

use crate::{folder, test_framework::{run, TestDesc}};
use crate::test_framework::*;
use map_macro::map;

/// Dest root needs deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "cancel" on the prompt, so the sync should be stopped.
#[test]
fn prompt_cancel() {
    let src = file("this will replace the dest!");
    let dest = folder! {
        // We put some files in the destination folder, to make sure that these aren't deleted even though the 
        // root isn't (because we delete in reverse order, this would be a potential bug)
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
            "--dest-root-needs-deleting".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Cancel sync"),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            Regex::new("dest root folder .* needs deleting").unwrap(),
            Regex::new(&regex::escape("Will not delete")).unwrap(), // skipped
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Dest root needs deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "delete" on the prompt, so the sync should go ahead and the dest root deleted.
#[test]
fn prompt_delete() {
    let src = file("this will replace the dest!");
    let dest = empty_folder();
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-root-needs-deleting".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Delete"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new("dest root folder .* needs deleting").unwrap(),
            Regex::new(&regex::escape("Deleted 0 file(s), 1 folder(s)")).unwrap(), // The root folder is deleted
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&src)), // Dest root deleted and replaced by src, so same as src
        ],
        ..Default::default()
    });
}

/// Dest root needs deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "skip" on the prompt, so the sync should stop but the program still report success.
#[test]
fn prompt_skip() {
    let src = file("this will replace the dest!");
    let dest = empty_folder();
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-root-needs-deleting".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Skip"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new("dest root folder .* needs deleting").unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Dest root needs deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to produce an error.
#[test]
fn error() {
    let src = file("this will replace the dest!");
    let dest = folder! {
        // We put some files in the destination folder, to make sure that these aren't deleted even though the 
        // root isn't (because we delete in reverse order, this would be a potential bug)
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
            "--dest-root-needs-deleting".to_string(),
            "error".to_string(),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Will not delete")).unwrap(),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Dest root needs deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to skip.
#[test]
fn skip() {
    let src = file("this will replace the dest!");
    let dest = folder! {
        // We put some files in the destination folder, to make sure that these aren't deleted even though the 
        // root isn't (because we delete in reverse order, this would be a potential bug)
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
            "--dest-root-needs-deleting".to_string(),
            "skip".to_string(),
        ],
        expected_exit_code: 0,
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Dest root needs deleting. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to delete, so the sync should go ahead and the dest root deleted.
#[test]
fn delete() {
    let src = file("this will replace the dest!");
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
            "--dest-root-needs-deleting".to_string(),
            "delete".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            Regex::new(&regex::escape("Deleted 1 file(s), 1 folder(s)")).unwrap(), // The file inside and the root folder deleted
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&src)), // Dest root deleted and replaced by src, so same as src
        ],
        ..Default::default()
    });
}
