# Shared workspaces

## MVP

We want to host one or more workspaces on a remote server via a headless instance of Xray. Then we want to connect to the server from a client Xray instance and open one of these remote workspaces in a new window. Once open, we should be able to use the file finder to open buffers. A second client opening the same buffer should be able to make concurrent edits.

### Server

* `xray --headless foo/ bar/ --listen 8888` starts the Xray server listening on port 8888.

### Basic client experience

* `xray --connect hostname:port` opens a new window that is connected to the first workspace available on the remote host.
* `cmd-t` in the new window searches through paths in the remote workspace.
* Selecting a path opens it.
* Running `xray --connect` from a second instance allows for collaborative editing when clients open the same buffer.

### Selecting between multiple workspaces on the client

* If the host exposes multiple workspaces, `xray --connect hostname:port` opens an *Open Workspace* dialog that allows the user to select which workspace to open.
* `cmd-o` in any Xray window opens the *Open Workspace* dialog listing workspaces from all connected servers.

## Architecture

### Protocol

How should we structure network communication in relation to shared workspaces.

I think we should consider Cap'N Proto RPC, which creates the abstraction of a remote object graph. You define "interfaces", which represent remote objects and methods that they handle. You can then create a representation of a remote object and call methods on it. It can return you additional remote objects that you can also call methods on, which can return more remote objects, and so on.

#### Pushing messages from the server to the client

We need to be able to push changes to clients rather than always requesting them. In Cap'N Proto RPC, it doesn't look like you can define methods that return streaming results, but from a cursory reading of the [capnproto-rust pub/sub example](https://github.com/capnproto/capnproto-rust/tree/master/capnp-rpc/examples/pubsub), it looks like we *should* be able to achieve the same effect by calling methods on the server side's representation of client side objects. In the pub/sub example, the `Publisher` has a `subscribe` method that takes a `Subscriber` as an argument. The `Publisher` adds these `Subscriber` objects to a map and calls methods on them when it wants to broadcast messages. Clients implement the `Subscriber` interface so they can handle messages pushed by the server. We could easily relay the results of a stream to a client object by repeatedly calling a remote method with the stream's values.

#### Bootstrapping

A client/server connection needs to be bootstrapped, meaning there's an initial object that the server gives the client access to. What should this object be?

It seems like we might want to name the Cap'N Proto interface `Peer`. This interface would represent the app as a whole, and be designed to handle incoming requests from arbitrary clients. This is where we can implement authentication in the future. Since Cap'N Proto interfaces are compiled to traits, we may want `App` to implement this trait.

From here, we can ask the `Peer` to list its workspaces. We can get a handle to one of these workspaces and interact with it over RPC. Maybe we can create a `RemoteWorkspace` to plug in as the implementation of a `Workspace` trait that is owned by our `WorkspaceView`. The `RemoteWorkspace` will cache data locally and deal with CRDT operations, etc, and it will be designed to RPC with a workspace instance on another machine.
