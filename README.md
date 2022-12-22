About
=====

Fast rsync-like tool for incrementally copying files. Runs natively on both Windows and Linux and uses network for communication, to maximise speed when syncing between Windows and WSL filesystems.

Installation
============

1. Download this repository
2. `cargo install --path . --locked`

Usage
=====

A quick example:

```
rjrssync local-folder/ user@hostname:/remote/folder
```

See `rjrssync --help` for more.
