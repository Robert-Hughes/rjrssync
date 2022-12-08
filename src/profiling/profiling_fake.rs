#[macro_export]
macro_rules! profile_this {
    ($($tts:tt)*) => {};
}

pub fn start_timer(_name: &str) -> () {}
pub fn stop_timer(_t: ()) {}

#[allow(dead_code)]
pub fn dump_all_profiling() {
}
