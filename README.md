Fast rsync-like tool for incrementally copying files. Runs natively on both Windows and Linux and uses network for communication, 
to maximise speed syncing between Windows and WSL filesystems (accessing WSL filesytem from Windows or vice-versa is slow).

TODO:

* Config file containing src/dest, ignore/allow list etc. Use serde_json?