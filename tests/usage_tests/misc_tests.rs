use crate::test_framework::*;
use regex::Regex;

/// Checks that --generate-auto-complete-script works
#[test]
fn generate_auto_complete_script() {
    run(TestDesc {
        args: vec![
            "--generate-auto-complete-script=bash".to_string(),
        ],
        expected_exit_code: 0,
        expected_output_messages: vec![
            (1, Regex::new("complete -F _rjrssync -o bashdefault -o default rjrssync").unwrap())
        ],
        ..Default::default()
    });
}
