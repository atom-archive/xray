# Xray Server

This crate is an executable that runs as a server process. It can be run in a headless mode in order to host workspaces for remote clients, and it is also spawned by `xray_electron`, which provides the application with a user interface and communicates with the server over a domain socket.
