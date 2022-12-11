use std::{process::Stdio, sync::mpsc::{Sender, Receiver, self, SendError}, thread, fmt::{Display, self}, io::{BufReader, BufRead}};

pub struct ProcessOutput {
    pub exit_status: std::process::ExitStatus,
    pub stdout: String,
    #[allow(unused)]
    pub stderr: String,
}

/// Runs a child processes and waits for it to exit. The stdout and stderr of the child process
/// are captured and forwarded to our own.
/// Simply letting the child process inherit out stdout/stderr seems to cause problems with line endings getting messed
/// up and losing output, and unwanted clearing of the screen.
/// This is mostly a copy-paste of the same function from boss_launch.rs, but we don't have a good way to share the code
/// and this version is slightly different, more suitable for tests (e.g. simpler error checking, logging printed with println).
pub fn run_process_with_live_output(c: &mut std::process::Command) -> ProcessOutput {
    println!("Running {:?} {:?}...", c.get_program(), c.get_args());

    let mut child = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to launch child process");

    // unwrap is fine here, as the streams should always be available as we piped them all
    let child_stdout = child.stdout.take().unwrap();
    let child_stderr = child.stderr.take().unwrap();

    // Spawn a background thread for each stdout and stderr, to process messages we get from the child
    // and forward them to the main thread. This is easier than some kind of async IO stuff.
    let (sender1, receiver): (
        Sender<OutputReaderThreadMsg2>,
        Receiver<OutputReaderThreadMsg2>,
    ) = mpsc::channel();
    let sender2 = sender1.clone();
    let thread_builder = thread::Builder::new().name("child_stdout_reader".to_string());
    thread_builder.spawn(move || {
        output_reader_thread_main2(child_stdout, OutputReaderStreamType::Stdout, sender1)
    }).unwrap();
    let thread_builder = thread::Builder::new().name("child_stderr_reader".to_string());
    thread_builder.spawn(move || {
        output_reader_thread_main2(child_stderr, OutputReaderStreamType::Stderr, sender2)
    }).unwrap();

    let mut captured_stdout = String::new();
    let mut captured_stderr = String::new();
    loop {
        match receiver.recv() {
            Ok(OutputReaderThreadMsg2::Line(stream_type, l)) => {
                // Print output for test debugging. Note that we need to use println, not write directly to stdout, so that
                // cargo's testing framework captures the output correctly.
                match stream_type {
                    OutputReaderStreamType::Stdout => {
                        println!("{}", l);
                        captured_stdout += &(l + "\n");
                    }
                    OutputReaderStreamType::Stderr => {
                        eprintln!("{}", l);
                        captured_stderr += &(l + "\n");
                    }
                }
            }
            Ok(OutputReaderThreadMsg2::Error(stream_type, e)) => {
                panic!("Error reading from {}: {}", stream_type, e);
            }
            Ok(OutputReaderThreadMsg2::StreamClosed(stream_type)) => {
                println!("Child process {} closed", stream_type);
            }
            Err(_) => {
                // Both senders have been dropped, i.e. both background threads exited
                println!("Both reader threads done, child process must have exited. Waiting for process.");
                // Wait for the process to exit, to get the exit code
                let result = match child.wait() {
                    Ok(r) => r,
                    Err(e) => panic!("Error waiting for child process: {}", e),
                };
                println!("Exit status: {:?}", result);
                return ProcessOutput { exit_status: result, stdout: captured_stdout, stderr: captured_stderr };
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum OutputReaderStreamType {
    Stdout,
    Stderr,
}
impl Display for OutputReaderStreamType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            OutputReaderStreamType::Stdout => write!(f, "stdout"),
            OutputReaderStreamType::Stderr => write!(f, "stderr"),
        }
    }
}

// Sent from the threads reading stdout and stderr of a child process back to the main thread.
enum OutputReaderThreadMsg2 {
    Line(OutputReaderStreamType, String),
    Error(OutputReaderStreamType, std::io::Error),
    StreamClosed(OutputReaderStreamType),
}

fn output_reader_thread_main2<S>(
    stream: S,
    stream_type: OutputReaderStreamType,
    sender: Sender<OutputReaderThreadMsg2>,
) -> Result<(), SendError<OutputReaderThreadMsg2>>
where S : std::io::Read {
    let mut reader = BufReader::new(stream);
    loop {
        let mut l: String = "".to_string();
        // Note we ignore errors on the sender here, as the other end should never have been dropped while it still cares
        // about our messages, but may have dropped if they abandon the child process, letting it finish itself.
        match reader.read_line(&mut l) {
            Err(e) => {
                sender.send(OutputReaderThreadMsg2::Error(stream_type, e))?;
                return Ok(());
            }
            Ok(0) => {
                // end of stream
                sender.send(OutputReaderThreadMsg2::StreamClosed(stream_type))?;
                return Ok(());
            }
            Ok(_) => {
                l.pop(); // Remove the trailing newline
                // A line of other content, for example a prompt or error from ssh itself
                sender.send(OutputReaderThreadMsg2::Line(stream_type, l))?;
            }
        }
    }
}