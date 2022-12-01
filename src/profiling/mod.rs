
#[cfg(feature = "profiling")]
pub mod profiling_real;
#[cfg(feature = "profiling")]
pub use profiling_real::*;

#[cfg(not(feature = "profiling"))]
pub mod profiling_fake;
#[cfg(not(feature = "profiling"))]
pub use profiling_fake::*;
