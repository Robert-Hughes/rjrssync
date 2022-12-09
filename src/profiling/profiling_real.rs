use log::trace;
use serde::{Serialize, Deserialize};
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::File,
    sync::Mutex,
    time::{Duration, Instant},
};

use lazy_static::{lazy_static};

lazy_static! {
    // Only initialize profiling when the first entry is added.
    static ref PROFILING_START: Instant = Instant::now();

    static ref GLOBAL_PROFILING_DATA: Mutex<Option<GlobalProfilingData>> = Mutex::new(Some(GlobalProfilingData::new()));
}

thread_local! {
    // Each thread will have it's own profiling entry to avoid weird race conditions
    static PROFILING_DATA: RefCell<ProfilingData> = RefCell::new(ProfilingData{entries: Some(Vec::with_capacity(1_000_000))});
}

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

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ProfilingEntry {
    scope_name: String,
    // start and end are durations since the start of profiling because Instant cannot be serialized by default.
    start: Duration,
    end: Duration,
    // duration could just be calculated offline, for now keep it here as it's sometimes useful.
    duration: Duration,
}

#[derive(Serialize, Clone, Default)]
struct ProfilingData {
    entries: Option<Vec<ProfilingEntry>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalProfilingEntry {
    local_profiling_entry: Vec<ProfilingEntry>,
    thread_name: String,
    process_name: String
}

// Store the thread name as well so we can distinguish the threads on the timeline
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GlobalProfilingData {
    entries: Vec<LocalProfilingEntry>,
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
            p.borrow_mut()
                .entries
                .as_mut().map(|x| x.push(ProfilingEntry {
                    scope_name: self.scope_name.take().unwrap(),
                    start: self.start,
                    end,
                    duration: end - self.start,
                }));
        });
    }
}

impl Drop for ProfilingData {
    fn drop(&mut self) {
        let thread_name = std::thread::current().name().unwrap().to_string();
        trace!("Copying local profiling data to global for {}", thread_name);
        if let Some(entries) = self.entries.take() {
            let entry = LocalProfilingEntry {
                local_profiling_entry: entries,
                thread_name,
                process_name: "Boss".to_string()
            };
            GLOBAL_PROFILING_DATA.lock().unwrap().as_mut().unwrap().push_events(entry);
        }
    }
}

#[derive(Serialize)]
struct ChromeTracing {
    name: String,
    cat: String,
    ph: &'static str,
    ts: u128,
    pid: usize,
    tid: usize,
    args: HashMap<String, serde_json::Value>,
}

impl GlobalProfilingData {
    fn new() -> Self {
        GlobalProfilingData { entries: vec![] }
    }

    fn push_events(&mut self, local_events: LocalProfilingEntry) -> &mut Self {
        self.entries.push(local_events);
        self
    }

    fn dump_profiling_to_chrome(&self, file_name: String) {
        // TODO: Clean this up its pretty disgusting
        assert!(self.entries.len() > 0);
        let file = File::create(&file_name).unwrap();

        let mut json_entries = vec![];

        // Keep track of which tid maps to which thread name
        let mut name_to_tid = HashMap::new();
        // Keep track of the first timestamp for each thread, so we can sort them by this later
        let mut thread_name_to_first_event = vec![];
        let get_tid_for_thread_name = |name_to_tid: &mut HashMap<_, _>, thread_name| {
            let new_tid = name_to_tid.len();
            *name_to_tid.entry(thread_name).or_insert(new_tid)
        };

        for i in 0..self.entries.len() {
            let thread_events = &self.entries[i].local_profiling_entry;

            thread_name_to_first_event.push((self.entries[i].thread_name.clone(), thread_events[0].start));

            for entry in thread_events {
                let name = entry.scope_name.clone();
                let cat = "None".to_string();
                let pid = 0;
                let tid = get_tid_for_thread_name(&mut name_to_tid, self.entries[i].thread_name.clone());
                let ts = entry.start.as_nanos();
                let beginning = ChromeTracing {
                    name,
                    cat: cat.clone(),
                    ph: "B",
                    ts,
                    pid,
                    tid,
                    args: HashMap::new(),
                };
                json_entries.push(beginning);
                let end_ts = entry.end.as_nanos();
                let name = entry.scope_name.clone();
                let end = ChromeTracing {
                    name,
                    cat,
                    ph: "E",
                    ts: end_ts,
                    pid,
                    tid,
                    args: HashMap::new(),
                };
                json_entries.push(end);
            }
        }

        // Rename threads so that they are sorted in a sensible order in Chrome
        thread_name_to_first_event.sort_by_key(|x| x.1);
        let keys : Vec<String> = name_to_tid.keys().map(|t| (*t).clone()).collect();
        for thread_name in keys {
            let idx = thread_name_to_first_event.iter().position(|x| x.0 == thread_name).unwrap();
            let new_thread_name = format!("{idx} {thread_name}");
            let v = name_to_tid.remove(&thread_name).unwrap();
            name_to_tid.insert(new_thread_name, v);
        }

        // Add metadata to define thread names
        for (thread_name, tid) in &name_to_tid {
            let entry = ChromeTracing {
                name: "thread_name".to_string(),
                cat: "None".to_string(),
                ph: "M",
                ts: 0,
                pid: 0,
                tid: *tid,
                args: HashMap::from([("name".to_string(), serde_json::Value::String(thread_name.to_string()))]),
            };
            json_entries.push(entry);
        }
        let entry = ChromeTracing {
            name: "process_name".to_string(),
            cat: "None".to_string(),
            ph: "M",
            ts: 0,
            pid: 0,
            tid: 0,
            args: HashMap::from([("name".to_string(), "rjrssync".into())]),
        };
        json_entries.push(entry);

        serde_json::to_writer(&file, &json_entries)
            .expect(&format!("Failed to save end converted profiling data"));
    }
}

// Only to be called by main.
// TODO: Assert for this somehow?
pub fn dump_all_profiling() {
    // Create the profiling_data directory again if somehow no other threads are launched on this side
    // Maybe some case where the doers are both on remotes?
    std::fs::create_dir_all("profiling_data").expect("Failed to create profiling data directory");
    let profiling_data = get_all_profiling();
    profiling_data.dump_profiling_to_chrome("profiling_data/".to_string() + "all_trace.json");
}

pub fn get_all_profiling() -> GlobalProfilingData {
    trace!("get_all_profiling");
    // As main is the only thread to not be joined (and thus the ProfilingData dropped)
    // we drop it manually here
    PROFILING_DATA.with(|p| p.take());
    GLOBAL_PROFILING_DATA
        .lock()
        .unwrap()
        .take()
        .unwrap()
}

pub fn add_remote_profiling(profiling_data: GlobalProfilingData, process_name: String) {
    trace!("add_remote_profiling");
    let mut g = GLOBAL_PROFILING_DATA.lock().unwrap();
    for mut data in profiling_data.entries{
        data.thread_name = process_name.clone();
        g.as_mut().unwrap().push_events(data);
    }
}