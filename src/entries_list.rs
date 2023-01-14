use std::{collections::HashMap, slice::Iter};

use crate::{root_relative_path::RootRelativePath, boss_doer_interface::EntryDetails};

/// A list of RootRelativePath and EntryDetails which is ordered and has fast lookup from 
/// RootRelativePath -> EntryDetails.
/// Implemented simply as storing both a Vec and HashMap, and keeping these in sync.
pub struct EntriesList {
    vec: Vec<(RootRelativePath, EntryDetails)>,
    map: HashMap<RootRelativePath, EntryDetails>
}
impl EntriesList {
    pub fn new() -> EntriesList {
        EntriesList { vec: vec![], map: HashMap::new() }
    }

    pub fn add(&mut self, path: RootRelativePath, entry: EntryDetails) {
        self.vec.push((path.clone(), entry.clone()));
        self.map.insert(path, entry);
    }

    pub fn len(&self) -> usize {
        self.vec.len()
    }

    pub fn iter(&self) -> Iter<(RootRelativePath, EntryDetails)> {
        self.vec.iter()
    }

    pub fn lookup(&self, p: &RootRelativePath) -> Option<&EntryDetails> {
        self.map.get(p)
    }
}