# Proton

This is an experiment in using the fundamental concepts of the Teletype CRDT as Atom's core text-storage data structure. It's implemented in Rust.

The idea is to use a thread-safe copy-on-write b-tree to store the document fragments. We also want to investigate using interval trees of logical positions as an implementation of the marker index.
