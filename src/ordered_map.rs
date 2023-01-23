use std::{collections::HashMap, hash::Hash};

/// A list of K and V which is ordered and has fast lookup from 
/// K -> V.
/// Implemented simply as storing both a Vec and HashMap, and keeping these in sync.
pub struct OrderedMap<K, V> {
    vec: Vec<(K, V)>,
    map: HashMap<K, V>
}
impl<K: Clone+ Eq + Hash, V: Clone> OrderedMap<K, V> {
    pub fn new() -> OrderedMap<K, V> {
        OrderedMap { vec: vec![], map: HashMap::new() }
    }

    pub fn add(&mut self, path: K, entry: V) {
        self.vec.push((path.clone(), entry.clone()));
        self.map.insert(path, entry);
    }

    pub fn len(&self) -> usize {
        // The vec len may be larger, if things have been removed, but the map len is always correct
        self.map.len() 
    }

    pub fn iter(&self) -> Box<dyn Iterator<Item = &(K, V)> + '_> {
        // Some entries may have been removed, so filter these out on the fly
        let iter = self.vec.iter().filter(|(p, e)| self.map.contains_key(p));
        Box::new(iter)
    }

    pub fn lookup(&self, p: &K) -> Option<&V> {
        self.map.get(p)
    }

    pub fn remove(&mut self, p: &K) {
        self.map.remove(p);
        // We don't remove from the vec, as that could be slow (shuffling data around).
        // Instead we make sure to check when iterating that the entry hasn't been removed
    }

    pub fn reverse_order(&mut self) {
        self.vec.reverse();
    }
}