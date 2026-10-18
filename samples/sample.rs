//! 示例：用 coderender 渲染这段代码
//! 支持中文注释（会使用系统非等宽字体回退）。

use std::collections::HashMap;
use std::sync::Arc;

/// 缓存条目：记录命中次数与最后访问时间。
#[derive(Debug, Clone)]
pub struct Entry<V> {
    pub value: V,
    pub hits: u64,
    pub last_access: u64,
}

pub struct LruCache<K, V> {
    capacity: usize,
    clock: u64,
    map: HashMap<K, Arc<Entry<V>>>,
}

impl<K: std::hash::Hash + Eq + Clone, V: Clone> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            clock: 0,
            map: HashMap::new(),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<V> {
        self.clock += 1;
        let entry = self.map.get(key)?.clone();
        Some(entry.value.clone())
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.clock += 1;
        let previous = self.map.insert(
            key,
            Arc::new(Entry {
                value,
                hits: 0,
                last_access: self.clock,
            }),
        );
        previous.map(|entry| Arc::try_unwrap(entry).ok().map(|e| e.value).unwrap())
    }

    pub fn stats(&self) -> (usize, u64) {
        let hits: u64 = self.map.values().map(|entry| entry.hits).sum();
        (self.map.len(), hits)
    }
}

fn main() {
    let mut cache = LruCache::new(128);
    for index in 0..256 {
        cache.insert(index, index * index);
    }
    println!("size = {}, hits = {}", cache.stats().0, cache.stats().1);
}
