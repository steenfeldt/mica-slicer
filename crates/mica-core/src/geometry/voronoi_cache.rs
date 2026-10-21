// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Bounded memoization of the pure segment Voronoi calculation. Exact ordered segment keys
//! include the coordinate frame; no translations, tolerances, or input canonicalization.
//! Shared across Rayon workers so preparation, tool-usage and export can reuse diagrams.

use super::voronoi::{Diagram, Line};
use std::collections::{HashMap, VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

const SHARDS: usize = 16;
const BYTES_PER_SHARD: usize = 128 * 1024 * 1024 / SHARDS;
const MAX_ENTRY_BYTES: usize = BYTES_PER_SHARD / 4;
type Result = (Diagram, bool);

#[derive(Default)]
struct Cache {
    entries: HashMap<Arc<[Line]>, Arc<Result>>,
    order: VecDeque<Arc<[Line]>>,
    bytes: usize,
}

fn entry_bytes(lines: &[Line], result: &Result) -> usize {
    // Map and FIFO share one exact key; account for backing storage and metadata.
    std::mem::size_of_val(lines)
        + result.0.cells.capacity() * std::mem::size_of::<super::voronoi::Cell>()
        + result.0.vertices.capacity() * std::mem::size_of::<super::voronoi::Vertex>()
        + result.0.edges.capacity() * std::mem::size_of::<super::voronoi::Edge>()
        + 256
}

impl Cache {
    fn insert(&mut self, lines: &[Line], result: Arc<Result>) {
        let bytes = entry_bytes(lines, &result);
        if bytes > MAX_ENTRY_BYTES || self.entries.contains_key(lines) {
            return;
        }
        while self.bytes + bytes > BYTES_PER_SHARD {
            let key = self.order.pop_front().unwrap();
            let previous = self.entries.remove(&key).unwrap();
            self.bytes -= entry_bytes(&key, &previous);
        }
        self.bytes += bytes;
        let key: Arc<[Line]> = Arc::from(lines);
        self.entries.insert(Arc::clone(&key), result);
        self.order.push_back(key);
    }
}

pub(super) fn get_or_build(lines: &[Line], build: impl FnOnce() -> Result) -> Result {
    static CACHE: OnceLock<[Mutex<Cache>; SHARDS]> = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::array::from_fn(|_| Mutex::default()));
    let mut hasher = DefaultHasher::new();
    lines.hash(&mut hasher);
    let shard = &cache[hasher.finish() as usize % SHARDS];
    // Release the lock before cloning the diagram or doing any geometry work. Concurrent
    // misses may build twice; they must never serialize the sweep or wait on another worker.
    let found = shard
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entries
        .get(lines)
        .cloned();
    if let Some(found) = found {
        let _timer = crate::timing::Span::new(crate::timing::Stage::VoronoiCacheHit);
        return (*found).clone();
    }
    let result = Arc::new(build());
    shard
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(lines, Arc::clone(&result));
    Arc::try_unwrap(result).unwrap_or_else(|result| (*result).clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Point;

    #[test]
    fn exact_ordered_keys_and_bounded_eviction() {
        let mut cache = Cache::default();
        let lines = [Line::new(Point::new(1, 2), Point::new(3, 4))];
        let result = Arc::new((Diagram::default(), true));
        cache.insert(&lines, Arc::clone(&result));
        assert!(Arc::ptr_eq(
            cache.entries.get(lines.as_slice()).unwrap(),
            &result
        ));
        let reverse = [Line::new(lines[0].b, lines[0].a)];
        assert!(!cache.entries.contains_key(reverse.as_slice()));
        let large = Arc::new((
            Diagram {
                edges: Vec::with_capacity(10_000),
                ..Default::default()
            },
            false,
        ));
        for x in 0..100 {
            cache.insert(
                &[Line::new(Point::new(x, 0), Point::new(x, 1))],
                Arc::clone(&large),
            );
            assert!(cache.bytes <= BYTES_PER_SHARD);
        }
        assert!(!cache.entries.contains_key(lines.as_slice()));
        assert!(cache.entries.len() < 100);
    }
}
