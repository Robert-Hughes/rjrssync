use std::{ops::{AddAssign, SubAssign}, time::{Instant, Duration}, thread, sync::{Arc}};

use crossbeam::atomic::AtomicCell;
use indicatif::{ProgressBar, HumanCount, HumanBytes, ProgressStyle, WeakProgressBar};

use crate::{doer::{EntryDetails, ProgressPhase, ProgressMarker}};

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
                    work: std::cmp::max(*size, 1024*1024), // Even small files will take some minimum amount of time to copy
                    copy: 1,
                    copy_bytes: *size,
                    ..Default::default()
                }
            },
            EntryDetails::Folder | EntryDetails::Symlink{..} => ProgressValues { 
                work: 1024*1024,
                copy: 1,
                ..Default::default()
            }
        }
    }

    /// Creates a set of ProgressValues to represent the deletion of a single entry.
    fn for_delete(_e: &EntryDetails) -> Self {
        ProgressValues { 
            work: 512*1024,
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
#[derive(Default)]
struct BarState {
    is_deleting: bool,
    completed: ProgressValues,
    total: ProgressValues,
}

/// Wrapper around progress-bar related logic.
/// 
/// Progress is a bit tricky because we initially don't know how much stuff needs deleting/copying,
/// and we only figure this out as the sync progresses. Because the destination doer is asynchronous,
/// just because the boss has sent a command to (e.g.) write a file, that command won't be completed
/// until some time in the future, so we can't advance the progress bar until that point. 
/// We also want the progress bar to move smoothly across different kinds of operation (deletes, 
/// copies of small files, copies of large files), all of which might take different amounts of time to complete.
/// 
/// The solution works like this. During the querying phase of the sync, we simply show "Querying...",
/// as we have no good indication of progress here. During this time we sum up a total of the amount of stuff
/// that might need deleting or copying. Initially we assume that this is everything (everything on the dest
/// will need deleting and everything on the source will need copying). This is pessimistic and gives an upper
/// bound of the amount of work that needs doing. Once the boss starts looking through the entries it will decide
/// what needs deleting and what doesn't. When it decides something doesn't need deleting we reduce our total work
/// accordingly. When it decides something does needs deleting it will send a command to the dest doer, and also
/// update a counter here of the amount of work sent to the doer. It will also (at some limited interval) send "progress marker"
/// commands to the dest doer, so that when the doer gets to that command it will echo it back to the boss to show us
/// how far the doer has got. When we receive that echoed marker back, we update a counter here with the amount of 
/// work completed. The progress bar shows the amount completed vs. the total.
/// 
/// There's an additional requirement to keep the overhead of updating the progress bar small, especially
/// for very fast syncs where nothing has changed. Because we will be updating the totals a lot for this case
/// (summing up all the potential work, then reducing it back down to zero), we can't afford to update the progress
/// bar so frequently. Instead, we run a background thread which updates it periodically.
pub struct Progress {
    /// The UI element from the `indicatif` crate that handles drawing the progress bar.
    bar: ProgressBar,

    /// Keeps track of the total amount of work that the dest doer needs
    /// to complete. Initially we won't have an accurate value for this, because we won't have checked which files are 
    /// up-to-date etc. so this will be adjusted as we go so that the progress bar can be more accurate.
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
}
impl Progress {
    pub fn new() -> Self {
        let bar = ProgressBar::new_spinner().with_message("Querying...");
        let new_bar_state = Arc::new(AtomicCell::new(None));

        let bar2 = bar.downgrade(); // Weak reference for the background thread
        let new_bar_state2 = new_bar_state.clone();
        thread::Builder::new().name("Progress bar".to_string()).spawn(
            move || Self::background_updater(bar2, new_bar_state2)).expect("Failed to spawn thread");

        Progress {
            bar,
            total: ProgressValues::default(),
            sent: ProgressValues::default(),
            completed: ProgressValues::default(),
            new_bar_state,
            last_progress_marker: 0,
            first_copy_time: None,
        }
    }

    /// Forwards to ProgressBar::suspend(). We avoid exposing the ProgressBar directly so that
    /// we can be the sole controller.
    pub fn suspend<F: FnOnce() -> R, R>(&self, f: F) -> R {
        self.bar.suspend(f)
    }

    /// Increases the totals to account for the given entry being deleted.
    pub fn inc_total_for_delete(&mut self, e: &EntryDetails) {
        self.total += ProgressValues::for_delete(e);
        // We don't need to update the bar length here (like we do in dec_total_for_delete)
        // because this function is only called during querying, at which point we haven't 
        // shown the proper progress bar yet.
    }
    /// Decreases the totals to account for the given entry not needing to be deleted.
    pub fn dec_total_for_delete(&mut self, e: &EntryDetails) {
        self.total -= ProgressValues::for_delete(e);

        // Update the bar length, to show that there is less work to be done.
        // We don't update it directly because this can lead to poor performance when do it
        // a lot (see comment on background_updater).
        self.update_bar_limited();
    }

    /// Increases the totals to account for the given entry being copied.
    pub fn inc_total_for_copy(&mut self, e: &EntryDetails) {
        self.total += ProgressValues::for_copy(e);

        // We don't need to update the bar length here (like we do in dec_total_for_copy)
        // because this function is only called during querying, at which point we haven't 
        // shown the proper progress bar yet.
    }
    /// Decreases the totals to account for the given entry not needing to be copied.
    pub fn dec_total_for_copy(&mut self, e: &EntryDetails) {
        self.total -= ProgressValues::for_copy(e);
 
        // Update the bar length, to show that there is less work to be done.
        // We don't update it directly because this can lead to poor performance when do it
        // a lot (see comment on background_updater).
        self.update_bar_limited();
    }

    /// Gets a ProgressMarker to be sent to the dest doer to mark the amount of work
    /// that has been already sent.
    pub fn get_progress_marker(&mut self) -> ProgressMarker {
        // Remember when we last sent a marker, so that we don't do it too often
        self.last_progress_marker = self.sent.work;

        // It's safe to compare sent vs total like this, because the total starts high and gets reduced,
        // and the sent starts at zero and gets increased. So they will only be equal once fully sent.
        debug_assert!(self.sent.delete <= self.total.delete);
        debug_assert!(self.sent.copy <= self.total.copy);
        if self.sent.delete < self.total.delete {
            // Still sending deletes
            ProgressMarker { 
                completed_work: self.sent.work,
                phase: ProgressPhase::Deleting { num_entries_deleted: self.sent.delete }
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
                    num_bytes_copied: self.sent.copy_bytes 
                }
            }
        }
    }

    /// Increases the sent counters to account for the given entry being deleted.
    /// Returns a ProgressMarker that should be sent to the dest doer to mark this point of progress.
    pub fn delete_sent(&mut self, e: &EntryDetails) -> Option<ProgressMarker> {
        self.sent += ProgressValues::for_delete(e);
        // Don't send progress markers too often, to avoid overhead
        if self.sent.work - self.last_progress_marker > 1024*1024 {
            Some(self.get_progress_marker())
        } else {
            None
        }       
    }
    /// Increases the sent counters to account for the given entry being copied.
    /// Returns a ProgressMarker that should be sent to the dest doer to mark this point of progress.
    pub fn copy_sent(&mut self, e: &EntryDetails) -> Option<ProgressMarker> {
        self.sent += ProgressValues::for_copy(e);
        // Don't send progress markers too often, to avoid overhead
        if self.sent.work - self.last_progress_marker > 1024*1024 {
            Some(self.get_progress_marker())
        } else {
            None
        }
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
                // If this is the first progress marker for deleting, then reset from its Querying... state:
                if num_entries_deleted == 0 {
                    // We don't yet know how many entries need deleting/copying, so can't draw an accurate progress bar.
                    // Start the progress bar initially with an upper bound assuming that everything needs deleting and everything
                    // needs copying.
                    // Note that we don't render the pos or length in the template, as the 'work' values are pretty meaningless
                    // for the user. Instead we show the percentage, and include a custom message where we print more details
                    self.bar.reset();
                    self.bar.set_length(self.total.work);
                    self.bar.set_style(ProgressStyle::with_template("{percent}% {bar:40.green/black} {msg}").unwrap());
                }

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
        let new_state = Box::new(BarState {
            is_deleting: self.first_copy_time.is_none(),
            completed: self.completed.clone(),
            total: self.total.clone(),
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
    /// this, it doesn't seem to be enough, especially when calling set_length() a lot which happens
    /// when syncing two identical directories (we call dec_total_for_copy a lot very quickly).
    /// To avoid this, we run our own background thread (instead of using enable_steady_tick) which
    /// limits calls to any APIs on the ProgressBar.
    fn background_updater(bar: WeakProgressBar, new_bar_state: Arc<AtomicCell<Option<Box<BarState>>>>) {
        loop {
            thread::sleep(Duration::from_millis(100));

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
                let message = if new_state.is_deleting {
                    // The doer is deleting entries, and will be some amount behind the boss which may have queued
                    // up many more deletes. Show the progress through these delete operations.
                    format!("Deleting {:>7}/{:>7}", 
                        HumanCount(new_state.completed.delete as u64).to_string(),
                        HumanCount(new_state.total.delete as u64).to_string())
                } else {
                    // The doer is now copying entries (i.e. writing them to disk), and will be some amount behind the boss 
                    // which may have queued up more copies.
                    // Show the progress through these copy operations, including the number of bytes being copied so that
                    // we can see this increase as large files are copied.
                    // Note the extra whitespace after "Copying" for alignment with "Deleting"
                    format!("Copying  {:>7}/{:>7} {:>11}/{:>11}", 
                        HumanCount(new_state.completed.copy as u64).to_string(), HumanCount(new_state.total.copy as u64).to_string(),
                        HumanBytes(new_state.completed.copy_bytes as u64).to_string(), HumanBytes(new_state.total.copy_bytes as u64).to_string())
                };

                bar.set_length(new_state.total.work);
                bar.set_position(new_state.completed.work);
                bar.set_message(message);
            }
            bar.tick(); // Make the spinner spin, regardless of any other updates
        }
    }
}
