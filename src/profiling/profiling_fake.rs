use std::time::Duration;
use super::ProcessProfilingData;

#[macro_export]
macro_rules! profile_this {
    ($($tts:tt)*) => {};
}

pub fn start_timer(_name: &str) -> () {}
pub fn stop_timer(_t: ()) {}

#[allow(dead_code)]
pub fn dump_all_profiling() {
}

pub fn add_remote_profiling(_remote_profiling_data: ProcessProfilingData, _process_name: String, _offset: Duration) {    
}

pub fn get_local_process_profiling() -> ProcessProfilingData {
    ProcessProfilingData::default()
}
