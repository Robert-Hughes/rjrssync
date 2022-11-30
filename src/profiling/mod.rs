
#[cfg(feature = "profiling")]
pub mod profiling_real;

#[cfg(not(feature = "profiling"))]
pub mod profiling_fake;

pub mod profiling{
    #[cfg(feature = "profiling")]
    pub use super::profiling_real::*;

    #[cfg(not(feature = "profiling"))]
    pub use super::profiling_fake::*;
}
