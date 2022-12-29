use std::{path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicUsize, Ordering}}, thread};

use crossbeam::{channel::{Receiver, Sender, SendError}};

use crate::profiling;

/// Similar to WalkDir from the walk_dir crate, this function gets all the files/folders/etc.
/// recursively inside a root directory. 
/// 
/// It runs in parallel across multiple threads to speed it up.
/// From testing, this makes it about 4x faster than a single-threaded approach.
/// 
/// The entries are provided to the caller through a crossbeam Receiver, and iteration is finished
/// when this receiver gets disconnected.
/// 
/// A filter function can be provided to skip some entries, and prevent recursion into unwanted directories.
pub fn parallel_walk_dir<
    T: Send + 'static, 
    F: Fn(&std::fs::DirEntry) -> Result<FilterResult<T>, String> + Send + Clone + 'static
    >(root: &Path, filter_func: F) -> Receiver<Result<Entry<T>, String>> 
{
    // A cross-thread queue of jobs to be executed by the worker threads (a 'job' is simply a directory to enumerate).
    // When encountering a sub-directory, worker threads will add those sub-directories as new jobs to the queue,
    // allowing them to be started in parallel.
    // Note that this channel can't simply be made bounded, because it will lead to deadlocks!
    let (job_sender, job_receiver) = crossbeam::channel::unbounded::<Job>(); 
    // The job queue initially has just one job - the root directory.
    job_sender.send(Job::Dir(PathBuf::from(root))).expect("Job channel disconnected");

    // Counter of the number of jobs that haven't been started, or are in progress.
    // This is used to detect when we are finished, and is slightly different from the job queue's 
    // length, as jobs are removed from the queue before they are finished.
    let num_unfinished_jobs = Arc::new(AtomicUsize::new(1));

    // The cross-thread queue of results, sent to the caller via their Receiver.
    let (result_sender, result_receiver) = crossbeam::channel::bounded::<Result<Entry<T>, String>>(1000);  // Bounded arbitrarily to prevent too high memory usage

    // The "best" number of threads to use depends on many things, and so isn't easily calculable.
    // We spawn this number of threads as the maximum, but scale things down dynamically based on
    // the length of the result queue.
    #[cfg(windows)]
    let num_threads = std::cmp::max(1, num_cpus::get() / 2);
    #[cfg(unix)]
    let num_threads = 1;

    // Spawn worker threads
    for i in 0..num_threads {
        let job_sender = job_sender.clone();
        let job_receiver = job_receiver.clone();
        let result_sender = result_sender.clone();
        let num_unfinished_jobs = num_unfinished_jobs.clone();
        let filter_func = filter_func.clone();
        thread::Builder::new().name(format!("parallel_walk_dir_{}_{i}", root.display())).spawn(move || worker_main(job_sender, job_receiver, result_sender,
            num_unfinished_jobs, num_threads, filter_func)).expect("Failed to spawn thread");
    }

    result_receiver
}

/// A single entry (file, folder etc.) as a result of iterating over a directory.
#[derive(Debug)]
pub struct Entry<T> {
    pub dir_entry: std::fs::DirEntry,
    /// We have to get this anyway, so might as well provide it to avoid the caller having to
    /// deal with the Err variant.
    pub file_type: std::fs::FileType,
    /// Additional data provided as a result from the filter function, to prevent the user
    /// having to re-calculate it.
    pub additional_data: T,
}

/// Result of checking an entry against a filter.
/// As well as determinining whether or not the entry should be skipped, this allows
/// additional data to be provided with the entry, to avoid having to re-calculate 
/// anything needed for the filtering process.
pub struct FilterResult<T> {
    pub skip: bool,
    pub additional_data: T
}

enum Job {
    Dir(PathBuf),
    Done
}

fn worker_main<T, F: Fn(&std::fs::DirEntry) -> Result<FilterResult<T>, String>>(
    job_sender: Sender<Job>, job_receiver: Receiver<Job>,
    result_sender: Sender<Result<Entry<T>, String>>, num_unfinished_jobs: Arc<AtomicUsize>,
    num_threads: usize, filter_func: F) 
    -> 
    Result<(), SendError<Result<Entry<T>, String>>>
{
    // Note that errors from sending to the _result_ channel are ignored and we silently stop this thread,
    // because this simply indicates that the user has dropped their receiver and so don't care about any 
    // more entries. We use the '?' for this to keep it concise.
    // The job channel however should always be alive as this thread holds both a Sender and Receiver to it.

    loop {
        // Get the next job from the queue, blocking until one is available
        let job = job_receiver.recv().expect("Job channel disconnected");

        match job {
            Job::Dir(dir) => {
                let timer = profiling::start_timer("read_dir");
                let iter = match std::fs::read_dir(&dir) {
                    Ok(x) => x,
                    Err(e) => {
                        result_sender.send(Err(format!("Error reading dir '{}': {e}", dir.display())))?;
                        continue;
                    }
                };
                profiling::stop_timer(timer);

                for entry in iter {
                    let entry = match entry {
                        Ok(x) => x,
                        Err(e) => {
                            result_sender.send(Err(format!("Error iterating dir '{}': {e}", dir.display())))?;
                            continue;
                        }
                    };

                    // Check if this entry should be filtered.
                    // Filtering a folder prevents iterating into child files/folders, so this is efficient.
                    let timer = profiling::start_timer("filter_func");
                    let additional_data = match filter_func(&entry) {
                        Ok(f) => {
                            if f.skip {
                                continue;
                            }
                            f.additional_data
                        },
                        Err(e) => {
                            result_sender.send(Err(format!("Error applying filter to '{}': {e}", entry.path().display())))?;
                            continue;
                        }
                    };
                    profiling::stop_timer(timer);
                    
                    // Before sending the entry as a result, check if it's a directory that we need to recurse into
                    let timer = profiling::start_timer(&format!("send result ({})", result_sender.len()));
                    let file_type = match entry.file_type() {
                        Ok(x) => x,
                        Err(e) => {
                            result_sender.send(Err(format!("Error checking file type of'{}': {e}", entry.path().display())))?;
                            continue;
                        }
                    };

                    let child_dir_to_recurse = if file_type.is_dir() {
                        Some(entry.path())
                    } else {
                        None
                    };

                    result_sender.send(Ok(Entry {
                        dir_entry: entry,
                        file_type,
                        additional_data,
                    }))?;
                    profiling::stop_timer(timer);

                    let timer = profiling::start_timer("recurse");
                    // Recurse into child directories by adding a job that other threads could pick up.
                    // Note that it's important that we do this _after_ sending the entry as a result, so that
                    // the children of this folder are always after the folder itself in the results.
                    if let Some(x) = child_dir_to_recurse {
                        num_unfinished_jobs.fetch_add(1, Ordering::SeqCst);
                        job_sender.send(Job::Dir(x)).expect("Job channel disconnected");
                    }
                    profiling::stop_timer(timer);
                }                        
            }
            Job::Done => break,
        }

        // Check if the job we just finished was the last job that needed doing, and thus we are finished.
        // In this case, wake all the other threads up and tell them to quit.
        let prev_count = num_unfinished_jobs.fetch_sub(1, Ordering::SeqCst);
        if prev_count == 1 {
            assert_eq!(job_sender.len(), 0); // Sanity test - the queue length should always be <= num_unfinished_jobs
            for _ in 0..num_threads {
                job_sender.send(Job::Done).expect("Job channel disconnected");
            }
        }
    }

    Ok(())
}