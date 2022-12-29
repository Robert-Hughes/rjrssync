use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use walkdir::WalkDir;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Instant;
use clap::Parser;

#[derive(Parser, Debug)]
struct Args {
    #[arg(short='t', long, default_value_t=1)]
    num_threads: u32,
    #[arg(short, long)]
    root: String,
}

fn main() {
    let args = Args::parse();

    let mut a = walk_dir(&args.root);
    let mut b = parallel_walk_dir(&args.root, args.num_threads);

    a.remove(0);
    a.sort();
    b.sort();
    assert_eq!(a, b);
}

fn walk_dir(root: &str) -> Vec<PathBuf> {
    let start = Instant::now();
    let walker_it = WalkDir::new(root)
        .follow_links(false)  // We want to see the symlinks, not their targets
        .into_iter();
    let mut result = vec![];
    for x in walker_it {
        result.push(PathBuf::from(x.unwrap().path()));
    }
    let elapsed = start.elapsed().as_millis();
    println!(
        "WalkDir {} in {}ms ({}/s)",
        result.len(),
        elapsed,
        1000.0 * result.len() as f32 / elapsed as f32
    );

    result
}

fn parallel_walk_dir(root: &str, num_threads: u32) -> Vec<PathBuf> {
    let start = Instant::now();
    let mut result = vec![];

    let (job_sender, job_receiver) = crossbeam::channel::unbounded();
    let (result_sender, result_receiver) = crossbeam::channel::unbounded();
    let busy_worker_count = Arc::new(AtomicUsize::new(0));
    for _ in 0..num_threads {
        let job_sender = job_sender.clone();
        let job_receiver = job_receiver.clone();
        let result_sender = result_sender.clone();
        let busy_worker_count = busy_worker_count.clone();
        std::thread::spawn(move || worker_main(job_sender, job_receiver, result_sender, busy_worker_count, num_threads));
    }

    job_sender.send(Job::Dir(PathBuf::from(root))).unwrap();

    drop(job_sender);
    drop(result_sender);

    while let Ok(x) = result_receiver.recv() {
        result.push(x.path());
    }

    let elapsed = start.elapsed().as_millis();
    println!(
        "Parallel {} in {}ms ({}/s)",
        result.len(),
        elapsed,
        1000.0 * result.len() as f32 / elapsed as f32
    );

    result
}

enum Job {
    Dir(PathBuf),
    Done
}

fn worker_main(job_sender: Sender<Job>, job_receiver: Receiver<Job>,
    result_sender: Sender<std::fs::DirEntry>, busy_worker_count: Arc<AtomicUsize>,
    num_threads: u32) {
    loop {
        let job = job_receiver.recv().expect("Receive error");
        busy_worker_count.fetch_add(1, Ordering::SeqCst);

        match job {
            Job::Dir(x) => {
                for x in std::fs::read_dir(x).expect("read dir error") {
                    let x = x.expect("entry error");

                    let child_dir_to_recurse = if x.file_type().unwrap().is_dir() {
                        Some(x.path())
                    } else {
                        None
                    };

                    result_sender.send(x).unwrap();

                    if let Some(x) = child_dir_to_recurse {
                        job_sender.send(Job::Dir(x)).unwrap();
                    }
                }               
            }
            Job::Done => break,
        }

        let prev_count = busy_worker_count.fetch_sub(1, Ordering::SeqCst);
        if prev_count == 1 && job_receiver.len() == 0 { // almost certainly this is wrong, use condition var or something instead? or Counter of "job not done"
            for _ in 0..num_threads {
                job_sender.send(Job::Done).unwrap();
            }
        }
    }
}