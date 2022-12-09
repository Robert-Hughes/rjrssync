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
