extern crate alloc;

use core::hash::{Hash, Hasher};

use alloc::{collections::BTreeMap, vec::Vec};

use super::sync::RwLock;

/// see:
/// https://www.rfc-editor.org/rfc/rfc9923.html#name-fnv-1a-c-code
#[repr(transparent)]
struct FnvHasher(u64);

const FNV_BASIS: u64 = 0x_cbf2_9ce4_8422_2325_;

// 0x0000_0100_0000_01B3
const FNV_PRIME: u64 = 2u64.pow(40) + 2u64.pow(8) + 0x_b3_u64;

impl Default for FnvHasher {
    fn default() -> Self {
        Self(FNV_BASIS)
    }
}

impl Hasher for FnvHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= byte as u64;
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }
}

pub struct ShardedHashMap<K, V> {
    mask: u64,
    shards: Vec<RwLock<BTreeMap<K, V>>>,
}

impl<K: Ord + Hash, V: Clone> ShardedHashMap<K, V> {
    pub fn new(shards: usize) -> Self {
        assert!(shards.is_power_of_two(), "number of shards must be 2^n");

        let mut shards_vec = Vec::with_capacity(shards);
        for _ in 0..shards {
            shards_vec.push(RwLock::new(BTreeMap::new()));
        }

        Self {
            mask: (shards - 1) as u64,
            shards: shards_vec,
        }
    }

    fn get_shard_i<Q: Hash + ?Sized>(&self, key: &Q) -> usize {
        let mut hash = FnvHasher::default();
        key.hash(&mut hash);
        (hash.finish() & self.mask) as usize
    }

    pub fn insert(&self, key: K, value: V) {
        let i = self.get_shard_i(&key);
        let mut shard = self.shards[i].write();
        shard.insert(key, value);
    }

    pub fn remove(&self, key: &K) -> Option<V> {
        let i = self.get_shard_i(key);
        let mut shard = self.shards[i].write();
        shard.remove(key)
    }

    pub fn get_cloned(&self, key: &K) -> Option<V>
    where
        V: Clone,
    {
        let i = self.get_shard_i(key);
        let shard = self.shards[i].read();
        shard.get(key).cloned()
    }

    pub fn with_value<R, F: FnOnce(&V) -> R>(&self, key: &K, f: F) -> Option<R> {
        let i = self.get_shard_i(key);
        let shard = self.shards[i].read();
        shard.get(key).map(f)
    }

    pub fn update<R, F: FnOnce(&mut V) -> R>(&self, key: &K, f: F) -> Option<R> {
        let i = self.get_shard_i(key);
        let mut shard = self.shards[i].write();
        shard.get_mut(key).map(f)
    }

    pub fn contains_key(&self, key: &K) -> bool {
        let i = self.get_shard_i(key);
        let shard = self.shards[i].read();
        shard.contains_key(key)
    }
}
