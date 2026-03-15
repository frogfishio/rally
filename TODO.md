# TODO

- [x] add support for configuring before, after (what happens before you strat an app, and after e.g. prep, cleanup)
- [x] add support for config reloading without restarting rally
- [x] add support for "depends on" so they processes are started (and brought down) in correct sequence
- [x] add ENV interpolation 
- [x] add suppport for pkg:cargo/ratatouille@0.1.0, send all messages there
- [x] add command line for teling where the sink is (url)... keep in mind that the sink may be one of the apps running, so it might not be available until we start it --sink http://..
- [ ] after pkg:cargo/ratatouille@0.1.0 is implemented for the app, add support for capturing stdout/stderr from running apps and forwarding them to sink
- [ ] add watch ability if a config for a running file has changed or the binary has changed restart it so we need to have an optional watch group for the tasks
- [ ] add world-class command line wit --help etc
- [ ] add command line config file specifying where to load it from