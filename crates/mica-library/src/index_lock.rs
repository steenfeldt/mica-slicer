// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original

//! Opens the index so another Mica process can still read it on Windows.
//!
//! redb's default (exclusive-writer) open locks the file from byte 0 up to its lock bytes at
//! 2^62. On Windows byte-range locks are mandatory: every other handle's `ReadFile` in that
//! range fails (ERROR_LOCK_VIOLATION), so a second Mica window could not copy the index to
//! browse it read-only ([`crate::store::Store::read_snapshot`]). There the index is opened
//! through [`DataUnlocked`], which moves the part of a lock that covers the data onto one
//! proxy byte just below redb's lock bytes. Opens still exclude each other exactly as before
//! (every open takes the proxy byte and the rest of redb's ranges), and the data stays
//! readable. Elsewhere locks are advisory and the default open is kept.

use redb::{Database, DatabaseError};
use std::path::Path;

/// Opens (or creates) the index file like [`Database::create`].
pub fn create(path: &Path) -> Result<Database, DatabaseError> {
    #[cfg(windows)]
    return unlocked::create_data_unlocked(path);
    #[cfg(not(windows))]
    Database::create(path)
}

#[cfg(any(windows, test))]
mod unlocked {
    use redb::backends::FileBackend;
    use redb::{BackendError, Database, DatabaseError, StorageBackend};
    use std::ops::Bound;
    use std::path::Path;

    /// redb's first lock byte; it never locks a range that starts in (0, LOCK_BASE).
    const LOCK_BASE: u64 = 1 << 62;
    /// Stands in for the data bytes in every lock that covers them.
    const DATA_PROXY: u64 = LOCK_BASE - 1;

    /// Opens (or creates) `path` with its data bytes left unlocked.
    pub fn create_data_unlocked(path: &Path) -> Result<Database, DatabaseError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        Database::builder().create_with_backend(DataUnlocked(FileBackend::new(file)?))
    }

    /// redb's file backend with lock ranges that start in the data moved to [`DATA_PROXY`].
    #[derive(Debug)]
    struct DataUnlocked(FileBackend);

    fn shift(start: Bound<u64>, end: Bound<u64>) -> (Bound<u64>, Bound<u64>) {
        // A whole-storage lock (redb's fallback for backends without ranges) stays whole.
        if matches!(start, Bound::Unbounded | Bound::Included(0)) && end == Bound::Unbounded {
            return (start, end);
        }
        let in_data = match start {
            Bound::Unbounded => true,
            Bound::Included(s) => s < LOCK_BASE,
            Bound::Excluded(s) => s < DATA_PROXY,
        };
        if !in_data {
            return (start, end);
        }
        let end = match end {
            Bound::Included(e) if e < DATA_PROXY => Bound::Included(DATA_PROXY),
            Bound::Excluded(e) if e <= DATA_PROXY => Bound::Included(DATA_PROXY),
            other => other,
        };
        (Bound::Included(DATA_PROXY), end)
    }

    impl StorageBackend for DataUnlocked {
        fn len(&self) -> std::io::Result<u64> {
            self.0.len()
        }
        fn read(&self, offset: u64, out: &mut [u8]) -> std::io::Result<()> {
            self.0.read(offset, out)
        }
        fn set_len(&self, len: u64) -> std::io::Result<()> {
            self.0.set_len(len)
        }
        fn sync_data(&self) -> std::io::Result<()> {
            self.0.sync_data()
        }
        fn write(&self, offset: u64, data: &[u8]) -> std::io::Result<()> {
            self.0.write(offset, data)
        }
        fn close(&self) -> std::io::Result<()> {
            self.0.close()
        }
        fn try_lock_range(&self, start: Bound<u64>, end: Bound<u64>) -> Result<bool, BackendError> {
            let (s, e) = shift(start, end);
            self.0.try_lock_range(s, e)
        }
        fn try_lock_shared_range(
            &self,
            start: Bound<u64>,
            end: Bound<u64>,
        ) -> Result<bool, BackendError> {
            let (s, e) = shift(start, end);
            self.0.try_lock_shared_range(s, e)
        }
        fn lock_range(&self, start: Bound<u64>, end: Bound<u64>) -> Result<(), BackendError> {
            let (s, e) = shift(start, end);
            self.0.lock_range(s, e)
        }
        fn lock_shared_range(
            &self,
            start: Bound<u64>,
            end: Bound<u64>,
        ) -> Result<(), BackendError> {
            let (s, e) = shift(start, end);
            self.0.lock_shared_range(s, e)
        }
        fn unlock_range(&self, start: Bound<u64>, end: Bound<u64>) -> Result<(), BackendError> {
            let (s, e) = shift(start, end);
            self.0.unlock_range(s, e)
        }
        fn query_lock_range(
            &self,
            start: Bound<u64>,
            end: Bound<u64>,
        ) -> Result<bool, BackendError> {
            let (s, e) = shift(start, end);
            self.0.query_lock_range(s, e)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use redb::{ReadableDatabase, TableDefinition};

        const T: TableDefinition<'static, u64, u64> = TableDefinition::new("t");

        #[test]
        fn opens_exclude_each_other_and_the_data_stays_readable() {
            let dir = crate::testkit::TempDir::new("index-lock");
            let path = dir.path().join("index.redb");
            let db = create_data_unlocked(&path).unwrap();
            let tx = db.begin_write().unwrap();
            tx.open_table(T).unwrap().insert(1, 2).unwrap();
            tx.commit().unwrap();
            // A second open of either kind is refused, as with redb's own locking.
            assert!(matches!(
                create_data_unlocked(&path),
                Err(DatabaseError::DatabaseAlreadyOpen)
            ));
            assert!(matches!(
                Database::create(&path),
                Err(DatabaseError::DatabaseAlreadyOpen)
            ));
            // The bytes can be read (on Windows too) and opened as a private copy.
            let copy = dir.path().join("copy.redb");
            std::fs::write(&copy, std::fs::read(&path).unwrap()).unwrap();
            let view = Database::create(&copy).unwrap();
            let rtx = view.begin_read().unwrap();
            assert_eq!(
                rtx.open_table(T).unwrap().get(1).unwrap().unwrap().value(),
                2
            );
            drop((rtx, view, db));
            // Closed: it opens again, both ways.
            drop(create_data_unlocked(&path).unwrap());
            drop(Database::create(&path).unwrap());
        }

        #[test]
        fn only_ranges_starting_in_the_data_move() {
            use Bound::*;
            assert_eq!(
                shift(Unbounded, Included(LOCK_BASE + 1)),
                (Included(DATA_PROXY), Included(LOCK_BASE + 1))
            );
            assert_eq!(shift(Unbounded, Unbounded), (Unbounded, Unbounded));
            assert_eq!(
                shift(Included(0), Excluded(10)),
                (Included(DATA_PROXY), Included(DATA_PROXY))
            );
            let suffix = (Included(LOCK_BASE + 1024), Unbounded);
            assert_eq!(shift(suffix.0, suffix.1), suffix);
            let middle = (Excluded(LOCK_BASE + 1), Excluded(LOCK_BASE + 896));
            assert_eq!(shift(middle.0, middle.1), middle);
        }
    }
}
