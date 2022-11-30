use log::trace;
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::File,
    io::Write,
    time::{Duration, Instant}, sync::Mutex,
};

use lazy_static::lazy_static;

lazy_static! {
    // Only initialize profiling when the first entry is added.
    static ref PROFILING_START: Instant = Instant::now();

    pub static ref ALL_PROFILING_DATA: Mutex<ProfilingData> = Mutex::new(ProfilingData::new());
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
        let _profiling_keep_alive = crate::profiling::profiling_real::Timer::new(crate::function_name!().to_string(), "".to_string());
    };
    ($mand_1:expr) => {
        let _profiling_keep_alive =
            crate::profiling::profiling_real::Timer::new(crate::function_name!().to_string() + " " + $mand_1, "".to_string());
    };
    ($mand_1:expr, $mand_2:expr) => {
        let _profiling_keep_alive =
            crate::profiling::profiling_real::Timer::new(crate::function_name!().to_string() + " " + $mand_1, $mand_2);
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
        self.dump_profiling_to_chrome(
            "profiling_data/".to_string() + &thread_name + "_trace.json");
    }
}

impl ProfilingData{
    pub fn new() -> Self {
        ProfilingData{entries: vec![]}
    }

    pub fn append(&mut self, other: &mut ProfilingData) -> &mut Self{
        self.entries.append(&mut other.entries);
        self
    }

    pub fn dump_profiling_to_chrome(&self, file_name: String) {
        let mut file = File::create(&file_name).unwrap();
        write!(file, "[").unwrap();
    
        for i in 0..self.entries.len() {
            let entry = &self.entries[i];
            let name = &entry.detailed_name;
            let cat = "josh";
            let pid = 0;
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
        for (k, v) in map_profiling_to_string() {
            write!(
                file,
                r#"
            {{
                "name": "thread_name", "ph": "M", "pid": 0, "tid": {v},
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
        ("receive_response", "11")
    ])
}
