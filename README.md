# Xray

This is an *experiment* (that may go nowhere) in a new design for an Electron-based text editor which intends to explore the following ideas:

## Prefer compiled code

The core of the application is written in Rust. The core is a pure Rust library that lives in `xray_core`. The core is then exposed to JavaScript as a native add-on for Node.js in `xray_core_node`.

## Store text in a CRDT and maximize parallelism

The buffer representation builds on what we learned with the Teletype CRDT, representing the document as a sequence of fragments in a persistent copy-on-write B+ tree. This should have a number of benefits.

* **No need for a marker index.** Rather than maintaining the positions of markers on every edit, we can use abstract positions to maintain stable references into the text. Abstract positions can be stored in an interval tree for cases where we need to perform efficient stabbing queries, but such a tree would not need to be modified on every edit.
* **Background packages.** CRDTs allow multiple threads to concurrently modify a buffer, which should make it possible to give packages synchronous write access to buffers even if they are running on background threads.
* **Maximal parallelism.** The copy-on-write data-structure makes cloning a buffer a constant-time operation. The CRDT means that abstract positions computed on a background thread remain valid when moved to the main thread, even if the buffer has changed in the meantime.

## Prioritize performance, subjugate web technology

Atom embraces web technology. Xray aims to subjugate it. While we still plan to give package authors maximal freedom to exploit all features of the web-based environment, the DOM will be treated more as an implementation detail rather than as a foundational abstraction. We aim to minimize leakage of any DOM-related concepts into our public APIs, with the possible exception of CSS for theming.

We want to avoid the DOM for rendering the text editor, for example, instead using a canvas, WebGL, or any other approach that can give us extremely good performance. This may make syntax themes slightly more difficult to build, and we accept that trade-off.

Focus on minimizing startup time from the beginning and don't add *anything* that degrades it.

## Static typing, versioning, and minimalism

Version APIs wherever possible to allow evolution without breakage. Use static typing wherever possible to clarify interfaces, improve documentation, and minimize the chances of inadvertent breakage. In core, expose the *minimal* API needed to get the job done, pushing other concerns out to libraries.
