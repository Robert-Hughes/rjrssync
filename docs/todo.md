TODO:
=====

Interface
----------

* Could have some kind of hierarchy of filters, so can exclude something without continuing to evaluate other filters?
* Perhaps could have hard/soft includes/excludes - soft would keep evaluating other filters which may change the decision, hard would stop evaluating and keep that as the final decision.
* If a dir is excluded by the filters (after resolving all filters), then we don't walk inside that dir, so stuff inside it will be excluded *even if the filters would have matched them*. Document this?
* --dry-run (and the same for -v) should make it clearer exactly what is being copied to where, e.g. give absolute paths. If there is a long path up to the root (or afterwards), could shorten it with ellipses, e.g. "Copying T:\work\...\bob\folder\...\thing.txt to X:\backups\...\newbackup\folder\...\thing.txt"
* Option to override the "dest file is newer" error. Test this behaviour (not sure if it's tested atm)
* Should filters expect to see trailing slashes on folder names or not? What about folder symlinks?
* Ctrl-C doesn't seem to work very well at stopping rjrssync when it's running

Remote launching
----------------

* Additional SSH options as command-line arguments (separate for source and dest?)
* SSH host key verification prompt doesn't echo the user's typing, but it does seem to work anyway
* Sometimes remote processes are left orphaned
* Using temporary dir means that rebooting the remote will mean we have to rebuild from scratch (on Linux)
* We could first attempt to use an already-installed version of rjrssync (in the PATH), and only if this
doesn't exist or is incompatible do we deploy/build from scratch?
* Sometimes see "ssh stderr: mesg: ttyname failed: Inappropriate ioctl for device" when deploying to remote (I think
on 'F**A' platforms). Can we hide this using "-T" for example?

Syncing logic
-------------

* Compare and sync file permissions?
* Modified time:
    - need to account for time zone differences etc. between source and dest when updating the timestamp
    - would this play nicely with other tools (e.g. build systems) that check timestamps - it might think that it doesn't need to rebuild anything, as the new timestamp for this file is still really old?
    - Maybe instead we could store something else, like a hash or our own marker to indicate when this file was synced, so that the timestamp is "correct", but we know not to sync it again next time.
* Testing for sync logic, including between different combinations of windows and linux, remote and local etc.
   - Filters, and how they work on normalized paths between windows and linux.
   - Between different OSes,to make sure the path normalisation works
* Test for --dry-run
* Test for --stats (maybe just all the command-line options...)
* Tests for when filesystem operations fail, e.g. failing to read/write a file
* Progress bar
  - can format the bar with number of bytes, or number of files, and it provides e.t.a. and rate of progress
  - when copying large files, the progress bar won't move. Maybe have a sub-bar per-file for large files? Or change
   the bar to be total bytes rather than total files?
  - hide progress bar for --dry-run?
  - hide progress bar for --quiet?
  - hide progress bar for --no-progress?
  - do we want to show the Connecting spinner whilst deploying/building on a remote?
* What happens if src and dest both point to the same place?
   - Either directly, or via symlink(s)?
* --no-encryption option, might be faster?
   - Possibly want to keep the authentication aspects, but drop the encryption?
* How to handle case when want to copy two different folders into the same destination, some sort of --no-delete?
* Use of SystemTime
   -  is this compatible between platforms, time zone changes, precision differences, etc. etc.
   - can we safely serialize this on one platform and deserialize on another?
* --dry-run isn't honoured when creating dest ancestors! It should instead say that it _would_ create the ancestors.
* When splitting large files, the optimum chunk size might vary, we could adjust this dynamically.
Right now I just picked an arbitrary value which could possibly be improved a lot!
Also the same buffer size this is used for both the filesystem read() buffer size, _and_ the size of data we send to the boss, _and_
// the size of data written on the other doer. The same size might not be optimal for all of these!
* Use new prompting and behaviour logic for other cases
  - When files need overwriting/updating.
  - Combined with the other options, this would be a complete "no-destructive" mode if you set everything to prompt or skip? Maybe could have an option to set all of them at once (either 
  to full destructive or no destructive)?
* Errors aren't displayed in a very friendly way (it has all the logging prefixes)
* The prompt messages don't account for --dry-run, so it will look like things are actually going to be deleted,
when they're not

Performance
------------

* Investigate if parallelising some stages would speed it up, e.g. walking the dir structure on multiple threads, or sending data across network on multiple threads
   - Maybe check if WalkDir is slow, by comparing its performance with direct std::fs stuff or even native OS stuff?
* Investigate if pipelining some stages would speed it up, e.g. encrypting and serialization at same time
* Probably better to batch together File() Responses, to avoid overhead from sending loads of messages
* Run benchmark tests on GitHub actions?
* If launching two remote doers, then it would be quicker to run the two setup_comms in paśallel
   - need to watch out for ssh prompts though - what if we get two of these in parallel!
* Could investigate using UDP or something else to reduce TCP overhead, possibly this could speed up the TCP connection time?
* Benchmarking with two remotes rather than just one
* Measure peak memory usage in benchmark?
* Profiling events like send/receive could show the message type?
* Because our send_command/response now returns immediately, if the network is being slow then we will
  slowly take up more and more memory in the channel buffer (same on both sending and receiving side)! 
  - Idea: keep track of the size (rough bytes) of messages in the channel with some kind of Arc<Atomic<usize>> - the sending side checks this and busy-waits if it's too full (could use
  the busy wait helper thing in Crossbeam), the receiving side decrements the counter.
  Would need a "get size" function on messages, which could be std::mem::size() + any heap data.
* crossbeam channels are supposed to have better performance (and better API/features)

Misc
-----

* Make GitHub actions run on both Windows and Linux.
* Configure GitHub actions to run with remote hosts somehow
* piper and tcper maybe shouldn't be in the `bin/` folder, as then they count as part of the proper program (and then get installed with cargo install, or with crate publish?),
but they should just be for testing/investigation. Maybe should be a separate crate?
* On work PC this fails:
`cargo run D:\TempSource\ robhug01@localhost:/home/robhug01/TempDest -v`
ERROR | rjrssync::boss_frontend: Sync error: Unexpected response from dest GetEntries: Ok(Error("normalize_path failed: Illegal characters in path"))
* Improve compile times. Is it the RustEmbed crate? Maybe the debug-embed feature of the crate could help?
* Maybe should extend test framework to support doing things remotely, like saving and loading filesystem nodes, making and clearing out a temporary folder etc.
* Upload to crates.io, so that we can "cargo install" from anywhere?
* "cargo install" should only install rjrssync, not the other binaries like piper etc.
* Warning if filter doesn't match anything, possibly after GetEntries but before actually doing anything (to prevent mistaken filter?)
* Would be nice to automatically detect cases where the version number hasn't been updated, e.g. if we
could see that the Command/Response struct layout has changed.
* Async comms might not be handling errors properly - the threads can stop early due to either the tcp connection or the channel being closed, and might need propagating somehow?
