use std::time::{Duration, SystemTime};

use regex::Regex;

use crate::{folder, test_framework::{run, TestDesc}};
use crate::test_framework::*;
use map_macro::map;

/// Syncing three files which already exists on the dest, but the dest has newer modified
/// dates. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "skip" and then "overwrite", then "skip" again.
#[test]
fn prompt_skip_then_overwrite() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
        "c3" => file_with_modified("contents6", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "c2" => file_with_modified("contents4", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "c3" => file_with_modified("contents7", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Skip (just this occurence)"),
            String::from("Overwrite (just this occurence)"),
            String::from("Skip (just this occurence)"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new("dest file .*c1.* is newer than source file .*c1.*").unwrap()),
            (1, Regex::new("dest file .*c2.* is newer than source file .*c2.*").unwrap()),
            // Note that we need this last check, to make sure that the second prompt response only affects one file, not all remaining files
            (1, Regex::new("dest file .*c3.* is newer than source file .*c3.*").unwrap()), 
            (1, Regex::new(&regex::escape("Copied 1 file(s)")).unwrap()), // 1 file copied the other skipped
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&folder! { // First and last file skipped, the other overwritten
                "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
                "c2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
                "c3" => file_with_modified("contents7", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
            })),
        ],
        ..Default::default()
    });
}

/// Syncing two files which already exists on the dest, but the dest has newer modified
/// dates. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "skip all" on the first prompt, so both files should be skipped.
#[test]
fn prompt_skip_all() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "c2" => file_with_modified("contents4", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Skip (all occurences)"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new("dest file .*c1|2.* is newer than source file .*c1|2.*").unwrap()), // We don't know which file will be first, as it depends on the OS
            (1, Regex::new(&regex::escape("Nothing to do")).unwrap()), // Both files skipped
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Syncing two files which already exists on the dest, but the dest has newer modified
/// dates. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose "overwrite all" on the first prompt, so both files should be overwritten.
#[test]
fn prompt_overwrite_all() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
        "c2" => file_with_modified("contents2", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents3", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        "c2" => file_with_modified("contents4", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Overwrite (all occurences)"),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new("dest file .*c\\d.* is newer than source file .*c\\d.*").unwrap()), // We don't know which file will be first, as it depends on the OS, we just need it to be there only once
            (1, Regex::new(&regex::escape("Copied 2 file(s)")).unwrap()), // Both files copied
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&src)), // All files copied, so same as src
        ],
        ..Default::default()
    });
}

/// Syncing a file which already exists on the dest, but the dest has a newer modified
/// date. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to "prompt", and choose to cancel the prompt, so the sync should stop.
#[test]
fn prompt_cancel() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "prompt".to_string(),
        ],
        prompt_responses: vec![
            String::from("Cancel sync"),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            // We actaully get this message twice - once for the prompt and once in the error message after the prompt is cancelled
            (2, Regex::new("dest file .*c1.* is newer than source file .*c1.*").unwrap()),
            (1, Regex::new(&regex::escape("Will not overwrite")).unwrap()), // Cancelled
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Syncing a file which already exists on the dest, but the dest has a newer modified
/// date. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to produce an error.
#[test]
fn error() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "error".to_string(),
        ],
        expected_exit_code: 12,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("Will not overwrite")).unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&dest)), // Unchanged
        ],
        ..Default::default()
    });
}

/// Syncing a file which already exists on the dest, but the dest has a newer modified
/// date. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to skip.
#[test]
fn skip() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
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

/// Syncing a file which already exists on the dest, but the dest has a newer modified
/// date. The expected behaviour is controlled by a command-line argument, which in this case
/// we set to overwrite.
#[test]
fn overwrite() {
    let src = folder! {
        "c1" => file_with_modified("contents1", SystemTime::UNIX_EPOCH),
    };
    let dest = folder! {
        "c1" => file_with_modified("contents2", SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
    };
    run(TestDesc {
        setup_filesystem_nodes: vec![
            ("$TEMP/src", &src),
            ("$TEMP/dest", &dest),
        ],
        args: vec![
            "$TEMP/src".to_string(),
            "$TEMP/dest".to_string(),
            "--dest-file-newer".to_string(),
            "overwrite".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new(&regex::escape("Copied 1 file(s)")).unwrap()),
        ],
        expected_filesystem_nodes: vec![
            ("$TEMP/src", Some(&src)), // Unchanged
            ("$TEMP/dest", Some(&src)), // Same as source
        ],
        ..Default::default()
    });
}
