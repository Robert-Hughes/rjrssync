use std::{ops::{AddAssign, SubAssign}, time::{Instant, Duration}, thread, sync::{Arc}};

use crossbeam::atomic::AtomicCell;
use indicatif::{ProgressBar, HumanCount, HumanBytes, ProgressStyle, WeakProgressBar, ProgressDrawTarget};

use crate::{boss_doer_interface::{EntryDetails, ProgressPhase, ProgressMarker}, root_relative_path::RootRelativePath, boss_sync::Actions};

/// FPS of progress bar update.
// This has a surprisingly significant effect on performance, seen especially when
// copying a large file. We had a regression on the perf results around 15th Jan when this
// value was increased (commit "Update progress partway through large files").
const BAR_UPDATE_RATE : f32 = 5.0;
/// The file size below which we assume that overhead is dominant, so the work is constant.
const MIN_FILE_SIZE : u64 = 1024*1024;
/// The minimum amount of work between progress markers.
const MARKER_THRESHOLD: u64 = 1024*1024;
/// The amount of work for deletes.
const DELETE_WORK: u64 = 1024*1024;

/// Set of related measurements for progress.
#[derive(Default, PartialEq, Eq, Debug, Clone)]
struct ProgressValues {
    /// Generic measure of how work, measured in arbitrary units to account for different file sizes etc.
    /// This is an estimate of how much 'time' it will take.
    work: u64,
    /// Number of entry deletions.
    delete: u32,
    /// Number of entry copies/creates.
    copy: u32,
    /// Number of bytes of file copies.
    copy_bytes: u64,
}
impl ProgressValues {
    /// Creates a set of ProgressValues to represent the copying of a single entry.
    fn for_copy(e: &EntryDetails) -> Self {
        match e {
            EntryDetails::File { size, .. } => {
                ProgressValues {
                    work: std::cmp::max(*size, MIN_FILE_SIZE), // Even small files will take some minimum amount of time to copy
                    copy: 1,
                    copy_bytes: *size,
                    ..Default::default()
                }
            },
            EntryDetails::Folder | EntryDetails::Symlink{..} => ProgressValues {
                work: MIN_FILE_SIZE, // Assume that folders/symlinks are equivalent to a small file
                copy: 1,
                ..Default::default()
            }
        }
    }

    /// Creates a set of ProgressValues to represent the copying of some amount (chunk) of a single file.
    fn for_copy_partial(chunk_start: u64, chunk_size: u64, file_size: u64) -> Self {
        // For files smaller than a threshold, we use a constant amount of work to account for the overhead.
        // We put all this overhead into the final chunk, as we assume that the chunk size is going to be larger
        // than the threshold so it doesn't matter anyway
        if chunk_start + chunk_size < file_size {
            // Non-final chunks of the file
            ProgressValues {
                work: if file_size > MIN_FILE_SIZE {
                    chunk_size
                } else {
                    0
                },
                copy: 0, // Only once the file is complete will be increase this
                copy_bytes: chunk_size,
                ..Default::default()
            }
        } else {
            // The last chunk of the file
            ProgressValues {
                work: if file_size > MIN_FILE_SIZE {
                    chunk_size
                } else {
                    MIN_FILE_SIZE
                },
                copy: 1,
                copy_bytes: chunk_size,
                ..Default::default()
            }
        }
    }

    /// Creates a set of ProgressValues to represent the deletion of a single entry.
    fn for_delete(_e: &EntryDetails) -> Self {
        ProgressValues {
            work: DELETE_WORK,
            delete: 1,
            ..Default::default()
        }
    }
}
impl AddAssign for ProgressValues {
    fn add_assign(&mut self, rhs: Self) {
        self.work += rhs.work;
        self.delete += rhs.delete;
        self.copy += rhs.copy;
        self.copy_bytes += rhs.copy_bytes;
    }
}
impl SubAssign for ProgressValues {
    fn sub_assign(&mut self, rhs: Self) {
        self.work -= rhs.work;
        self.delete -= rhs.delete;
        self.copy -= rhs.copy;
        self.copy_bytes -= rhs.copy_bytes;
    }
}

/// State to communicate with the background thread.
struct BarState {
    is_deleting: bool,
    completed: ProgressValues,
    total: ProgressValues,
    current_entry: Option<RootRelativePath>,
}

/// Wrapper around progress-bar related logic, which is a little tricky.
///
/// Because the destination doer is asynchronous, just because the boss has sent a command to (e.g.)
/// write a file, that command won't be completed until some time in the future, so we can't advance
/// the progress bar until that point. We also want the progress bar to move smoothly across
/// different kinds of operation (deletes, copies of small files, copies of large files), all of
/// which might take different amounts of time to complete.
///
/// The solution works like this. During the querying phase of the sync, we simply show
/// "Querying...", as we have no good indication of progress here. Once we know what we're going to
/// be doing, we replace this with a proper progress bar, with length scaled heuristically to
/// account for copies/deletes of different sizes etc. The boss will (at some limited interval) send
/// "progress marker" commands to the dest doer, so that when the doer gets to that command it will
/// echo it back to the boss to show us how far the doer has got. When we receive that echoed marker
/// back, we update a counter here with the amount of work completed. The progress bar shows the
/// amount completed vs. the total.
///
/// There's an additional requirement to keep the overhead of updating the progress bar small,
/// especially for very fast syncs where nothing has changed. In the past we were updating the
/// totals a lot more frequency and so made a background thread which updates it periodically.
/// Things might not be so bad now, but the background thread is still there (perhaps unnecessarily).
pub struct Progress {
    /// The UI element from the `indicatif` crate that handles drawing the progress bar.
    bar: ProgressBar,

    /// The total amount of work that the dest doer needs to complete, set on creation.
    total: ProgressValues,
    /// Keeps track of how much work has been sent to the dest doer
    /// so far. The doer won't necessarily have completed (or even received) this work yet, so the
    /// progress bar isn't updated until we receive ProgressMarkers back from the doer.
    sent: ProgressValues,
    /// Keeps track of how much work has been completed by the dest doer so far.
    completed: ProgressValues,

    /// This monstrosity is for sharing the BoxState with the background thread.
    new_bar_state: Arc<AtomicCell<Option<Box<BarState>>>>,

    /// The work value of the last ProgressMarker we sent to the doer. Used to avoid sending
    /// too many markers in a short space of time to reduce the overhead of measuring progress.
    last_progress_marker: u64,

    /// The time at which we received a progress marker from the dest doer showing that it had finished
    /// the deletes and had moved on to the copies.
    first_copy_time: Option<Instant>,

    /// Lists of source and dest paths, so that we can match up progress markers
    /// to filenames to display on the progress bar.
    to_copy_paths: Vec<RootRelativePath>,
    to_delete_paths: Vec<RootRelativePath>,
}
impl Progress {
    pub fn new(actions: &Actions) -> Self {
        // Sum up the total amount of work that needs doing, and store a copy of the
        // paths so that we can display these on the progress bar as we go
        let mut to_delete_paths = vec![];
        let mut total = ProgressValues::default();
        for (p, (d, _)) in actions.to_delete.iter() {
            total += ProgressValues::for_delete(d);
            to_delete_paths.push(p.clone());
        }
        let mut to_copy_paths = vec![];
        for (p, (c, _)) in actions.to_copy.iter() {
            total += ProgressValues::for_copy(c);
            to_copy_paths.push(p.clone());
        }

        // Set up the UI element
        // Note that we don't render the pos or length in the template, as the 'work' values are pretty meaningless
        // for the user. Instead we show the percentage, and include a custom message where we print more details
        // Note the use of "wide_msg" vs "msg", which prevents the message from wrapping to the next line
        // when the filename is long, which causes problems.
        let bar = ProgressBar::new(total.work).with_style(
            ProgressStyle::with_template("{percent}% {bar:40.green/black} {wide_msg}").unwrap());

         // We control the update rate ourselves with our background thread, so disable(reduce) the built-in limiting
        bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(60));
        let new_bar_state = Arc::new(AtomicCell::new(None));

        let bar2 = bar.downgrade(); // Weak reference for the background thread
        let new_bar_state2 = new_bar_state.clone();
        thread::Builder::new().name("Progress bar".to_string()).spawn(
            move || Self::background_updater(bar2, new_bar_state2)).expect("Failed to spawn thread");

        Progress {
            bar,
            total,
            sent: ProgressValues::default(),
            completed: ProgressValues::default(),
            new_bar_state,
            last_progress_marker: 0,
            first_copy_time: None,
            to_delete_paths,
            to_copy_paths,
        }
    }

    /// Gets a ProgressMarker to be sent to the dest doer to mark the amount of work
    /// that has been already sent.
    /// This might return None if the last update was sent too recently, to avoid too much overhead
    /// from the progress markers.
    pub fn get_progress_marker_limited(&mut self) -> Option<ProgressMarker> {
        // Don't send progress markers too often, to avoid overhead
        if self.sent.work - self.last_progress_marker < MARKER_THRESHOLD {
            return None
        }
        Some(self.get_progress_marker())
    }

    /// Gets a ProgressMarker to be sent to the dest doer to mark the amount of work
    /// that has been already sent
    pub fn get_progress_marker(&mut self) -> ProgressMarker {
        // Remember when we last sent a marker, so that we don't do it too often
        self.last_progress_marker = self.sent.work;

        debug_assert!(self.sent.delete <= self.total.delete);
        debug_assert!(self.sent.copy <= self.total.copy);
        if self.sent.delete < self.total.delete {
            // Still sending deletes
            ProgressMarker {
                completed_work: self.sent.work,
                phase: ProgressPhase::Deleting {
                    num_entries_deleted: self.sent.delete,
                },
            }
        } else {
            // Finished sending deletes, but still sending copies
            // Note that we might have actually finished sending all the copies too, and so we are Done,
            // but we don't return that here otherwise we might end up with two Done markers, which can
            // cause problems.
            ProgressMarker {
                completed_work: self.sent.work,
                phase: ProgressPhase::Copying {
                    num_entries_copied: self.sent.copy,
                    num_bytes_copied: self.sent.copy_bytes,
                }
            }
        }
    }

    /// Increases the sent counters to account for the given entry being deleted.
    pub fn delete_sent(&mut self, e: &EntryDetails) {
        self.sent += ProgressValues::for_delete(e);
    }
    /// Increases the sent counters to account for the given entry being copied.
    pub fn copy_sent(&mut self, e: &EntryDetails) {
        self.sent += ProgressValues::for_copy(e);
    }
    /// Increases the sent counters to account for the given entry being partially copied (a chunk).
    pub fn copy_sent_partial(&mut self, chunk_start: u64, chunk_size: u64, file_size: u64) {
        self.sent += ProgressValues::for_copy_partial(chunk_start, chunk_size, file_size);
    }

    /// Called when all work has been sent to the dest doer.
    /// Returns a ProgressMarker that should be sent to the dest doer to mark this point of progress.
    pub fn all_work_sent(&mut self) -> ProgressMarker {
        debug_assert_eq!(self.total, self.sent);
        ProgressMarker {
            completed_work: self.sent.work,
            phase: ProgressPhase::Done
        }
    }

    /// Called when we have received a Marker from the dest doer indicating that progress has been made.
    /// We update the progress bar to show this progress.
    pub fn update_completed(&mut self, marker: &ProgressMarker) {
        self.completed.work = marker.completed_work;

        match marker.phase {
            ProgressPhase::Deleting { num_entries_deleted } => {
                self.completed.delete = num_entries_deleted;

                // Update the progress bar based on the progress that the dest doer has made.
                self.update_bar_limited();
            }
            ProgressPhase::Copying { num_entries_copied, num_bytes_copied } => {
                // If this is the first progress marker for Copying, then update stat timers as we know
                // we have finished all the deletes and are now about to start the copies
                if self.first_copy_time.is_none() && num_entries_copied == 0 {
                    self.first_copy_time = Some(Instant::now());
                }

                self.completed.copy = num_entries_copied;
                self.completed.copy_bytes = num_bytes_copied;

                // Update the progress bar based on the progress that the dest doer has made.
                self.update_bar_limited();
            }
            ProgressPhase::Done => {
                self.bar.finish_and_clear();
            }
        }
    }

    // Doesn't directly update the bar, because we might do this too quickly and cause too much overhead
    // (see comment on background_updater).
    fn update_bar_limited(&mut self) {
        // Note that we don't format the message string here, because this function will be called a lot
        // and that would be too slow. Instead we format it on the background thread, once we're about to use it.

        // The entry currently being processed is the one after the one we just did, so show that
        // name alongside the progress bar.
        let current_entry =
            if self.first_copy_time.is_none() {
                self.to_delete_paths.get(self.completed.delete as usize).cloned()
            } else {
                self.to_copy_paths.get(self.completed.copy as usize).cloned()
            };

        let new_state = Box::new(BarState {
            is_deleting: self.first_copy_time.is_none(),
            completed: self.completed.clone(),
            total: self.total.clone(),
            current_entry,
        });
        // (static assert) Depending on what type put in the AtomicCell it might use locks, so we choose something that should collapse to a single pointer and thus be lock-free.
        debug_assert!(AtomicCell::<Option<Box<BarState>>>::is_lock_free());
        self.new_bar_state.store(Some(new_state));
    }

    pub fn get_first_copy_time(&self) -> Option<Instant> {
        self.first_copy_time
    }

    /// If we update the progress bar too often then the performance cost is too high.
    /// Even though the ProgressBar is supposed to have some kind of rate limiter/framerate to avoid
    /// this, this wasn't enough, especially when we were calling set_length() a lot which happened
    /// when syncing two identical directories. (After some refactoring though this no longer happens,
    /// so the overhead might now not be a problem any more.)
    /// To avoid this, we run our own background thread (instead of using enable_steady_tick) which
    /// limits calls to any APIs on the ProgressBar.
    fn background_updater(bar: WeakProgressBar, new_bar_state: Arc<AtomicCell<Option<Box<BarState>>>>) {
        loop {
            thread::sleep(Duration::from_secs_f32(1.0 / BAR_UPDATE_RATE));

            // If the main thread has dropped the ProgressBar, or marked it as finished, stop this background thread.
            // Without this, we would keep trying to update it forever.
            let bar = match bar.upgrade() {
                Some(b) => b,
                None => break,
            };
            if bar.is_finished() {
                break
            }

            // Take out the new state put there by the main thread, replacing it with a None.
            // If what we got out was a None, it means that there was no state put there, so nothing for us to do
            // (static assert) Depending on what type we put in the AtomicCell it might use locks, so we choose something that should collapse to a single pointer and thus be lock-free.
            debug_assert!(AtomicCell::<Option<Box<BarState>>>::is_lock_free());
            if let Some(new_state) = new_bar_state.take() {
                let mut message = if new_state.is_deleting {
                    // The doer is deleting entries, and will be some amount behind the boss which may have queued
                    // up many more deletes. Show the progress through these delete operations.
                    // Align values that will change, so that things stay in the same place as we progress
                    format!("Deleting {:>7}/{}",
                        HumanCount(new_state.completed.delete as u64).to_string(),
                        HumanCount(new_state.total.delete as u64).to_string())
                } else {
                    // The doer is now copying entries (i.e. writing them to disk), and will be some amount behind the boss
                    // which may have queued up more copies.
                    // Show the progress through these copy operations, including the number of bytes being copied so that
                    // we can see this increase as large files are copied.
                    // Align values that will change, so that things stay in the same place as we progress
                    // Note the extra whitespace after "Copying" for alignment with "Deleting"
                    format!("Copying  {:>7}/{} {:>11}/{}",
                        HumanCount(new_state.completed.copy as u64).to_string(), HumanCount(new_state.total.copy as u64).to_string(),
                        HumanBytes(new_state.completed.copy_bytes as u64).to_string(), HumanBytes(new_state.total.copy_bytes as u64).to_string())
                };
                if let Some(e) = new_state.current_entry {
                    message += &format!("   {}", e);
                }

                bar.set_length(new_state.total.work);
                bar.set_position(new_state.completed.work);
                bar.set_message(message);
            }
            bar.tick(); // Make the spinner spin, regardless of any other updates
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;

    #[test]
    fn progress_values() {
        // Small files of different sizes still have the same work
        assert_eq!(
            ProgressValues::for_copy(&EntryDetails::File { modified_time: SystemTime::UNIX_EPOCH, size: 1 }).work,
            ProgressValues::for_copy(&EntryDetails::File { modified_time: SystemTime::UNIX_EPOCH, size: 100 }).work
        );

        // But big files scale linearly
        assert_eq!(
            ProgressValues::for_copy(&EntryDetails::File { modified_time: SystemTime::UNIX_EPOCH, size: 10_000_000_000 }).work,
            ProgressValues::for_copy(&EntryDetails::File { modified_time: SystemTime::UNIX_EPOCH, size: 1_000_000_000 }).work * 10
        );

        // Several partial copies add up to the same total as the whole file - small file
        let mut p = ProgressValues::for_copy_partial(0, 100, 1000);
        p += ProgressValues::for_copy_partial(100, 100, 1000);
        p += ProgressValues::for_copy_partial(200, 800, 1000);
        assert_eq!(p,
            ProgressValues::for_copy(&EntryDetails::File { modified_time: SystemTime::UNIX_EPOCH, size: 1000 })
        );

        // Several partial copies add up to the same total as the whole file - large file
        let mut p = ProgressValues::for_copy_partial(0, 100, 1_000_000_000);
        p += ProgressValues::for_copy_partial(100, 100, 1_000_000_000);
        p += ProgressValues::for_copy_partial(200, 800, 1_000_000_000);
        p += ProgressValues::for_copy_partial(1000, 999_999_000, 1_000_000_000);
        assert_eq!(p,
            ProgressValues::for_copy(&EntryDetails::File { modified_time: SystemTime::UNIX_EPOCH, size: 1_000_000_000 })
        );
    }
}