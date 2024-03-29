use std::time::{SystemTime};

use regex::Regex;

use crate::{folder, test_framework::{run, TestDesc, NumActions}};
use map_macro::map;
use crate::filesystem_node::*;

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
            "--dest-root-needs-deleting=prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("1:.*:Cancel sync"),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            // We actaully get this message twice - once for the prompt and once in the error message after the prompt is cancelled
            (2, Regex::new("dest root folder .* needs deleting").unwrap()),
            (1, Regex::new(&regex::escape("Will not delete")).unwrap()), // skipped
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
            "--dest-root-needs-deleting=prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("1:.*:Delete"),
        ],
        expected_exit_code: 0,
        expected_output_messages: [&[
            (1, Regex::new("dest root folder .* needs deleting").unwrap()),
        ], &<NumActions as Into<Vec<(usize, Regex)>>>::into(NumActions {
            deleted_folders: 1,  // The root folder is deleted
            copied_files: 1,
            ..Default::default() })[..]].concat(),
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
            "--dest-root-needs-deleting=prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("1:.*:Skip"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new("dest root folder .* needs deleting").unwrap()),
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
            "--dest-root-needs-deleting=error".to_string(),
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
            "--dest-root-needs-deleting=skip".to_string(),
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
            "--dest-root-needs-deleting=delete".to_string(),
        ],
        expected_exit_code: 0,
        // The file inside and the root folder deleted
        expected_output_messages: NumActions {
            deleted_files: 1, deleted_folders: 1,
            copied_files: 1,
            ..Default::default() }.into(),
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&src)), // Dest root deleted and replaced by src, so same as src
        ],
        ..Default::default()
    });
}
