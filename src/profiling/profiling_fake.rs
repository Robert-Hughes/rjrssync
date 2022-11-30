
#[macro_export]
macro_rules! function {
    () => {};
}
#[macro_export]
macro_rules! profile_this {
    ($($tts:tt)*) => {};
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct ProfilingData {
}

#[allow(dead_code)]
fn dump_profiling_to_chrome(_: &ProfilingData, _: String) {
    panic!();
}
