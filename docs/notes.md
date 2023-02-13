Notes on performance & security
===============================

Some perf results of walking directories on each OS:

```
   Host ->       Windows     Linux
Filesystem:
  Windows        100k        9k
   Linux          1k         500k
```

Conclusion => accessing WSL filesytem from Windows or vice-versa is slow, so we use native access.

Transferring data through the stdin/stdout of ssh is pretty slow (on Windows at least),
at most about 20MB/s. It's nice and secure though (we get authentication and encryption).

A faster option would be to use ssh's port forwarding so that we can use a separate TCP
connection. Testing on Windows showed this could peak at around 200MB/s. We lose some of the
security though, as anybody can connect to the tunnel.

The best for performance is a direct TCP connection (without ssh), which peaks at around 2GB/s
locally on Windows. We can use ssh for the initial setup, sharing some kind of secret key
so that each side of the connection is secure.

```

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

```

Disabling PAM on sshd_config seems to speed up ssh login https://serverfault.com/questions/792486/ssh-connection-takes-forever-to-initiate-stuck-at-pledge-network

Disabling progress bar --no-progress can help, esp. on GitHub actions (low CPU count)

Different build toolchains (e.g. -gnu vs -musl) can make a difference to performance.

Removing a big allocation from receive() seemed to fix the dodgy progress bar updates on -musl remote builds. Seems like (big?) allocations on musl might be slow, can reduce these to help perf.

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
   - '!' indicates that the behaviour might be surprising/destructive because it deletes an existing file or folder and replaces it
        with a folder/file. We prompt the user for this.

```

|---------------------------------------------------------------------|
|          Dest ->    |  Non-existent |File or symlink|    Folder     |
|                     |---------------|---------------|---------------|
|  Source v           |   b   |  b/   |   b   |  b/ * |   b   |  b/   |
|---------------------|-------|-------|-------|-------|-------|-------|
|              src/a  |               |               |               |
| Non-existent        |       X       |       X       |       X       |
|              src/a/ |               |               |               |
|---------------------|-------|-------|-------|-------|-------|-------|
|              src/a  |   b   |  b/a  |   b   |   X   |   b!  |  b/a  |
| File or             |-------|-------|-------|-------|-------|-------|
| symlink     src/a/ *|   X   |   X   |   X   |   X   |   X   |   X   |
|---------------------|-------|-------|-------|-------|-------|-------|
|              src/a  |   b   |   b   |   b!  |   X   |   b   |   b   |
| Folder              |-------|-------|-------|-------|-------|-------|
|              src/a/ |   b   |   b   |   b!  |   X   |   b   |   b   |
|---------------------|-------|-------|-------|-------|-------|-------|

*: On Linux, a symlink with a trailing slash that points to a folder is treated as the target folder.

```

The behaviour can be summarised as a "golden rule" which is that after the sync, the object pointed to by the destination path will be identical to the object pointed to by the source path, i.e. `tree $SRC == tree $DEST`.

There is one exception, which is that if the dest path has a trailing slash, and source is an (existing) file, then the dest path is first modified to have the final part of the source path appended to it. e.g.:

`rjrssync folder/file.txt backup/` => `backup/file.txt`

This makes it more ergonomic to copy individual files. Unfortunately it makes the behaviour of files and folder inconsistent, but this this is fine because files and folders are indeed different, and it's worth the sacrifice.

It has the property that non-existent dest files/folders are treated the same as if they did exist, which means that you get a consistent final state no matter the starting state (behaviour is idempotent).

It also prevents unintended creation of nested folders with the same name which can be annoying, e.g.

`rjrssync src/folder dest/folder` => `dest/folder/...` (rather than `dest/folder/folder/...`)

Trailing slashes on files are always invalid, because this gives the impression that the file is actually a folder, and so could lead to unexpected behaviour.

Symlinks are treated the same as files, because that's essentially how rjrssync treats symlinks (it syncs
the link address, as if it was a small text file). Therefore they can't have trailing slashes.
Note that tab-completion (on bash and cmd at least) does not put a trailing slash on symlinks automatically,
so this shouldn't be a problem. (For folders, bash does do this which is why it's useful to allow trailing
slashes on folder names).

Note though that on Linux, the OS treats a trailing slash on a symlink to refer to the _target_
of the symlink, not the symlink itself, and so rjrssync doesn't actually see these as symlinks at all, so they
behave as folders.

Notes on symlinks
==================

Symlinks could be present as ancestors in the path(s) being synced (`a/b/symlink/c`),
the path being synced itself (`a/b/symlink`, one or both sides), or as one of the items inside a folder being synced.

Symlinks can point to either a file, a folder, nothing (broken), or another symlink, which itself could point to any of those.

Symlinks can cause cycles and DAGs, including pointing to itself. This shouldn't be too important for us,
as we (generally) never follow symlinks and so will never observe these cases.

On Windows, a symlink is either a "file symlink" or "directory symlink" (specified on creation),
whereas on Linux it is simply a symlink. A "directory symlink" that points to a file (possibly via other symlinks) or a "file symlink" that points to a directory is considered broken (similar to the target not existing at all).

It is possible to create an invalid symlink (target is the wrong 'type' or doesn't exist)

Symlink targets can be specified as relative or absolute.

Symlinks have their own modified time (which is when the link path was changed, not equal to the target's modified time), but we don't use as we can compare the link target instead (we don't have this luxury when
syncing files, because their contents might be huge, but a link target is small).

There can be multiple symlinks followed in a path being synced, e.g. <ROOT>/symlink1/folder2/symlink3/file,
but this would only be observed if rjrssync was unaware of symlinks (otherwise it would never walk into the first symlink), so we can't actually encounter this.

This crate has some useful explanation of some of these concepts: https://crates.io/crates/symlink.

The target link address on Windows might contain backslashes, which would need converting when sending over to Linux. The other way round should be fine, because Windows supports forward slashes too.

rjrssync treats symlinks as if they were simple text files containing their target address.
They are not (generally) followed or validated. They will be reproduced as accurately as possible on
the destination.
Note that this only applies to symlinks that are part of the sync (including as the root);
symlinks that are ancestors of the path provided as source or dest will always be followed in order
to get to the transfer root.

Known quirks with the current behaviour:

Existing windows symlink on the dest side, and then a broken Unix symlink on the source side, with the same target link address:
Currently this will result in an error, as we will attempt to create a Generic symlink on the dest, which will fail. In this case it's pretty likely that the user would prefer us to simply leave the dest symlink as it is
(whether it be a file or folder symlink).

Trailing slashes on symlinks are handled different on Windows and Linux, see above section on trailing slashes
for details.


It was considered for rjrssync to have two 'modes', as to whether it ignores the symlinks (treats them as
their targets) or syncs them as the links. However it was decided against this because of the increase in
complexity of the testing and some quirky behaviour in the "unaware" mode (see below):

Quirks of unaware mode:

On Linux, if there's a symlink to a folder and we're in unaware mode, then we won't be able to delete it because on Linux, deleting a symlink has to be done via remove_file, not remove_dir. However when in unaware mode, we see it as a dir, and so use remove_dir.

When deleting a symlink folder and we're in unaware model, the whole symlink *target* folder will be cleared out too, as well as deleting the symlink itself. This is somewhat surprising, but has to be the case because rjrssync is unaware that it is deleting stuff through a symlink.

Idea for filters, with re-usable "functions":
===============

```

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

```

Benchmarking results
=======================

`cargo bench --features embed-all`, see benchmarks.rs. Run on both Windows and Linux.

Some more advanced options (see `cargo bench -- --help` for details):

`cargo bench --features embed-all -- --skip-setup --only-remote --programs rjrssync -n 5`

```
Each cell shows <min> - <max> over 5 sample(s) for: time | local memory (if available) | remote memory (if available)

Windows -> Windows
┌───────────────────┬────────────────────────────────────────────┐────────────────────────────────────────────┬────────────────────────────────────────────┬────────────────────────────────────────────┬───────────────────┐
│ Test case         │ rjrssync.exe (x10)                         │ scp (x10)                                  │ xcopy (x10)                                │ robocopy (x10)                             │ APIs (x10)        │
├───────────────────┼────────────────────────────────────────────┤────────────────────────────────────────────┼────────────────────────────────────────────┼────────────────────────────────────────────┼───────────────────┤
│ Everything copied │ 3.09s   - 3.90s  | 37.84 MiB  - 78.93 MiB  │ 2.40s   - 3.08s  | 7.23 MiB   - 7.25 MiB   │ 2.43s   - 3.01s  | 5.70 MiB   - 5.84 MiB   │ 1.85s   - 2.46s  | 6.83 MiB   - 6.96 MiB   │ 2.02s   - 2.98s   │
│ Nothing copied    │ 58ms    - 119ms  | 9.54 MiB   - 9.90 MiB   │ Skipped                                    │ Skipped                                    │ 93ms    - 126ms  | 5.69 MiB   - 5.71 MiB   │ Skipped           │
│ Some copied       │ 78ms    - 127ms  | 9.51 MiB   - 9.97 MiB   │ Skipped                                    │ Skipped                                    │ 237ms   - 291ms  | 6.04 MiB   - 6.11 MiB   │ Skipped           │
│ Delete and copy   │ 2.87s   - 3.59s  | 64.95 MiB  - 65.14 MiB  │ Skipped                                    │ Skipped                                    │ 2.76s   - 3.17s  | 6.91 MiB   - 7.04 MiB   │ Skipped           │
│ Single large file │ 725ms   - 888ms  | 29.40 MiB  - 70.22 MiB  │ 403ms   - 488ms  | 7.23 MiB   - 7.24 MiB   │ 380ms   - 438ms  | 5.63 MiB   - 5.65 MiB   │ 382ms   - 462ms  | 6.69 MiB   - 6.80 MiB   │ 387ms   - 537ms   │
└───────────────────┴────────────────────────────────────────────┘────────────────────────────────────────────┴────────────────────────────────────────────┴────────────────────────────────────────────┴───────────────────┘

Windows -> \\wsl$\...
┌───────────────────┬────────────────────────────────────────────┐────────────────────────────────────────────┬────────────────────────────────────────────┬────────────────────────────────────────────┬───────────────────┐
│ Test case         │ rjrssync.exe (x10)                         │ scp (x10)                                  │ xcopy (x10)                                │ robocopy (x10)                             │ APIs (x10)        │
├───────────────────┼────────────────────────────────────────────┤────────────────────────────────────────────┼────────────────────────────────────────────┼────────────────────────────────────────────┼───────────────────┤
│ Everything copied │ 8.56s   - 10.46s | 78.58 MiB  - 79.03 MiB  │ 23.85s  - 24.51s | 7.22 MiB   - 7.25 MiB   │ 23.76s  - 24.31s | 12.28 MiB  - 12.59 MiB  │ 12.81s  - 13.15s | 13.77 MiB  - 14.02 MiB  │ 8.80s   - 9.14s   │
│ Nothing copied    │ 219ms   - 241ms  | 8.85 MiB   - 9.16 MiB   │ Skipped                                    │ Skipped                                    │ 925ms   - 973ms  | 5.70 MiB   - 5.72 MiB   │ Skipped           │
│ Some copied       │ 282ms   - 316ms  | 8.92 MiB   - 9.14 MiB   │ Skipped                                    │ Skipped                                    │ 1.04s   - 1.07s  | 5.95 MiB   - 6.04 MiB   │ Skipped           │
│ Delete and copy   │ 12.15s  - 12.96s | 65.18 MiB  - 65.46 MiB  │ Skipped                                    │ Skipped                                    │ 18.73s  - 19.09s | 13.82 MiB  - 14.09 MiB  │ Skipped           │
│ Single large file │ 6.90s   - 8.22s  | 222.62 MiB - 222.69 MiB │ 2.94s   - 3.03s  | 7.22 MiB   - 7.25 MiB   │ 2.96s   - 3.03s  | 12.18 MiB  - 12.19 MiB  │ 2.91s   - 3.00s  | 13.67 MiB  - 13.72 MiB  │ 2.90s   - 2.98s   │
└───────────────────┴────────────────────────────────────────────┘────────────────────────────────────────────┴────────────────────────────────────────────┴────────────────────────────────────────────┴───────────────────┘

Windows -> Remote Windows
┌───────────────────┬─────────────────────────────────────────────────────────────────────┐────────────────────────────────────────────┐
│ Test case         │ rjrssync.exe (x10)                                                  │ scp (x10)                                  │
├───────────────────┼─────────────────────────────────────────────────────────────────────┤────────────────────────────────────────────┤
│ Everything copied │ 3.37s   - 4.37s  | 20.05 MiB  - 20.83 MiB | 21.68 MiB  - 33.76 MiB  │ 4.48s   - 5.02s  | 7.96 MiB   - 8.06 MiB   │
│ Nothing copied    │ 366ms   - 615ms  | 8.57 MiB   - 9.24 MiB  | 6.87 MiB   - 7.33 MiB   │ Skipped                                    │
│ Some copied       │ 248ms   - 498ms  | 8.92 MiB   - 9.07 MiB  | 7.02 MiB   - 7.36 MiB   │ Skipped                                    │
│ Delete and copy   │ 3.21s   - 4.04s  | 21.67 MiB  - 22.70 MiB | 43.09 MiB  - 45.23 MiB  │ Skipped                                    │
│ Single large file │ 4.07s   - 7.57s  | 226.66 MiB - 227.44 MiB| 18.83 MiB  - 24.71 MiB  │ 4.93s   - 5.14s  | 7.35 MiB   - 7.51 MiB   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┘────────────────────────────────────────────┘

Windows -> Remote Linux
┌───────────────────┬─────────────────────────────────────────────────────────────────────┐────────────────────────────────────────────┐
│ Test case         │ rjrssync.exe (x10)                                                  │ scp (x10)                                  │
├───────────────────┼─────────────────────────────────────────────────────────────────────┤────────────────────────────────────────────┤
│ Everything copied │ 1.07s   - 1.63s  | 20.79 MiB  - 21.46 MiB | 215.95 MiB - 215.95 MiB │ 14.96s  - 16.92s | 7.99 MiB   - 8.16 MiB   │
│ Nothing copied    │ 618ms   - 1.21s  | 8.96 MiB   - 9.37 MiB  | 217.96 MiB - 218.09 MiB │ Skipped                                    │
│ Some copied       │ 622ms   - 1.24s  | 9.02 MiB   - 9.34 MiB  | 217.96 MiB - 218.09 MiB │ Skipped                                    │
│ Delete and copy   │ 1.10s   - 1.64s  | 21.57 MiB  - 23.70 MiB | 229.96 MiB - 230.08 MiB │ Skipped                                    │
│ Single large file │ 3.34s   - 3.97s  | 226.77 MiB - 227.41 MiB| 215.95 MiB - 215.95 MiB │ 5.70s   - 5.89s  | 7.48 MiB   - 7.51 MiB   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┘────────────────────────────────────────────┘

Linux -> Linux
┌───────────────────┬────────────────────────────────────────────┐───────────────────┬───────────────────┬───────────────────┬───────────────────┐
│ Test case         │ rjrssync (x10)                             │ rsync (x10)       │ scp (x10)         │ cp (x10)          │ APIs (x10)        │
├───────────────────┼────────────────────────────────────────────┤───────────────────┼───────────────────┼───────────────────┼───────────────────┤
│ Everything copied │ 145ms   - 934ms  | 347.98 MiB - 347.98 MiB │ 195ms   - 204ms   │ 89ms    - 104ms   │ 89ms    - 108ms   │ 97ms    - 150ms   │
│ Nothing copied    │ 15ms    - 88ms   | 351.43 MiB - 351.64 MiB │ 25ms    - 32ms    │ Skipped           │ Skipped           │ Skipped           │
│ Some copied       │ 205ms   - 1.00s  | 361.46 MiB - 361.57 MiB │ 34ms    - 36ms    │ Skipped           │ Skipped           │ Skipped           │
│ Delete and copy   │ 170ms   - 827ms  | 362.02 MiB - 366.11 MiB │ 244ms   - 250ms   │ Skipped           │ Skipped           │ Skipped           │
│ Single large file │ 489ms   - 889ms  | 347.98 MiB - 475.97 MiB │ 1.85s   - 1.89s   │ 434ms   - 537ms   │ 433ms   - 544ms   │ 399ms   - 620ms   │
└───────────────────┴────────────────────────────────────────────┘───────────────────┴───────────────────┴───────────────────┴───────────────────┘

Linux -> /mnt/...
┌───────────────────┬────────────────────────────────────────────┐───────────────────┬───────────────────┬───────────────────┬───────────────────┐
│ Test case         │ rjrssync (x10)                             │ rsync (x10)       │ scp (x10)         │ cp (x10)          │ APIs (x10)        │
├───────────────────┼────────────────────────────────────────────┤───────────────────┼───────────────────┼───────────────────┼───────────────────┤
│ Everything copied │ 14.50s  - 16.91s | 347.98 MiB - 347.98 MiB │ 42.40s  - 46.95s  │ 16.47s  - 18.64s  │ 16.75s  - 18.37s  │ 16.51s  - 18.53s  │
│ Nothing copied    │ 15.69s  - 16.73s | 381.70 MiB - 478.00 MiB │ 25.93s  - 27.84s  │ Skipped           │ Skipped           │ Skipped           │
│ Some copied       │ 15.51s  - 16.76s | 381.73 MiB - 478.00 MiB │ 25.57s  - 27.93s  │ Skipped           │ Skipped           │ Skipped           │
│ Delete and copy   │ 26.45s  - 29.08s | 382.23 MiB - 382.27 MiB │ 51.65s  - 57.16s  │ Skipped           │ Skipped           │ Skipped           │
│ Single large file │ 3.48s   - 4.53s  | 568.06 MiB - 568.06 MiB │ 5.52s   - 6.44s   │ 4.75s   - 5.77s   │ 4.83s   - 5.83s   │ 4.37s   - 6.45s   │
└───────────────────┴────────────────────────────────────────────┘───────────────────┴───────────────────┴───────────────────┴───────────────────┘

Linux -> Remote Windows
┌───────────────────┬─────────────────────────────────────────────────────────────────────┐───────────────────┐
│ Test case         │ rjrssync (x10)                                                      │ scp (x10)         │
├───────────────────┼─────────────────────────────────────────────────────────────────────┤───────────────────┤
│ Everything copied │ 3.37s   - 3.74s  | 428.75 MiB - 480.01 MiB| 44.16 MiB  - 54.05 MiB  │ 5.35s   - 6.74s   │
│ Nothing copied    │ 422ms   - 756ms  | 417.62 MiB - 480.01 MiB| 6.69 MiB   - 7.00 MiB   │ Skipped           │
│ Some copied       │ 3.13s   - 3.76s  | 429.65 MiB - 480.01 MiB| 44.78 MiB  - 52.97 MiB  │ Skipped           │
│ Delete and copy   │ 3.73s   - 4.42s  | 480.01 MiB - 480.01 MiB| 54.68 MiB  - 55.22 MiB  │ Skipped           │
│ Single large file │ 2.93s   - 3.13s  | 632.02 MiB - 700.09 MiB| 18.84 MiB  - 19.12 MiB  │ 4.66s   - 5.41s   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┘───────────────────┘

Linux -> Remote Linux
┌───────────────────┬─────────────────────────────────────────────────────────────────────┐───────────────────┬───────────────────┐
│ Test case         │ rjrssync (x10)                                                      │ rsync (x10)       │ scp (x10)         │
├───────────────────┼─────────────────────────────────────────────────────────────────────┤───────────────────┼───────────────────┤
│ Everything copied │ 257ms   - 440ms  | 428.77 MiB - 480.01 MiB| 215.95 MiB - 279.95 MiB │ 294ms   - 379ms   │ 1.48s   - 1.83s   │
│ Nothing copied    │ 36ms    - 102ms  | 417.52 MiB - 480.01 MiB| 217.96 MiB - 271.53 MiB │ 35ms    - 50ms    │ Skipped           │
│ Some copied       │ 281ms   - 417ms  | 429.62 MiB - 480.01 MiB| 229.96 MiB - 232.97 MiB │ 53ms    - 70ms    │ Skipped           │
│ Delete and copy   │ 239ms   - 381ms  | 429.74 MiB - 480.01 MiB| 232.97 MiB - 234.79 MiB │ 339ms   - 545ms   │ Skipped           │
│ Single large file │ 1.73s   - 1.91s  | 608.00 MiB - 676.00 MiB| 215.95 MiB - 279.95 MiB │ 3.82s   - 4.58s   │ 4.27s   - 5.10s   │
└───────────────────┴─────────────────────────────────────────────────────────────────────┘───────────────────┴───────────────────┘

```

Notes on filters
================

Currently, the same filter is applied on both source and dest sides and there is no way to have a different filter on each side. This is simpler, but means that if you run a sync which copies some files you forgot to exclude, then add the exclude and re-run the sync, those files will still be present on the dest (but just hidden by the filter). So you would need to manually remove them which isn't great. If we allowed separate source/dest filters, then you could exclude the files just on the source and then they would be removed from the dest. However, having separate filters could lead to other potential issues - if you exclude some files on the dest only, and those files do exist on the source, then they will be copied every time regardless. Perhaps files should only be excludable on the source, or on both, but never just on the dest? Or perhaps a file should never be copied to the dest, if it would be excluded by the dest filter?

Notes on remote deployment
==========================

There are several methods we (could) use to get rjrssync onto remote devices:
 * Log on to the remote device and "install" it (apt, cargo install, cargo binstall etc., either as a binary download or build from source)
 * Upload a pre-built binary
 * Upload the source code and build it using cargo on the remote device

Each of these has advantages/disadvantages. Some properties that would be nice are:

Easy to develop - new versions need to be quick to deploy for iterative development.

All binaries should be "equal" - if we have some binaries that can be used to deploy to remote platforms ("big") and others that can't ("lite"), then this is confusing and hard to keep track of.

Quick and easy to deploy to a new platform. If it takes 10 mins to build from source then that is annoying, or if you need to set up a bunch of build environment first that is tricky.

A build from source should be as simple as "cargo build" - ideally we don't have any post-build steps or custom wrapper scripts.

For the targets that we include as embedded binaries:

We prefer to always have the exact same set, no matter the platform that we are building on.
This means we can't use -msvc for the windows target for example, as this isn't available when building from Linux. Even though the "outer binary" (the code that actually runs) will be built with MSVC when building on Windows, the embedded binaries payload will still be -gnu. We could choose to additionally/instead always include the platform that we are currently targeting, but this would lead to differences in the embedded binaries which could be confusing. Of course somebody can get the source code and build rjrssync for any target they want and run it there, but it will only ever be deployable onto targets that are in the predefined list, so we want this to always be consistent. If somebody wants to deploy onto another platform, they'd have to add it to the list and rebuild, or manually "install" rjrssync on that target, doing a build from source.

Maybe if we have a "rjrssync-lite" name for lite binaries, this will be enough to distinguish them. This might lift some of the restrictions? Getting the environment set up to build all of the embedded binaries on all platforms is difficult, e.g. mingw doesn't support Windows on Arm, so I'm not sure if there is a way to build for Windows on Arm from Linux. Getting mingw to work for a cross-build seems to require downloading mingw binaries separately (e.g. apt install mingw-w64 on Linux), which is a pain.

Unfortuntately this means that it's infeasible to require the same set of embedded binaries in every build as it would be too restrictive for how the software can be built. Ultimately the software is deployed via source code, so people can choose to modify it in any way they want (including changing the embedded binaries), so trying to enforce these restrictions isn't really possible. If we were making a binary distribution, we could make sure that contained a certain set of embedded binaries, but we're not (yet). We don't want to force people to have to set up a build environment to cross compile to a bunch of targets that they don't care about, so we allow this to be customised by the person building the software. We have a --list-embedded-binaries option to make it easier to see what remote targets a particular binary supports.

Note that we do need to include the lite binary for the native build, as this will be needed if the big binary is used to produce a new big binary for a different platform - that new big binary will need to have the lite binary for the native platform. Technically we could get this by downgrading the big binary to a lite binary before embedding it, but this would be more complicated.

Decided to remove the source deploy option because it would be more to maintain alongside the binary deploy, and the use cases for it for quite minimal now that the binary deploy is working. The main use case is for running on a platform that we don't have a pre-built binary for, which should be quite rare and there are other options - add an embedded binary for that platform, or do a special lite binary build just for that platform and copy it into the expected location. Also the source build will be even slower if it includes building embedded binaries, and harder to set up because you need a more complex build environment.

One option is that remote source builds should just build lite binaries, for speed, but then it breaks the nice property that all binaries seen on disk are equal (big) - we don't want some binaries to not support (binary) deployment as this will be confusing ("which binary do i have").
But it does mean that users would need to set up all the rustup cross-compilation targets on their remote platform.

On Windows, we could embed the lite binaries as proper "resources" (using .rc file etc.), but this isn't a thing on Linux, so we choose to use the same approach for both and so don't use this Windows feature. Instead we append the embedded binaries as sections in the final binary (.exe/.elf) (both platforms have the concept of sections in their executable formats). Because we'll need to manipulate the binaries anyway at runtime when building a new big binary, we're gonna need to mess around with the sections anyway, and making them work as resources is more work.

Ideas for reducing binary size: https://github.com/johnthagen/min-sized-rust

Stripping symbols helped quite a lot (~50% reduction)
Optimising for speed rather than size helped reasonably as well, but might have perf effects so didn't do this
LTO helped a little bit, and might produce better code too so turned it on.

Sizes here built from windows (in release!)

original:

x86_64-pc-windows-msvc (3.31 MiB)
x86_64-unknown-linux-musl (8.68 MiB)
aarch64-unknown-linux-musl (8.65 MiB)

with strip

x86_64-pc-windows-msvc (3.30 MiB)
x86_64-unknown-linux-musl (3.59 MiB)
aarch64-unknown-linux-musl (2.90 MiB)

with strip + optimize for size

x86_64-pc-windows-msvc (2.64 MiB)
x86_64-unknown-linux-musl (3.22 MiB)
aarch64-unknown-linux-musl (2.65 MiB)

with strip + optimize for size + lto:

x86_64-pc-windows-msvc (2.29 MiB)
x86_64-unknown-linux-musl (2.48 MiB)
aarch64-unknown-linux-musl (1.99 MiB)

with strip + lto (optimize for speed, not size):

x86_64-pc-windows-msvc (3.19 MiB)
x86_64-unknown-linux-musl (3.07 MiB)
aarch64-unknown-linux-musl (2.41 MiB)

Compression of embedded binaries also helps quite a bit (~50% reduction in embedded binaries). Possibly more could be gained by a better compression format (zstd, brotli, lzma?)

We can avoid including a lite binary for the platform that is the outer binary, as we can extract this instead

Documentation (README.md, --help, etc.)
=======================================

The output of --help is quite long and verbose, and yet is still missing a lot of information. This might not be the best place to put a lot of detailed documentation, and instead be used as more of a short reference guide? We could have a short and a long version of --help, which clap does support. Alternatively we could put more information in the README.md, as this is easier to read (e.g. in a browser, not a terminal window). However the README isn't part of the built program, and therefore a user might not have access to it. We can probably assume that they have internet access though, and they probably got the program via GitHub or crates.io (which both show the README), so we link to the README in the --help. This link isn't great though because the version on GitHub won't necessarily be the same as the version the user has a binary of.

Even if we include all the information we need in the --help, it's still good to have stuff in the README because that's the "advertisement" for the program that people would see before they have downloaded it. We don't want people to have to download and build it just to see what kind of features it has.