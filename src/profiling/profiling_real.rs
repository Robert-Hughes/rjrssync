use log::trace;
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::File,
    sync::Mutex,
    time::{Duration, Instant},
};

use lazy_static::lazy_static;

lazy_static! {
    // Only initialize profiling when the first entry is added.
    static ref PROFILING_START: Instant = Instant::now();

    static ref GLOBAL_PROFILING_DATA: Mutex<GlobalProfilingData> = Mutex::new(GlobalProfilingData::new());
}

thread_local! {
    // Each thread will have it's own profiling entry to avoid weird race conditions
    static PROFILING_DATA: RefCell<ProfilingData> = RefCell::new(ProfilingData{entries: Some(Vec::with_capacity(1_000_000))});
}

#[macro_export]
macro_rules! function_name {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        &name[..name.len() - 3].split("::").last().unwrap()
    }};
}

#[macro_export]
macro_rules! profile_this {
    () => {
        let _profiling_keep_alive = crate::profiling::profiling_real::Timer::new(
            crate::function_name!().to_string(),
            "".to_string(),
        );
    };
    ($mand_1:expr) => {
        let _profiling_keep_alive = crate::profiling::profiling_real::Timer::new(
            crate::function_name!().to_string() + " " + $mand_1,
            "".to_string(),
        );
    };
    ($mand_1:expr, $mand_2:expr) => {
        let _profiling_keep_alive = crate::profiling::profiling_real::Timer::new(
            crate::function_name!().to_string() + " " + $mand_1,
            $mand_2,
        );
    };
}

#[derive(Serialize, Clone)]
struct ProfilingEntry {
    category_name: String,
    detailed_name: String,
    // start and end are durations since the start of profiling because Instant cannot be serialized by default.
    start: Duration,
    end: Duration,
    // duration could just be calculated offline, for now keep it here as it's sometimes useful.
    duration: Duration,
}

#[derive(Serialize, Clone)]
struct ProfilingData {
    entries: Option<Vec<ProfilingEntry>>,
}

struct LocalProfilingEntry {
    local_profiling_entry: Vec<ProfilingEntry>,
    name: String,
}

// Store the thread name as well so we can distinguish the threads on the timeline
struct GlobalProfilingData {
    entries: Vec<LocalProfilingEntry>,
}

pub struct Timer {
    // Make name an Option so we can move out of it in the drop later.
    category_name: Option<String>,
    detailed_name: Option<String>,
    start: Duration,
}

impl Timer {
    pub fn new(category_name: String, detailed_name: String) -> Timer {
        let start = PROFILING_START.elapsed();
        Timer {
            category_name: Some(category_name),
            detailed_name: Some(detailed_name),
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
                .as_mut()
                .unwrap()
                .push(ProfilingEntry {
                    category_name: self.category_name.take().unwrap(),
                    detailed_name: self.detailed_name.take().unwrap(),
                    start: self.start,
                    end,
                    duration: end - self.start,
                });
        });
    }
}

impl Drop for ProfilingData {
    fn drop(&mut self) {
        let thread_name = std::thread::current().name().unwrap().to_string();
        trace!("Copying local profiling data to global");
        if let Some(entries) = self.entries.take() {
            let entry = LocalProfilingEntry {
                local_profiling_entry: entries,
                name: thread_name,
            };
            GLOBAL_PROFILING_DATA.lock().unwrap().push_events(entry);
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

        // Use pid to mark the different threads because why not
        // Keep track of which pid maps to which thread name
        let mut name_to_pid = HashMap::from([(self.entries[0].name.clone(), 0)]);
        for i in 0..self.entries.len() {
            let thread_events = &self.entries[i].local_profiling_entry;
            name_to_pid.insert(self.entries[i].name.clone(), i);
            for entry in thread_events {
                let name = entry.detailed_name.clone();
                let cat = entry.category_name.clone();
                let pid = i;
                let tid = *map_profiling_to_string()
                    .get(&entry.category_name as &str)
                    .expect(&format!(
                        "mapping profiling string failed: {}",
                        &entry.category_name
                    ));
                let ts = entry.start.as_nanos();
                let beginning = ChromeTracing {
                    name,
                    cat,
                    ph: "B",
                    ts,
                    pid,
                    tid,
                    args: HashMap::new(),
                };
                json_entries.push(beginning);
                let end_ts = entry.end.as_nanos();
                let name = entry.detailed_name.clone();
                let cat = entry.category_name.clone();
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
        for i in 0..=self.entries.len() {
            for (k, v) in map_profiling_to_string() {
                let entry = ChromeTracing {
                    name: "thread_name".to_string(),
                    cat: "None".to_string(),
                    ph: "M",
                    ts: 0,
                    pid: i,
                    tid: v,
                    args: HashMap::from([("name".to_string(), k.into())]),
                };
                json_entries.push(entry);
            }
        }
        for (k, v) in name_to_pid {
            let entry = ChromeTracing {
                name: "process_name".to_string(),
                cat: "None".to_string(),
                ph: "M",
                ts: 0,
                pid: v,
                tid: 0,
                args: HashMap::from([("name".to_string(), k.into())]),
            };
            json_entries.push(entry);
        }
        serde_json::to_writer_pretty(&file, &json_entries)
            .expect(&format!("Failed to save end converted profiling data"));
    }
}

// Only to be called by main.
// TODO: Assert for this somehow?
pub fn dump_all_profiling() {
    // Create the profiling_data directory again if somehow no other threads are launched on this side
    // Maybe some case where the doers are both on remotes?
    std::fs::create_dir_all("profiling_data").expect("Failed to create profiling data directory");
    // As main is the only thread to not be joined (and thus the ProfilingData dropped)
    // it must be append to the global profiling manually.
    let name = std::thread::current().name().unwrap().to_string();
    let mut thread_local_profiling_data = PROFILING_DATA.with(|p| p.clone()).into_inner();
    let main_thread_events = LocalProfilingEntry {
        local_profiling_entry: thread_local_profiling_data.entries.take().unwrap(),
        name,
    };
    GLOBAL_PROFILING_DATA
        .lock()
        .unwrap()
        .push_events(main_thread_events)
        .dump_profiling_to_chrome("profiling_data/".to_string() + "all_trace.json");
}

fn map_profiling_to_string() -> HashMap<&'static str, usize> {
    HashMap::from([
        ("exec_command GetEntries", 1),
        ("exec_command CreateRootAncestors", 2),
        ("exec_command GetFileContent", 3),
        ("exec_command CreateOrUpdateFile", 4),
        ("exec_command CreateFolder", 5),
        ("exec_command DeleteFile", 6),
        ("exec_command DeleteFolder", 7),
        ("send Serialize", 8),
        ("send Encrypt", 9),
        ("send Tcp Write", 10),
        ("receive_response", 11),
        ("send", 12),
    ])
}
