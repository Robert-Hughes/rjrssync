Fast rsync-like tool for incrementally copying files. Runs natively on both Windows and Linux and uses network for communication, 
to maximise speed syncing between Windows and WSL filesystems (accessing WSL filesytem from Windows or vice-versa is slow).

TODO:

* Config file containing src/dest, ignore/allow list etc. Use serde_json?
* Security! We're opening ports that allow people to control our computers!
* Network port specified as command line arg or other configuration variable?
