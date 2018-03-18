# Contributing to Xray

This project is still in the very early days, and isn't going to be usable for even basic editing for some time. At this point, the we're looking for contributors that are willing to roll up their sleeves and solve problems. Please communicate with us however it makes sense, but in general opening a *pull request that fixes an issue* is going to be far more valuable than just reporting an issue.

As the architecture stabilizes and the surface area of the project expands, there will be increasing opportunities to help out. To get some ideas for specific projects that could help in the short term, check out [issues that are labeled "help wanted"](https://github.com/atom/xray/issues?q=is%3Aopen+is%3Aissue+label%3A%22help+wanted%22). If you have an idea you'd like to pursue outside of these, that's awesome, but you may want to discuss it with us in an issue first to ensure it fits before spending too much time on it.

It's really important to us to have a smooth on-ramp for contributors, and one great way you can contribute is by helping us improve this guide. If your experience is bumpy, can you open a pull request that makes it smoother for the next person?

## Communicating with maintainers

The best way to communicate with maintainers is by posting a issue to this repository. The more thought you put into articulating your question or idea, the more value you'll be adding to the community and the easier it will be for maintainers to respond. That said, just try your best. If you have something you want to say, we'd prefer that you say it imperfectly rather than not saying it at all.

You can also communicate with maintainers or other community members on the `#xray` channel on Atom's public slack instance. After you [request an invite via this form](http://atom-slack.herokuapp.com/), you can access our Slack instance at https://atomio.slack.com.

## Building

So far, we have only built this project on macOS. If you'd like to help us improve our build or documentation to support other platforms, that would be a huge help!

### Install system dependencies

#### Install Node v8.9.3

To install Node, you can install [`nvm`](https://github.com/creationix/nvm) and then run `nvm install v8.9.3`.

Later versions may work, but you should ideally run the build with the same version of Node that is bundled into Xray's current Electron dependency. If in doubt, you can check the version of the `electron` dependency in [`xray_electron/package.json`](https://github.com/atom/xray/blob/master/xray_electron/package.json), then run `process.versions.node` in the console of that version of Electron to ensure that these instructions haven't gotten out of date.

#### Install Rust

You can install Rust via [`rustup`](https://www.rustup.rs/). We currently build correctly on Rust 1.24.1, but frequently build on the nightly channel in development to enable formatting of generated bindings. The nightly channel should not be *required* however, and if it is, that's a bug.

### Build the Electron App

This repository contains several components in top-level folders prefixed with `xray_*`. The main applicaiton is located in `xray_electron`, and you can build it as follows:

```sh
# Move to this subdirectory of the repository:
cd xray_electron

# Install and build dependencies:
npm install

# Launch Electron:
npm start
```

If you want to *rebuild* the Rust dependencies after making changes and test them in the Electron app, run this:

```sh
# Rebuild Rust dependencies:
npm rebuild xray
```

### Build other modules independently

If you're working on a particular subsystem, such as [`xray_core`](./xray_core), you can build and test it independently of the Electron app. Each top-level module should have its own instructions in its README.
