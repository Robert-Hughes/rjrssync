#[macro_export]
macro_rules! function_name {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        &name[..name.len() - 3].split("::").last().unwrap()
    }};
}

#[cfg(feature = "profiling")]
pub mod profiling_real;
#[cfg(feature = "profiling")]
pub use profiling_real::*;

#[cfg(not(feature = "profiling"))]
pub mod profiling_fake;
#[cfg(not(feature = "profiling"))]
pub use profiling_fake::*;

/// Gets the peak memory usage of the current process.
pub fn get_peak_memory_usage() -> usize {
    #[cfg(windows)]
    unsafe {
        let mut counters : winapi::um::psapi::PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        let handle = winapi::um::processthreadsapi::GetCurrentProcess();
        if winapi::um::psapi::GetProcessMemoryInfo(handle, &mut counters, 
            std::mem::size_of::<winapi::um::psapi::PROCESS_MEMORY_COUNTERS>() as u32) == 0 
        {
            panic!("Win32 API failed!");
        }
        // I think this only accounts for physical memory, not paged memory, but hopefully that's fine
        counters.PeakWorkingSetSize
    }
    #[cfg(unix)]
    {
        std::fs::read_to_string(format!("/proc/{}/status", std::process::id()))
            .expect("Failed to read /proc/X/status")
            .lines().filter(|l| l.starts_with("VmPeak")).next().expect("Couldn't find VmPeak line")
            .split_once(':').expect("Failed to parse line").1.trim()
            .trim_end_matches(|c: char| !c.is_digit(10))
            .parse::<usize>().expect("Failed to parse number") * 1024 // Value is always in kB
    }
}
