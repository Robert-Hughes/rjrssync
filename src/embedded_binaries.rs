use serde::{Deserialize, Serialize};

/// The name of the section we add to big binaries which contains the embedded binary data.
/// Must be <= 8 chars as this is truncated on Windows exes.
pub const SECTION_NAME: &'static str = ".rjembed";

/// Definition of embedded binaries table, in a separate file for sharing
/// between build.rs and boss_deploy.rs.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct EmbeddedBinaries {
    pub binaries: Vec<EmbeddedBinary>,
}

/// Each entry in the table is a binary for a particular target triple.
#[derive(Serialize, Deserialize, Debug)]
pub struct EmbeddedBinary {
    pub target_triple: String,
    #[serde(with = "serde_bytes")] // Make serde fast
    pub data: Vec<u8>,
}
