#[path = "../test_utils.rs"]
#[allow(unused)]
mod test_utils;
#[path = "../filesystem_node.rs"]
#[allow(unused)]
mod filesystem_node;
mod test_framework;

mod sync_tests;
mod filter_tests;
mod trailing_slash_tests;
mod remote_tests;
mod symlink_tests;
mod dest_file_newer_tests;
mod dest_file_older_tests;
mod files_same_time_tests;
mod dest_entry_needs_deleting_tests;
mod dest_root_needs_deleting_tests;
mod misc_tests;
