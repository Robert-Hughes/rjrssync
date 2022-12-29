About
=====

Fast rsync-like tool for incrementally copying files. Runs natively on both Windows and Linux and uses network for communication, to maximise speed when syncing between Windows and WSL filesystems.

Installation
============

```
cargo install rjrssync
```

Usage
=====

A quick example:

```
rjrssync local-folder/ user@hostname:/remote/folder
```

The first time that a remote host is used, rjrssync will deploy its source code to the remote host and use `cargo` to build it for that platform. Therefore `cargo` needs to be installed and available on the remote host. It will take some time for this initial build.

See `rjrssync --help` for more.
