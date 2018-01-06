# Xray

This is an experiment in using the fundamental concepts of the Teletype CRDT as Atom's core text-storage data structure. It's implemented in Rust.

The idea is to use a thread-safe copy-on-write b-tree to store the document fragments. We also want to investigate using interval trees of logical positions as an implementation of the marker index.

## Building

This project produces a library which is designed to be loaded into a Node.js executable as a shared library. Because it looks up symbols from Node dynamically, it cannot be built with cargo directly without additional linker flags. See `scripts/build.js` for details.

This project depends on the [`collider`](https://github.com/atom/collider) crate, (which provides a safe interface to Node's N-API. Currently, `collider` is expected to be present as a sibling directory until I take the time to set it up more correctly.
