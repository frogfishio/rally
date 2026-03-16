# TODO

- [x] add support for configuring before, after (what happens before you strat an app, and after e.g. prep, cleanup)
- [x] add support for config reloading without restarting rally
- [x] add support for "depends on" so they processes are started (and brought down) in correct sequence
- [x] add ENV interpolation 
- [x] add suppport for pkg:cargo/ratatouille@0.1.0, send all messages there
- [x] add command line for teling where the sink is (url)... keep in mind that the sink may be one of the apps running, so it might not be available until we start it --sink http://..
- [x] after pkg:cargo/ratatouille@0.1.0 is implemented for the app, add support for capturing stdout/stderr from running apps and forwarding them to sink
- [x] add watch ability if a config for a running file has changed or the binary has changed restart it so we need to have an optional watch group for the tasks
- [x] add world-class command line wit --help etc
- [x] add command line config file specifying where to load it from
- [x] add one more item to TOML file: "access". This served little functional value but shows access in a dashboard instead of the exact line we used to start the app.

why? Some apps have access url/port/setup and it's hard to remember what is what. 

so: 

- if access is defined then put it there. If not leave it as it is. It is an optional overrid
- recognise if the acces has URL pattern and if so then turn it into a link which opens new tab, for easier accss
- [x] rally looks for a file rally.toml unless specified differently. Please add also RALLY_CONFIG optional environment variable to look at
- [x] we should also add commands to rally that connect to an existing rally instance rather than starting new one: start, stop, restart. This is so we can control it from local scripts on dev machine, also 2 more commands: enable/disable which set the TOML enabled=false or true
- [x] add TOML enabled flag. it is optional so by default is is true (enabled). We might want to have this level of programmatic control so to say enable/disable as we are doing automated testing. NOTE: enable/disable doesn't respect dependencies. It only works on one. Why? we might want to test what happens when a DB is gone and a service layer still expects it. It is a dynamic flag, even if not set in TOML... it is stil there defaulting to true. Meaning, a restart of the server without explicit false will return it to original true state
- [x] add \[env] support in the TOML where we can add environment variables for running apps. This is the missing part. Also update the GUI to show the env vars that are in play
- [x] add cargo option into TOML. If app binary is not reachable (doesnt exist) and cargo is set, then fork a call to cargo install. While it's doing that, set status to installing so people looking at the GUI know what's happening. Then after it's installed we can run it. This is a nifty way to package a set of runtime apps with your own apps (apps you don't link to)


