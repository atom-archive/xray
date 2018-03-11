# Xray

Xray is an experimental Electron-based text editor informed by what we've learned in the four years since the launch of Atom. In the short term, this project is a testbed for rapidly iterating on several radical ideas without risking the stability of Atom. The longer term future of the code in this repository will become clearer after a few months of progress. For now, our primary goal is to iterate rapidly and learn as much as possible.

## Updates

* [March 5, 2018](./docs/updates/2018_03_05.md)

## Foundational priorities

Our goal is to build a cross-platform text editor that is designed from the beginning around the following foundational priorities:

### High performance

*Xray feels lightweight and responsive.*

We design our features to be responsive from the beginning. We reliably provide visual feedback within the latency windows suggested by the [RAIL performance model](https://developers.google.com/web/fundamentals/performance/rail). For all interactions, we shoot for the following targets on the hardware of our median user:

| Duration | Action |    
| - | - |
| 8ms | Scrolling, animations, and fine-grained interactions such as typing or cursor movement. |
| 50ms | Coarse-grained interactions such as opening a file or initiating a search. If we can't complete the action within this window, we should show a progress bar. |
| 150ms | Opening an application window. |

We are careful to maximize throughput of batch operations such as project-wide search. Memory consumption is kept within a low constant factor of the size of the project and open buffer set, but we trade memory for speed and extensibility so long as memory requirements are reasonable.

### Collaboration

*Xray makes it as easy to code together as it is to code alone.*

We design features for collaborative use from the beginning. Editors and other relevant UI elements are designed to be occupied by multiple users. Interactions with the file system and other resources such as subprocesses are abstracted to work over network connections.

### Extensibility

*Xray gives developers control over their own tools.*

We expose convenient and powerful APIs to enable users to add non-trivial functionality to the application. We balance the power of our APIs with the ability to ensure the responsiveness, stability, and security of the application as a whole. We avoid leaking implementation details and use versioning where possible to enable a sustained rapid development without destabilizing the package ecosystem.

### Web compatibility

*Editing on GitHub feels like editing in Xray.*

We provide a feature-rich editor component that can be used on the web and within other Electron applications. This will ultimately help us provide a more unified experience between GitHub.com and this editor and give us a stronger base of stakeholders in the core editing technology. If this forces serious performance compromises we may potentially drop this objective, but we don't think that it will.

## Architecture

Martin Fowler defines software architecture as those decisions which are both important and hard to change. Since these decisions are hard to change, we need to be sure that our foundational priorities are well-served by these decisions.

![Architecture](docs/images/architecture.png)

### The UI is built with Electron

Electron adds a lot of overhead, which detracts from our top priority of high-performance. However, Electron is also the best approach that we know of to deliver a cross-platform, extensible user interface. Atom proved that developers want to add non-trivial UI elements to their editor, and we still see web technologies as the most viable way to offer them that ability. We also want to provide extension authors with a scripting API, and the JavaScript VM that ships with Electron is well suited to that task.

The fundamental question is whether we can gain Electron's benefits for extensibility while still meeting our desired performance goals. Our hypothesis is that it's possible–with the right architecture.

### Core application logic is written in Rust

The core of the application is implemented in a pure Rust crate (`/xray_core`) and made accessible to JavaScript through N-API bindings (`/xray_node`). This module is loaded into the Electron application (`/xray_electron`) via Node's native add-on system. The binding layer will be responsible for exposing a thread-safe API to JS so that the same native module can be used in the render thread and worker threads.

All of the core application code other than the view logic should be written in Rust. This will ensure that it has a minimal footprint to load and execute, and Rust's robust type system will help us maintain it more efficiently than dynamically typed code. A language that is fundamentally designed for multi-threading will also make it easier to exploit parallelism whenever the need arises, whereas JavaScript's single-threaded nature makes parallelism awkward and challenging.

Fundamentally, we want to spend our time writing in a language that is fast by default. It's true that it's possible to write slow Rust, and also possible to write fast JavaScript. It's *also* true that it's much harder to write slow Rust than it is to write slow JavaScript. By spending fewer resources on the implementation of the platform itself, we'll make more resources available to run package code.

### Packages will run primarily in worker threads

A misbehaving package should not be able to impact the responsiveness of the application. The best way to guarantee this while preserving ease of development is to activate packages on worker threads. We can do a worker thread per package or run packages in their own contexts across a pool of threads.

Packages *can* run code on the render thread by specifying versioned components in their `package.json`.

```json
"components": {
  "TodoList": "./components/todo-list.js"
}
```

If a package called `my-todos` had the above entry in its `package.json`, it could request that the workspace attach that component by referring to `myTodos.TodoList` when adding an item. On package installation, we can automatically update the V8 snapshot to include the components of every installed package. Components will only be dynamically loaded from the provided paths in development mode.

APIs for interacting with the core application state and the underlying operating system will only be available within worker threads, discouraging package authors from putting too much logic into their views. We'll use a combination of asynchronous channels and CRDTs to present convenient APIs to package authors within worker threads.

### Text is stored in a copy-on-write CRDT

To fully exploit Rust's unique advantage of parallelism, we need to store text in a concurrency-friendly way. We use a variant of RGA called RGASplit, which is described in [this research paper](https://pages.lip6.fr/Marc.Shapiro/papers/rgasplit-group2016-11.pdf).

![CRDT diagram](docs/images/crdt.png)

In RGA split, the document is stored as a sequence of insertion fragments. In the example above, the document starts as just a single insertion containing `hello world`. We then introduce `, there` and `!` as additional insertions, splitting the original insertion into two fragments. To delete the `ld` at the end of `world` in the third step, we create another fragment containing just the `ld` and mark it as deleted with a tombstone.

Structuring the document in this way has a number of advantages.

* Real-time collaboration works out of the box
* Concurrent edits: Any thread can read or write its own replica of the document without diverging in the presence of concurrent edits.
* Integrated non-linear history: To undo any group of operations, we increment an undo counter associated with any insertions and deletions that controls their visibility. This means we only need to store operation ids in the history rather than operations themselves, and we can undo any operation at any time rather than adhering to historical order.
* Stable logical positions: Instead of tracking the location of markers on every edit, we can refer to stable positions that are guaranteed to be valid for any future buffer state. For example, we can mark the positions of all search results in a background thread and continue to interpret them in a foreground thread if edits are performed in the meantime.

Our use of a CRDT is similar to the Xi editor, but the approach we're exploring is somewhat different. Our current understanding is that in Xi, the buffer is stored in a rope data structure, then a secondary layer is used to incorporate edits. In Xray, the fundamental storage structure of all text is itself a CRDT. It's similar to Xi's rope in that it uses a copy-on-write B-tree to index all inserted fragments, but it does not require any secondary system for incorporating edits.

### Derived state will be computed asynchronously

We should avoid implementing synchronous APIs that depend on open-ended computations of derived state. For example, when soft wrapping is enabled in Atom, we synchronously update a display index that maps display coordinates to buffer coordinates, which can block the UI.

In Xray, we want to avoid making these kinds of promises in our API. For example, we will allow the display index to be accessed synchronously after a buffer edit, but only provide an interpolated version of its state that can be produced in logarithmic time. This means it will be spatially consistent with the underlying buffer, but may contain lines that have not yet been soft-wrapped.

We can expose an asynchronous API that allows a package author to wait until the display layer is up to date with a specific version of the buffer. In the user interface, we can display a progress bar for any derived state updates that exceed 50ms, which may occur when the user pastes multiple megabytes of text into the editor.

### System interaction will be centralized in a "workspace server"

File system interactions will be routed through a central server called the *workspace server*.

The server will serialize buffer loads and saves on a per-path basis, and maintains a persistent database of CRDT operations for each file. As edits are performed in windows, they will be streamed to the host process to be stored and echoed out to any other windows with the same open buffer. This will enable unsaved changes to always be incrementally preserved in case of a crash or power failure and preserves the history associated with a file indefinitely.

Early on, we should design the application process to be capable of connecting to multiple workspace servers to facilitate real-time collaboration or editing files on a remote server by running a headless host process. To support these use cases, we should prefer implementing most code paths that touch the file system or spawn subprocesses in the host process and interacting with them via RPC.

### React will be used for presentation

By using React, we completely eliminate the view framework as a concern that we need to deal with and give package authors access to a tool they're likely to be familiar with. We also raise the level of abstraction above basic DOM APIs. The risk of using React is of course that it is not standardized and could have breaking API changes. To mitigate this risk, we will require packages to declare which version of React they depend on. We will attempt use this version information to provide shims to older versions of React when we upgrade the bundled version. When it's not possible to shim breaking changes, we'll use the version information to present a warning.

### Styling will be specified in JS

CSS is a widely-known and well-supported tool for styling user interfaces, which is why we embraced it in Atom. Unfortunately, the performance and maintainability of CSS degrade as the number of selectors increases. CSS also lacks good tools for exposing a versioned theming APIs and applying programmatic logic such as altering colors. Finally, the browser does not expose APIs for being notified when computed styles change, making it difficult to use CSS as a source of truth for complex components. For a theming system that performs well and scales, we need more direct control. We plan to use a CSS-in-JS approach that automatically generates atomic selectors so as to keep our total number of selectors minimal.

### Text is rendered via WebGL

In Atom, the vast majority of computation of any given frame is spent manipulating the DOM, recalculating styles, and performing layout. To achieve good text rendering performance, it is critical that we bypass this overhead and take direct control over rendering. Like Alacritty and Xi, we plan to employ OpenGL to position quads that are mapped to glyph bitmaps in a texture atlas.

We plan to use HarfBuzz to determine an accurate mapping between character sequences and glyphs, since one character does not always correspond to one glyph. Once we identify clusters of characters corresponding to glyphs, we'll rasterize glyphs via an HTML 5 canvas to ensure we use the appropriate text rasterization method for the current platform, then upload them to the texture atlas on the GPU to be referenced in shaders.

Bypassing the DOM means that we'll need to implement styling and text layout ourselves. That is a high price to pay, but we think it will be worth it to bypass the performance overhead imposed by the DOM.

## Development process

### Experiment

At this phase, this code is focused on learning. Whatever code we write should be production-quality, but we don't need to support everything at this phase. We can defer features that don't contribute substantially to learning.

### Documentation-driven development

Before coding, we ask ourselves whether the code we're writing can be motivated by something that's written in the guide. The right approach here will always be a judgment call, but lets err on the side of transparency and see what happens. See [About this guide]() for more details.

### Disciplined monorepo

All code related to Xray should live in this repository, but intra-repository dependencies should be expressed in a disciplined way to ensure that a one-line docs change doesn't require us to rebuild the world. Builds should be finger-printed on a per-component basis and we should aim to keep components granular.

### Community SLA

Well-formulated PRs and issues will receive some form of response by the end of the next business day. If this interferes with our ability to learn, we revisit.

## Contributing

Interested in helping out? Welcome! Check out the [CONTRIBUTING](./CONTRIBUTING.md) guide to get started.

## Q1 2018 Roadmap

The primary goal of the next three months is to validate the key ideas presented in this document and to get a sense for how long the envisioned system might take to develop. That's a pretty abstract goal, however.

More concretely, our goal is to ship a high-performance standalone editor component suitable for use in any web application, something we could eventually use on GitHub.com. This standalone editor will give us a chance to test a limited set of critical features in production scenarios without building out an entire desktop-based editor. We plan to develop this new editor in the context of a prototype Electron application, but we'll offer the standalone component as a separate build artifact from the main app.

* [ ] Standalone editor component (dev release)
  * [x] CRDT based text storage
  * [ ] WebGL-based black and white text rendering
    * [x] Rendering of glyphs with a 1:1 correspondence with characters
    * [x] Subpixel horizontal character positioning
    * [ ] Multi-character graphemes
  * [ ] Cursors and selections
    * [ ] Rendering
    * [ ] Basic movement
    * [ ] Auto-scroll
    * [ ] Advanced movement
  * [ ] Editing
    * [ ] Insert characters
    * [ ] Copy and paste
  * [ ] Web-compatible build artifact
  * [ ] Syntax highlighting
  * [ ] History
  * [ ] Synthetic scrollbars
* Stretch
  * [ ] Auto-indent
  * [ ] File system interaction on the desktop
