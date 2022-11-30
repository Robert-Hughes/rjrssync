use log::trace;
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::File,
    io::Write,
    sync::Mutex,
    time::{Duration, Instant},
};

use lazy_static::lazy_static;

lazy_static! {
    // Only initialize profiling when the first entry is added.
    static ref PROFILING_START: Instant = Instant::now();

    pub static ref GLOBAL_PROFILING_DATA: Mutex<GlobalProfilingData> = Mutex::new(GlobalProfilingData::new());
}

thread_local! {
    // Each thread will have it's own profiling entry to avoid weird race conditions
    pub static PROFILING_DATA: RefCell<ProfilingData> = RefCell::new(ProfilingData{entries:Vec::with_capacity(1_000_000)});
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
pub struct ProfilingData {
    entries: Vec<ProfilingEntry>,
}

struct LastEntryWithName {
    last: usize,
    name: String,
}

// Store the thread name as well so we can distinguish the threads on the timeline
pub struct GlobalProfilingData {
    entries: Vec<ProfilingEntry>,
    last_entry_with_name: Vec<LastEntryWithName>,
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
            p.borrow_mut().entries.push(ProfilingEntry {
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
        trace!(
            "Dumping profiling data to profiling_data/{}.json",
            thread_name
        );
        std::fs::create_dir_all("profiling_data")
            .expect("Failed to create profiling data directory");
        let file = File::create("profiling_data/".to_string() + &thread_name + ".json").unwrap();
        serde_json::to_writer_pretty(file, self).expect(&format!(
            "Failed to save profiling data for {}",
            thread_name.clone() + ".json"
        ));
        GLOBAL_PROFILING_DATA
            .lock()
            .unwrap()
            .append(self, &thread_name);
    }
}

impl GlobalProfilingData {
    pub fn new() -> Self {
        GlobalProfilingData {
            entries: vec![],
            last_entry_with_name: vec![],
        }
    }

    pub fn append(&mut self, other: &mut ProfilingData, thread_name: &String) -> &mut Self {
        self.entries.append(&mut other.entries);
        let last = self.entries.len();
        self.last_entry_with_name.push(LastEntryWithName {
            last,
            name: thread_name.clone(),
        });
        self
    }

    pub fn dump_profiling_to_chrome(&self, file_name: String) {
        // TODO: Clean this up its pretty disgusting
        assert!(self.entries.len() > 0 && self.last_entry_with_name.len() > 0);
        let mut file = File::create(&file_name).unwrap();
        write!(file, "[").unwrap();

        // Use pid to mark the different threads because why not
        // Keep track of which pid maps to which thread name
        let mut current_pid = 0;
        let mut name_to_pid =
            HashMap::from([(self.last_entry_with_name[0].name.clone(), current_pid)]);
        let mut last_entry_with_name_index = 0;
        for i in 0..self.entries.len() {
            if i >= self.last_entry_with_name[last_entry_with_name_index].last {
                last_entry_with_name_index += 1;
                current_pid += 1;
                if last_entry_with_name_index < self.last_entry_with_name.len() {
                    name_to_pid.insert(
                        self.last_entry_with_name[last_entry_with_name_index]
                            .name
                            .clone(),
                        current_pid,
                    );
                }
            }
            let entry = &self.entries[i];
            let name = &entry.detailed_name;
            let cat = &entry.category_name;
            let pid = current_pid;
            let tid = *map_profiling_to_string()
                .get(&entry.category_name as &str)
                .expect(&format!(
                    "mapping profiling string failed: {}",
                    &entry.category_name
                ));
            let beginning = {
                let ts = entry.start.as_nanos();
                format!(
                    r#"{{"name": "{name}",
                    "cat": "{cat}",
                    "ph": "B",
                    "ts": {ts},
                    "pid": {pid},
                    "tid": {tid},
                    "args": {{
                    }}
                }},"#
                )
            };
            let end = {
                let ts = entry.end.as_nanos();
                format!(
                    r#"{{
                    "name": "{name}",
                    "cat": "{cat}",
                    "ph": "E",
                    "ts": {ts},
                    "pid": {pid},
                    "tid": {tid},
                    "args": {{
                    }}
                }},"#
                )
            };
            file.write_all(&beginning.as_bytes()).expect(&format!(
                "Failed to save beginning converted profiling data {}",
                &name
            ));
            file.write_all(&end.as_bytes()).expect(&format!(
                "Failed to save end converted profiling data for {}",
                &name
            ));
        }
        for i in 0..=current_pid {
            for (k, v) in map_profiling_to_string() {
                write!(
                    file,
                    r#"
                {{
                    "name": "thread_name", "ph": "M", "pid": {i}, "tid": {v},
                    "args": {{
                        "name" : "{k}"
                    }}
                }},"#
                )
                .unwrap();
            }
        }
        for (k, v) in name_to_pid {
            write!(
                file,
                r#"
            {{
                "name": "process_name", "ph": "M", "pid": {v}, "tid": 0,
                "args": {{
                    "name" : "{k}"
                }}
            }},"#
            )
            .unwrap();
        }
        write!(
            file,
            r#"
            {{
                "name": "thread_name", "ph": "M", "pid": 0, "tid": 0,
                "args": {{
                    "name" : "PLEASE UPDATE PROFILING MAP"
                }}
            }}"#
        )
        .unwrap();

        write!(file, "]").unwrap();
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
    GLOBAL_PROFILING_DATA
        .lock()
        .unwrap()
        .append(&mut thread_local_profiling_data, &name.to_string())
        .dump_profiling_to_chrome("profiling_data/".to_string() + "all_trace.json");
}

fn map_profiling_to_string() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("exec_command GetEntries", "1"),
        ("exec_command CreateRootAncestors", "2"),
        ("exec_command GetFileContent", "3"),
        ("exec_command CreateOrUpdateFile", "4"),
        ("exec_command CreateFolder", "5"),
        ("exec_command DeleteFile", "6"),
        ("exec_command DeleteFolder", "7"),
        ("send Serialize", "8"),
        ("send Encrypt", "9"),
        ("send Tcp_Write", "10"),
        ("receive_response", "11"),
    ])
}
