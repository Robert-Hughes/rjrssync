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

Disabling PAM on sshd_config seems to speed up ssh login https://serverfault.com/questions/792486/ssh-connection-takes-forever-to-initiate-stuck-at-pledge-network


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
        with a folder/file. We should probably warn for this.

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

Benchmarking results
=======================

`cargo bench`, see benchmarks.rs

From Windows
------------

Local -> Local
┌──────────────┬───────────────────┬────────────────┬─────────────┬───────────────────┐
│ Method       │ Everything copied │ Nothing copied │ Some copied │ Single large file │
├──────────────┼───────────────────┼────────────────┼─────────────┼───────────────────┤
│ rjrssync.exe │ 4.6861765s        │ 165.1569ms     │ 255.2879ms  │ 1.9668654s        │
│ scp          │ 3.7587507s        │ 3.2117669s     │ 2.5626346s  │ 437.0342ms        │
│ xcopy        │ 2.7802775s        │ 2.341118s      │ 2.2331518s  │ 395.7386ms        │
│ robocopy     │ 2.2889186s        │ 95.7522ms      │ 241.5638ms  │ 396.1048ms        │
│ APIs         │ 2.5209556s        │ 1.7566445s     │ 1.8727899s  │ 362.7545ms        │
└──────────────┴───────────────────┴────────────────┴─────────────┴───────────────────┘

Local -> \\wsl$\...
┌──────────────┬───────────────────┬────────────────┬─────────────┬───────────────────┐
│ Method       │ Everything copied │ Nothing copied │ Some copied │ Single large file │
├──────────────┼───────────────────┼────────────────┼─────────────┼───────────────────┤
│ rjrssync.exe │ 9.0569208s        │ 523.1353ms     │ 604.4056ms  │ 1612.871946s      │
│ scp          │ 23.9899789s       │ 24.0248723s    │ 24.4265919s │ 3.3814891s        │
│ xcopy        │ 24.0942546s       │ 24.3750163s    │ 24.8903191s │ 3.1909175s        │
│ robocopy     │ 12.9143492s       │ 934.8254ms     │ 1.0275249s  │ 2.9421081s        │
│ APIs         │ 9.1405191s        │ 9.0561342s     │ 8.8848025s  │ 3.2530861s        │
└──────────────┴───────────────────┴────────────────┴─────────────┴───────────────────┘

Local -> Remote Windows
┌──────────────┬───────────────────┬────────────────┬─────────────┬───────────────────┐
│ Method       │ Everything copied │ Nothing copied │ Some copied │ Single large file │
├──────────────┼───────────────────┼────────────────┼─────────────┼───────────────────┤
│ rjrssync.exe │ 4.3033501s        │ 711.326ms      │ 400.1021ms  │ 10.3783916s       │
│ scp          │ 5.5332449s        │ 5.1707648s     │ 4.9287981s  │ 5.0944189s        │
└──────────────┴───────────────────┴────────────────┴─────────────┴───────────────────┘

Local -> Remote Linux
┌──────────────┬───────────────────┬────────────────┬─────────────┬───────────────────┐
│ Method       │ Everything copied │ Nothing copied │ Some copied │ Single large file │
├──────────────┼───────────────────┼────────────────┼─────────────┼───────────────────┤
│ rjrssync.exe │ 19.9767491s       │ 451.4564ms     │ 647.1778ms  │ 10.8559916s       │
│ scp          │ 14.7192943s       │ 18.3526111s    │ 18.3817042s │ 5.8270207s        │
└──────────────┴───────────────────┴────────────────┴─────────────┴───────────────────┘

From Linux
-----------

TODO: Single large file for Linux looks wrong! Re-run it

Local -> Local
┌──────────┬───────────────────┬────────────────┬─────────────┬───────────────────┐
│ Method   │ Everything copied │ Nothing copied │ Some copied │ Single large file │
├──────────┼───────────────────┼────────────────┼─────────────┼───────────────────┤
│ rjrssync │ 661.4886ms        │ 57.5216ms      │ 442.0045ms  │ 626.79ms          │
│ rsync    │ 218.8317ms        │ 26.5807ms      │ 159.0995ms  │ 206.9003ms        │
│ scp      │ 97.6563ms         │ 97.3493ms      │ 142.8372ms  │ 108.1592ms        │
│ cp       │ 102.4808ms        │ 94.2193ms      │ 93.8572ms   │ 103.9471ms        │
│ APIs     │ 124.1922ms        │ 146.6863ms     │ 146.2064ms  │ 113.1608ms        │
└──────────┴───────────────────┴────────────────┴─────────────┴───────────────────┘

Local -> /mnt/...
┌──────────┬───────────────────┬────────────────┬─────────────┬───────────────────┐
│ Method   │ Everything copied │ Nothing copied │ Some copied │ Single large file │
├──────────┼───────────────────┼────────────────┼─────────────┼───────────────────┤
│ rjrssync │ 25.1078243s       │ 29.2568121s    │ 26.1036976s │ 27.3555689s       │
│ rsync    │ 45.8593816s       │ 24.7783698s    │ 24.4049877s │ 45.719907s        │
│ scp      │ 18.8023797s       │ 21.6127126s    │ 18.3937744s │ 17.0488304s       │
│ cp       │ 15.6981755s       │ 19.6974815s    │ 17.6403505s │ 15.912663s        │
│ APIs     │ 17.2637453s       │ 17.330068s     │ 15.478452s  │ 18.6760742s       │
└──────────┴───────────────────┴────────────────┴─────────────┴───────────────────┘

Local -> Remote Windows
┌──────────┬───────────────────┬────────────────┬─────────────┬───────────────────┐
│ Method   │ Everything copied │ Nothing copied │ Some copied │ Single large file │
├──────────┼───────────────────┼────────────────┼─────────────┼───────────────────┤
│ rjrssync │ 6.1132198s        │ 315.4216ms     │ 4.2337818s  │ 5.6314544s        │
│ scp      │ 5.8013899s        │ 5.5714418s     │ 5.5500562s  │ 5.5162344s        │
└──────────┴───────────────────┴────────────────┴─────────────┴───────────────────┘

Local -> Remote Linux
┌──────────┬───────────────────┬────────────────┬─────────────┬───────────────────┐
│ Method   │ Everything copied │ Nothing copied │ Some copied │ Single large file │
├──────────┼───────────────────┼────────────────┼─────────────┼───────────────────┤
│ rjrssync │ 1.0982752s        │ 394.1328ms     │ 1.160108s   │ 1.181368s         │
│ rsync    │ 576.7668ms        │ 336.713ms      │ 548.8059ms  │ 592.6911ms        │
│ scp      │ 1.7771327s        │ 1.7964356s     │ 1.7862826s  │ 1.7711581s        │
└──────────┴───────────────────┴────────────────┴─────────────┴───────────────────┘

Notes on filters
================

Currently, the same filter is applied on both source and dest sides and there is no way to have a different filter on each side. This is simpler, but means that if you run a sync which copies some files you forgot to exclude, then add the exclude and re-run the sync, those files will still be present on the dest (but just hidden by the filter). So you would need to manually remove them which isn't great. If we allowed separate source/dest filters, then you could exclude the files just on the source and then they would be removed from the dest. However, having separate filters could lead to other potential issues - if you exclude some files on the dest only, and those files do exist on the source, then they will be copied every time regardless. Perhaps files should only be excludable on the source, or on both, but never just on the dest? Or perhaps a file should never be copied to the dest, if it would be excluded by the dest filter?