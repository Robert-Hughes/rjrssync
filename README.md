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


Example spec file
-----------------

For use with `--spec`

```
# Note that if no src_hostname is specified, then the respective src path is assumed to be local.
# The same goes for dest.
src_hostname: computer1
src_username: root
dest_hostname: computer2
dest_username: myuser
# Multiple paths can be synced
syncs:
  - src: D:/Source
    dest: D:/Dest
    # Filters are regular expressions with a leading '+' or '-', indicating includes or excludes.
    filter: [ "+.*\.txt", "-garbage\.txt" ]
  - src: D:/Source2
    dest: D:/Dest2
```


