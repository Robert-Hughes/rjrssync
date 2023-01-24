use std::{collections::HashMap, hash::Hash};

/// A map of K -> V which remembers the order in which things were added,
/// and uses this order when iterating.
/// Implemented simply as storing both a Vec and HashMap, and keeping these in sync.
#[derive(Debug, Clone)]
pub struct OrderedMap<K, V> {
    // Note that the vec doesn't store V. This means we don't have to keep V up to date here
    // (e.g. in update()). We have to do a lookup in the map anyway when iterating, so we also
    // fetch the V while we're there.
    vec: Vec<K>,
    map: HashMap<K, V>
}
impl<K: Clone+ Eq + Hash, V: Clone> OrderedMap<K, V> {
    pub fn new() -> OrderedMap<K, V> {
        OrderedMap { vec: vec![], map: HashMap::new() }
    }

    pub fn add(&mut self, k: K, v: V) {
        self.vec.push(k.clone());
        self.map.insert(k, v);
    }

    pub fn len(&self) -> usize {
        // The vec len may be larger, if things have been removed, but the map len is always correct
        self.map.len()
    }

    pub fn iter(&self) -> Box<dyn Iterator<Item = (&K, &V)> + '_> {
        // Some entries may have been removed, so filter these out on the fly.
        // Also grab the V from the map.
        let iter = self.vec.iter().filter_map(|k| self.map.get(k).and_then(|v| Some((k, v))));
        Box::new(iter)
    }

    pub fn lookup(&self, k: &K) -> Option<&V> {
        self.map.get(k)
    }

    pub fn remove(&mut self, k: &K) {
        self.map.remove(k);
        // We don't remove from the vec, as that could be slow (shuffling data around).
        // Instead we make sure to check when iterating that the entry hasn't been removed
    }

    pub fn reverse_order(&mut self) {
        self.vec.reverse();
    }

    pub fn update(&mut self, k: &K, new_value: V) {
        *self.map.get_mut(k).unwrap() = new_value;
    }
}
