use json::JsonValue;
use log::{trace, info};
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::File,
    sync::Mutex,
    time::{Duration}, ops::DerefMut, io::{Write},
};

use lazy_static::{lazy_static};
use super::*;

lazy_static! {
    static ref GLOBAL_PROFILING_DATA: Mutex<GlobalProfilingData> = Mutex::new(GlobalProfilingData::default());
}

thread_local! {
    // Each thread will have it's own vec of profiling entries to avoid weird race conditions
    static PROFILING_DATA: RefCell<ThreadRecorder> = RefCell::new(
        ThreadRecorder {
            thread_name: std::thread::current().name().expect("Thread has no name").to_string(),
            entries: Vec::with_capacity(1_000_000) });
}

const LOCAL_PROCESS_NAME: &str = "<Local>";

#[macro_export]
macro_rules! profile_this {
    () => {
        let _profiling_keep_alive = crate::profiling::profiling_real::Timer::new(
            crate::function_name!().to_string(),
        );
    };
    ($mand_1:expr) => {
        let _profiling_keep_alive = crate::profiling::profiling_real::Timer::new(
            $mand_1.into(),
        );
    };
}

#[derive(Default)]
struct ThreadRecorder {
    thread_name: String, // Querying this in drop() crashes (badly) on MUSL builds, so store it on creation here instead
    entries: Vec<ProfilingEntry>,
}
impl Drop for ThreadRecorder {
    fn drop(&mut self) {
        trace!("Moving thread profiling data to global for thread '{}'", self.thread_name);
        GLOBAL_PROFILING_DATA.lock().expect("Locking error").processes.entry(LOCAL_PROCESS_NAME.to_string())
            .or_default().threads.insert(self.thread_name.clone(), ThreadProfilingData {
                entries: std::mem::take(&mut self.entries)
            });
    }
}


#[derive(Default)]
pub struct GlobalProfilingData {
    processes: HashMap<String, ProcessProfilingData>,
}

pub struct Timer {
    // Make name an Option so we can move out of it in the drop later.
    scope_name: Option<String>,
    start: Duration,
}

pub fn start_timer(name: &str) -> Timer {
    Timer::new(name.to_string())
}

pub fn stop_timer(_t: Timer) {} // This will drop the Timer and thus call Timer::drop() which stores the event.

impl Timer {
    pub fn new(scope_name: String) -> Timer {
        let start = PROFILING_START.elapsed();
        Timer {
            scope_name: Some(scope_name),
            start,
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        let end = PROFILING_START.elapsed();
        PROFILING_DATA.with(|p| {
            p.borrow_mut().entries.push(ProfilingEntry {
                scope_name: self.scope_name.take().unwrap(),
                start: self.start,
                end,
                duration: end - self.start,
            });
        });
    }
}

struct ChromeTracing {
    name: String,
    cat: String,
    ph: &'static str,
    ts: u128,
    pid: usize,
    tid: usize,
    args: JsonValue,
}

impl GlobalProfilingData {
    fn dump_profiling_to_chrome(&self, file_name: String) {
        let mut file = File::create(&file_name).unwrap();

        let mut json_entries = vec![];

        // Keep track of which pid maps to which process name
        let mut name_to_pid = HashMap::new();
        let get_pid_for_process_name = |name_to_pid: &mut HashMap<_, _>, process_name| {
            let new_pid = name_to_pid.len();
            *name_to_pid.entry(process_name).or_insert(new_pid)
        };
        // Keep track of the first timestamp for each process, so we can sort them by this later
        let mut process_name_to_first_event = vec![];

        for (process_name, process_profiling_data) in &self.processes {
            let pid = get_pid_for_process_name(&mut name_to_pid, process_name.clone());

            // Keep track of which tid maps to which thread name
            let mut name_to_tid = HashMap::new();
            let get_tid_for_thread_name = |name_to_tid: &mut HashMap<_, _>, thread_name| {
                let new_tid = name_to_tid.len();
                *name_to_tid.entry(thread_name).or_insert(new_tid)
            };
            // Keep track of the first timestamp for each thread, so we can sort them by this later
            let mut thread_name_to_first_event = vec![];

            for (thread_name, thread_profiling_data) in &process_profiling_data.threads {

                thread_name_to_first_event.push((thread_name.clone(), thread_profiling_data.entries.last().unwrap().start
                    + process_profiling_data.timestamp_offset));

                for entry in &thread_profiling_data.entries {
                    let name = entry.scope_name.clone();
                    let cat = "None".to_string();
                    let tid = get_tid_for_thread_name(&mut name_to_tid, thread_name.clone());
                    let ts = (entry.start + process_profiling_data.timestamp_offset).as_nanos();
                    let beginning = ChromeTracing {
                        name,
                        cat: cat.clone(),
                        ph: "B",
                        ts,
                        pid,
                        tid,
                        args: json::object!{},
                    };
                    json_entries.push(beginning);
                    let end_ts = (entry.end + process_profiling_data.timestamp_offset).as_nanos();
                    let name = entry.scope_name.clone();
                    let end = ChromeTracing {
                        name,
                        cat,
                        ph: "E",
                        ts: end_ts,
                        pid,
                        tid,
                        args: json::object!{},
                    };
                    json_entries.push(end);
                }
            }

            // Rename threads so that they are sorted in a sensible order in Chrome
            thread_name_to_first_event.sort_by_key(|x| x.1);
            let keys : Vec<String> = name_to_tid.keys().map(|t| (*t).clone()).collect();
            for thread_name in keys {
                let idx = thread_name_to_first_event.iter().position(|x| x.0 == thread_name).unwrap();
                let new_thread_name = format!("{idx:0>2}) {thread_name}");
                let v = name_to_tid.remove(&thread_name).unwrap();
                name_to_tid.insert(new_thread_name, v);
            }

            process_name_to_first_event.push((process_name.clone(), thread_name_to_first_event[0].1));

            // Add metadata to define thread names
            for (thread_name, tid) in &name_to_tid {
                let entry = ChromeTracing {
                    name: "thread_name".to_string(),
                    cat: "None".to_string(),
                    ph: "M",
                    ts: 0,
                    pid,
                    tid: *tid,
                    args: json::object!{ name: thread_name.to_string() },
                };
                json_entries.push(entry);
            }
        }

        // Rename processes so that they are sorted in a sensible order in Chrome
        process_name_to_first_event.sort_by_key(|x| x.1);
        let keys : Vec<String> = name_to_pid.keys().map(|t| (*t).clone()).collect();
        for process_name in keys {
            let idx = process_name_to_first_event.iter().position(|x| x.0 == process_name).unwrap();
            let mut new_process_name = format!("{idx}) {process_name}");
            if process_name == LOCAL_PROCESS_NAME {
                new_process_name = "0) Boss".to_string();
            }
            let v = name_to_pid.remove(&process_name).unwrap();
            name_to_pid.insert(new_process_name, v);
        }

        // Add metadata to define process names
        for (process_name, pid) in &name_to_pid {
            let entry = ChromeTracing {
                name: "process_name".to_string(),
                cat: "None".to_string(),
                ph: "M",
                ts: 0,
                pid: *pid,
                tid: 0,
                args: json::object!{ name: process_name.to_string() },
            };
            json_entries.push(entry);
        }

        // Manual string formatting is a lot quicker than using serde_json or similar.
        // We do need to handle escaping some values though
        write!(file, "[").expect("Failed to write profiling data");
        for e in json_entries {
            write!(file, r#"{{"name":{},"cat":"{}","ph":"{}","ts":{},"pid":{},"tid":{},"args":{}}},"#,
                JsonValue::String(e.name).dump(), e.cat, e.ph, e.ts, e.pid, e.tid, e.args.dump(),
            ).expect("Failed to write profiling data");
        }
        write!(file, "\"dummy\" ]").expect("Failed to write profiling data");
    }
}

// Only to be called by main.
pub fn dump_all_profiling() {
    // Create the profiling_data directory again if somehow no other threads are launched on this side
    // Maybe some case where the doers are both on remotes?
    std::fs::create_dir_all("profiling_data").expect("Failed to create profiling data directory");
    let profiling_data = get_all_profiling();
    let output_path = "profiling_data/".to_string() + "all_trace.json";
    info!("Dumping profiling data ({} processes, {} threads, {} entries) to {}",
        profiling_data.processes.len(),
        profiling_data.processes.values().map(|p| p.threads.len()).sum::<usize>(),
        profiling_data.processes.values().map(|p| p.threads.values().map(|t| t.entries.len()).sum::<usize>()).sum::<usize>(),
        output_path);
    profiling_data.dump_profiling_to_chrome(output_path);
}

fn get_all_profiling() -> GlobalProfilingData {
    trace!("get_all_profiling");
    assert_eq!(std::thread::current().name(), Some("main"));
    // As main is the only thread to not be joined (and thus the ProfilingData dropped)
    // we drop it manually here, and it will be added to the global data
    PROFILING_DATA.with(|p| p.take());
    std::mem::take(GLOBAL_PROFILING_DATA.lock().unwrap().deref_mut())
}

pub fn get_local_process_profiling() -> ProcessProfilingData {
    std::mem::take(get_all_profiling().processes.get_mut(LOCAL_PROCESS_NAME).unwrap())
}

pub fn add_remote_profiling(mut remote_profiling_data: ProcessProfilingData, process_name: String, offset: Duration) {
    trace!("add_remote_profiling");
    remote_profiling_data.timestamp_offset = offset;
    GLOBAL_PROFILING_DATA.lock().unwrap().processes.insert(process_name, remote_profiling_data);
}