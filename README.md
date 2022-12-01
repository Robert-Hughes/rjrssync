Fast rsync-like tool for incrementally copying files. Runs natively on both Windows and Linux and uses network for communication, to maximise speed when syncing between Windows and WSL filesystems.

Notes on performance & security
===============================

Some perf results of walking directories on each OS:

   Host ->       Windows     Linux
Filesystem:
  Windows        100k        9k
   Linux          1k         500k

Conclusion => accessing WSL filesytem from Windows or vice-versa is slow, so we use native access.

Transferring data through the stdin/stdout of ssh is pretty slow (on Windows at least),
at most about 20MB/s. It's nice and secure though (we get authentication and encryption).

A faster option would be to use ssh's port forwarding so that we can use a separate TCP
connection. Testing on Windows showed this could peak at around 200MB/s. We lose some of the
security though, as anybody can connect to the tunnel.

The best for performance is a direct TCP connection (without ssh), which peaks at around 2GB/s
locally on Windows. We can use ssh for the initial setup, sharing some kind of secret key
so that each side of the connection is secure.

All readings from home PC (MANTA)

Stdin throughput (piper, buffers all 40960):
(no encryption)

Windows -> Windows (piper.exe | piper.exe): ~2.7GB/s
Windows -> WSL (piper.exe | wsl piper): ~500MB/s
WSL -> Windows (piper | piper.exe): 300-400MB/s
WSL -> WSL (piper | piper): ~7.5GB/s

SSH stdin throughput (piper, buffers all 40960):
(encrypted and authenticated etc, as it's all through SSH)

Windows -> Windows (piper.exe | ssh windows piper.exe): ~18MB/s
Windows -> WSL (piper.exe | ssh wsl piper): ~18MB/s
WSL -> Windows (piper | ssh windows piper.exe): ~200MB/s
WSL -> WSL (piper | ssh wsl piper.exe): 200-250MB/s

TCP connection throughput (tcper, buffers all 40960):
(no encryption)

Windows -> Windows: ~1.5GB/s (gets even faster with bigger buffers!)
Windows -> WSL: 500-600MB/s
WSL -> Windows: ~1.5GB/s (firewall needs enabling for PUBLIC networks!)
WSL -> WSL: ~6GB/s

SSH port forwarded throughput (tcper, buffers all 40960):
(note that even though the traffic is encrypted in-transit, anybody can connect to the ports!)

Windows -> Windows (ssh from Windows to Windows to forward local port): ~150MB/s
Windows -> WSL (ssh from Windows to WSL to forward local port): ~150MB/s
WSL -> Windows (ssh from WSL to Windows to forward local port): ~200MB/s
WSL -> WSL (ssh from WSL to WSL to forward local port): ~230MB/s

TCP connection with shared key encryption using serde_encrypt (tcper -e, buffers all 40960):
NOTE: these readings were taken with a bug in both piper and tcper where we weren't writing all the data! :O

Windows -> Windows: 250-300MB/s
Windows -> WSL: 250-300MB/s
WSL -> Windows: ~300MB/s
WSL -> WSL: ~300MB/s

TCP connection with shared key encryption using aes-gcm Aes256Gcm (tcper -e, buffers all 40960):

Windows -> Windows: ~700MB/s
Windows -> WSL: ~500MB/s
WSL -> Windows: 700-800MB/s
WSL -> WSL: 900-1000MB/s


Tree representation
===================

There are some interesting design questions that arise about how to handle trees of files and folders,
for example whether two files should be considered equal if they have the same contents but different names.
If the answer is yes then it seems that the name shouldn't be part of the file itself, but is instead
used as an identifier _for_ the file. The same thing applies to folders. This seems to be the most
consistent view on the matter. This means that the program makes the destination object (file, folder etc.)
be the same as the source object, and that doesn't mean they need to have the same name.

There are several ambiguities about how to sync depending on whether the paths given exist, if they have a trailing
slash, etc or if they are a file or a folder. The goal is that the program should be intuitive and easy to use and
easy to reason about its behaviour. The current plan is to behave as follows:

Each row is for a different type of source that the source path points to (something that doesn't exist, an
existing file or an existing folder). Each column is for a different type of dest that the dest path points to.
Each row/col is further broken down into versions that include or do not include a trailing slash on the path.
The cell contents describe the behaviour given those inputs:
   - 'X' means this is an error
   - 'b' means that the source is copied over to the path b, creating, updating or replacing whatever might be there.
   - 'b/a' means that the source is copied over to the path b/a, creating, updating or replacing whatever might be there.
   - '*' indicates that the behaviour might be surprising/destructive because it deletes an existing file or folder and replaces it
        with a folder/file. We should probably warn for this.

|---------------------------------------------------------------------|
|          Dest ->    |  Non-existent |      File     |    Folder     |
|                     |---------------|---------------|---------------|
|  Source v           |   b   |  b/   |   b   |  b/   |   b   |  b/   |
|---------------------|-------|-------|-------|-------|-------|-------|
|              src/a  |               |               |               |
| Non-existent        |       X       |       X       |       X       |
|              src/a/ |               |               |               |
|---------------------|-------|-------|-------|-------|-------|-------|
|              src/a  |   b   |  b/a  |   b   |   X   |   b*  |  b/a  |
| File                |-------|-------|-------|-------|-------|-------|
|              src/a/ |   X   |   X   |   X   |   X   |   X   |   X   |
|---------------------|-------|-------|-------|-------|-------|-------|
|              src/a  |   b   |   b   |   b*  |   X   |   b   |   b   |
| Folder              |-------|-------|-------|-------|-------|-------|
|              src/a/ |   b   |   b   |   b*  |   X   |   b   |   b   |
|---------------------|-------|-------|-------|-------|-------|-------|

The behaviour can be summarised as a "golden rule" which is that after the sync, the object pointed to by the destination
path will be identical to the object pointed to by the source path, i.e. `tree $SRC == tree $DEST`.

There is one exception, which is that if the dest path has a trailing slash, and source is an (existing) file, then
the dest path is first modified to have the final part of the source path appended to it. e.g.:

`rjrssync folder/file.txt backup/` => `backup/file.txt`

This makes it more ergonomic to copy individual files. Unfortunately it makes the behaviour of files and folder inconsistent,
but this this is fine because files and folders are indeed different, and it's worth the sacrifice.

It has the property that non-existent dest files/folders are treated the same as if they did exist, which means that
you get a consistent final state no matter the starting state (behaviour is idempotent).

It also prevents unintended creation of nested folders with the same name which can be annoying, e.g.

`rjrssync src/folder dest/folder` => `dest/folder/...` (rather than `dest/folder/folder/...`)

Trailing slashes on files are always invalid, because this gives the impression that the file is actually a folder,
and so could lead to unexpected behaviour.

Notes on symlinks
==================

Symlinks could be present as ancestors in the path(s) being synced (`a/b/symlink/c`),
the path being synced itself (`a/b/symlink`), or as one of the items inside a folder being synced.

Symlinks can point to either a file, a folder, nothing (broken), or another symlink, which itself could point to
any of those.

Symlinks can cause cycles.

On Windows, a symlink is either a "file symlink" or "directory symlink" (specified on creation),
whereas on Linux it is simply a symlink.

Example spec file
===================

```
# Defaults to local path, if no remote hostname is specified
# src_hostname: computer1
# src_username: root
dest_hostname: computer2
dest_username: myuser
syncs:
  - src: D:/Source
    dest: D:/Dest
    exclude: [ "\.txt" ]
  - src: D:/Source
    dest: D:/Dest2
```

TODO:
=====

Interface
----------

* Could have some kind of hierarchy of filters, so can exclude something without continuing to evaluate other filters?
* Perhaps could have hard/soft includes/excludes - soft would keep evaluating other filters which may change the decision, hard would stop evaluating and keep that as the final decision.
* If a dir is excluded by the filters (after resolving all filters), then we don't walk inside that dir, so stuff inside it will be excluded *even if the filters would have matched them*. Document this?
* --dry-run (and the same for -v) should make it clearer exactly what is being copied to where, e.g. give absolute paths. If there is a long path up to the root (or afterwards), could shorten it with ellipses, e.g. "Copying T:\work\...\bob\folder\...\thing.txt to X:\backups\...\newbackup\folder\...\thing.txt"
* Option to override the "dest file is newer" error
* Should filters expect to see trailing slashes on folder names or not?

Remote launching
----------------

* Additional SSH options as command-line arguments (separate for source and dest?)
* SSH host key verification prompt doesn't echo the user's typing, but it does seem to work anyway
* Sometimes remote processes are left orphaned, preventing new ones from listening on the same port
* Using temporary dir means that rebooting the remote will mean we have to rebuild from scratch (on Linux)


Syncing logic
-------------

* Compare and sync file permissions?
* Modified time:
    - need to account for time zone differences etc. between source and dest when updating the timestamp
    - would this play nicely with other tools (e.g. build systems) that check timestamps - it might think that it doesn't need to rebuild anything, as the new timestamp for this file is still really old?
    - Maybe instead we could store something else, like a hash or our own marker to indicate when this file was synced, so that the timestamp is "correct", but we know not to sync it again next time.
* Testing for sync logic, including between different combinations of windows and linux, remote and local etc.
   - Exclude filters
* Test for --dry-run
* Test for --stats (maybe just all the command-line options...)
* Progress bar
* Support symlinks (see notes on symblinks above)
  - Support in testing framework
  - Two modes - symlink unaware (treats the link as the target), and symlink aware (just syncs the link)?
  - Support/test different 'types' of symlinks (windows types and also different target types?)
  - Symlink target could be a file, a folder, non-existent, or another symlink (of any of these types...)
  - Either src or dest itself could be symlink
  - Symlinks could be present as ancestors as src or dest path (though this shouldn't matter)
  - Symlinks could be present inside a folder being synced.
  - Symlinks could be combined with any existing type as src or dest (e.g. symlink => folder, file => symlink etc.)
* What happens if src and dest both point to the same place?
   - Either directly, or via symlink(s)?
* --no-encryption option, might be faster?
* How to handle case when want to copy two different folders into the same destination, some sort of --no-delete?
* Use of SystemTime
   -  is this compatible between platforms, time zone changes, precision differences, etc. etc.
   - can we safely serialize this on one platform and deserialize on another?



Performance
------------

* Investigate if parallelising some stages would speed it up, e.g. walking the dir structure on multiple threads, or sending data across network on multiple threads
* Parallelise querying  - see parallel-query branch
* Investigate if pipelining some stages would speed it up, e.g. sending file list while also sending it
* Probably better to batch together File() Responses, to avoid overhead from sending loads of messages
* Perf comparison with regular rsync (for cases where there are zero or few changes, and for cases with more changes)


Misc
-----

* Fix intermittent github actions failing because of RustEmbed
* Make GitHub actions run on both Windows and Linux.
* Configure GitHub actions to run with remote hosts somehow
* piper and tcper maybe shouldn't be in the `bin/` folder, as then they count as part of the proper program,
but they should just be for testing/investigation. Maybe should be a separate crate?
* On work PC this fails:
`cargo run D:\TempSource\ robhug01@localhost:/home/robhug01/TempDest -v`
ERROR | rjrssync::boss_frontend: Sync error: Unexpected response from dest GetEntries: Ok(Error("normalize_path failed: Illegal characters in path"))
* Improve compile times. Is it the RustEmbed crate?


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
