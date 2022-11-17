Fast rsync-like tool for incrementally copying files. Runs natively on both Windows and Linux and uses network for communication, to maximise speed when syncing between Windows and WSL filesystems (accessing WSL filesytem from Windows or vice-versa is slow).

Some perf results of walking directories on each OS:

   Host ->       Windows     Linux
Filesystem:
  Windows        100k        9k
   Linux          1k         500k

Notes on performance & security
===============================

Transferring data through the stdin/stdout of ssh is pretty slow (on Windows at least),
at most about 20MB/s. It's nice and secure though (we get authentication and encryption).

A faster option would be to use ssh's port forwarding so that we can use a separate TCP
connection. Testing on Windows showed this could peak at around 200MB/s. We lose some of the
security though, as anybody can connect to the tunnel.

The best for performance is a direct TCP connection (without ssh), which peaks at around 2GB/s
locally on Windows. We can use ssh for the initial setup, sharing some kind of secret key 
so that each side of the connection knows that the other end is authentic. If we want to add
encryption to the data being transferred, we would need to do this separately, but it isn't
a big concern at the moment.

TCP connection throughput (tcper):

Windows -> Windows: 2-3GB/s
Windows -> WSL: 500-600MB/s
WSL -> Windows: can't connect!

SSH port forwarded throughput (ntttcp):

Windows -> Windows: ~200MB/s

SSH stdin throughput (piper):

Windows -> Windows: ~20MB/s

Stdin throughput (piper):


TODO:

* Review/tidy up sync code in boss_sync.rs and also the command handling code in doer.rs
* Dry run flag
* Config file containing src/dest, ignore/allow list etc. Use serde_json?
    * List of folders to sync, with src and dest spec (computer and absolute path)
    * Each folder to be synced has list of include/exclude filters, applied in order (can mix and match include/exclude however you want)
    * Could have some kind of hierarchy of filters, so can exclude something without continuing to evaluate other filters?
    * Perhaps could have hard/soft includes/excludes - soft would keep evaluating other filters which may change the decision, hard would stop evaluating and keep that as the final decision.
    * Filters could be regexes on the path relative to the root (folder being synced)
    * If a dir is excluded by the filters (after resolving all filters), then we don't walk inside that dir, so stuff inside it will be excluded *even if the filters would have matched them*
* Additional SSH options as command-line arguments (separate for source and dest?)
* Investigate if parallelising some stages would speed it up, e.g. walking the dir structure on multiple threads, or sending data across network on multiple threads
* Investigate if pipelining some stages would speed it up, e.g. sending file list while also sending it
* Remote launching on windows (temp folder path is unix-style!)
* SSH host key verification prompt doesn't echo the user's typing, but it does seem to work anyway
* Probably better to batch together File() Responses, to avoid overhead from sending loads of messages
* Perf comparison with regular rsync (for cases where there are zero or few changes, and for cases with more changes)
* Compare and sync file permissions?
* Modified time:
    - need to account for time zone differences etc. between source and dest when updating the timestamp
    - would this play nicely with other tools (e.g. build systems) that check timestamps - it might think that it doesn't need to rebuild anything, as the new timestamp for this file is still really old?
    - Maybe instead we could store something else, like a hash or our own marker to indicate when this file was synced, so that the timestamp is "correct", but we know not to sync it again next time.
* Testing for ssh launching/copying/deploying stuff
* Testing for sync logic, including between different combinations of windows and linux, remote and local etc. 
   - Exclude filters
* Progress bar
* Format total bytes and total files etc. with commas, or GB, MB etc.
* Create destination root if it doesn't exist?
* Only show histograms based on an argument, to avoid cluttering the output?
* Logging seems to be slowing things down by adding extra bandwidth, especially for the remote side?
Even when it's disabled, it might still be evaluating ths log arguments (including the full contents of files!)

Idea for filters, with re-usable "functions":
===============

"filters": [
   "src/.*" : include,
   "tests/.*" : include,
   ".*\.exe" : exclude,
   ".*/rob.exe" : include,
   "folderA/(.*)" : artifactsOnly($1),
   "folderB/(.*)" : artifactsOnly($1),
]

"artifactsOnly": [
   ".*" : exclude,
   "artifacts/.*\.bin": include,
   "other/artifacts/.*\.bin" : include,
]
