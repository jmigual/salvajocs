//! The space each game's backups use, worked out off the UI thread and kept in the session's
//! cache (ADR-0027).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use eframe::egui;

use crate::job::Job;
use crate::state::{SharedStorage, SpaceCache, lock_storage};

/// The figures of one run, failures included, by a game's sorted snapshot ids.
type Done = Arc<Mutex<HashMap<Vec<String>, Result<u64, String>>>>;

/// What [`Space::poll`] found since its last call.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Polled {
    /// Nothing new.
    Nothing,
    /// New figures; the job goes on.
    Arrived,
    /// The job ended.
    Finished,
}

/// A run that works out the space of some games' backups, and reads the figures. Dropping it
/// stops the job after the game it is on.
pub(crate) struct Space {
    cache: SpaceCache,
    job: Option<Job<()>>,
    stop: Arc<AtomicBool>,
    done: Done,
    /// How many figures of `done` the last poll saw.
    seen: usize,
}

impl Space {
    /// Works out, in a job, the space of each of `games` (a game's sorted snapshot ids) that
    /// `cache` lacks, locking `storage` for one game at a time. No job runs when the cache has
    /// them all.
    pub(crate) fn start(
        ctx: &egui::Context,
        storage: &SharedStorage,
        cache: &SpaceCache,
        games: Vec<Vec<String>>,
    ) -> Self {
        let missing: Vec<_> = {
            let cached = lock(cache);
            (games.into_iter())
                .filter(|ids| !cached.contains_key(ids))
                .collect()
        };
        let (stop, done) = (Arc::<AtomicBool>::default(), Done::default());
        let job = (!missing.is_empty()).then(|| {
            let (storage, cache, waker) = (storage.clone(), cache.clone(), ctx.clone());
            let (stop, done) = (stop.clone(), done.clone());
            Job::spawn(ctx, move || {
                for ids in missing {
                    let mut storage = lock_storage(&storage)?;
                    // Checked once the lock is held: a backup may have run meanwhile.
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // Another tab's run may have worked it out; it still counts as arrived.
                    let cached = lock(&cache).get(&ids).copied();
                    let result = match cached {
                        Some(bytes) => Ok(bytes),
                        None => {
                            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                            let result = storage.space_used(&refs).map_err(|e| e.to_string());
                            if let Ok(bytes) = result {
                                lock(&cache).insert(ids.clone(), bytes);
                            }
                            result
                        }
                    };
                    drop(storage);
                    lock(&done).insert(ids, result);
                    waker.request_repaint();
                }
                Ok(())
            })
        });
        Self {
            cache: cache.clone(),
            job,
            stop,
            done,
            seen: 0,
        }
    }

    /// Whether figures arrived or the job ended since the last call.
    pub(crate) fn poll(&mut self) -> Polled {
        if let Some(job) = &mut self.job
            && job.take_result().is_some()
        {
            self.job = None;
            return Polled::Finished;
        }
        let count = lock(&self.done).len();
        if std::mem::replace(&mut self.seen, count) == count {
            Polled::Nothing
        } else {
            Polled::Arrived
        }
    }

    /// The space of the game with the sorted snapshot `ids`: `None` while it is being worked
    /// out, `Err` when that failed.
    pub(crate) fn get(&self, ids: &[String]) -> Option<Result<u64, String>> {
        if let Some(result) = lock(&self.done).get(ids) {
            return Some(result.clone());
        }
        lock(&self.cache).get(ids).map(|&bytes| Ok(bytes))
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
