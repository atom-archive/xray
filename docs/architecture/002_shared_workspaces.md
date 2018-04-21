# Shared workspaces

## Current features

An instance of `xray_server` can host one or more shared workspaces, which can be accessed by other `xray_server` instances over the network. Currently, when connecting to a remote peer, we automatically open its first shared workspace in a window on the client. The client can use the file finder to locate and open any file in the shared workspace's project. When multiple participants open a buffer for the same file, their edits are replicated to other collaborators in real time.

### Server

* `xray foo/ bar/ --listen 8888` starts the Xray server listening on port 8888.
* The `--headless` flag can be passed to create a server that only hosts workspaces for other clients and does not present its own UI.

### Basic client experience

* `xray --connect hostname:port` opens a new window that is connected to the first workspace available on the remote host.
* `cmd-t` in the new window searches through paths in the remote workspace.
* Selecting a path opens it.
* Running `xray --connect` from a second instance allows for collaborative editing when clients open the same buffer.

### Selecting between multiple workspaces on the client

* If the host exposes multiple workspaces, `xray --connect hostname:port` opens an *Open Workspace* dialog that allows the user to select which workspace to open.
* `cmd-o` in any Xray window opens the *Open Workspace* dialog listing workspaces from all connected servers.

## RPC System

We implement shared workspaces on top of an RPC system that allows objects on the client to derive their state and behavior from objects that live on the server.

### Goals

#### Support replicated objects

The primary goal of the system is to support the construction of a replicated object-oriented domain model. In addition to supporting remote procedure calls, we also want the system to explicitly support long-lived, stateful objects that change over time.

Replication support should be fairly additive, meaning that the domain model on the server side should be designed pretty much as if it weren't replicated. On the client side, interacting with representations of remote objects should be explicit but convenient.

#### Capabilities-based security

Secure ECMA Script and Cap'N Proto introduced me to the concept of capabilities-based security, and our system adopts the same philosophy. Objects on the server are exposed via *services*, which can be thought of as "capabilities" that grant access to a narrow slice of functionality that is dynamically defined. Starting from a single root service, remote users are granted increasing access by being provided with additional capabilities.

#### Dynamic resource management

Server-side services only need to live as long as they are referenced by a client. Server-side code can elect to retain a reference to a service. Otherwise, ownership is maintained by clients over the wire. If both the server and the client drop their reference-counted handle to a service, we should drop the service on the server side automatically.

#### Binary messages

We want to move data efficiently between the server and client, so a binary encoding scheme for messages is important. For now, we're using bincode for convenience, but we should eventually switch to Protocol Buffers to support graceful evolution of the protocol.

### Design

![Diagram](../images/rpc.png)

**Services** are the fundamental abstraction of the system.

In `rpc::server`, `Service` is a *trait* that can be implemented by a custom service wrapper for each domain object that makes the object accessible to remote clients. A `Service` exposes a static snapshot of the object's initial state, a stream of updates, and the ability to handle requests. The `Service` trait has various associated types for `Request`, `Response`, `Update`, and `State`.

When server-side code accepts connections, it creates an `rpc::server::Connection` object for each client that takes ownership of the `Stream` of that client's incoming messages. `Connection`s must be created with a *root service*, which is sent to the client immediately. The `Connection` is itself a `Stream` of outgoing messages to be sent to the connected client.

On the client side, we create a connection by passing the `Stream` of incoming messages to `rpc::client::Connection::new`, which returns a *future* for a tuple containing two objects. The first object is a `rpc::client::Service` representing the *root service* that was sent from the server. The second is an instance of `client::Connection`, which is a `Stream` of outgoing messages to send to the server.

Using the root service, the client can make requests to gain access to additional services. In Xray, the root service is currently `app::AppService`, which includes a list of shared workspaces in its replicated state. After a client connects to a server, it stores a handle to its root service in a `PeerList` object. We will eventually build a `PeerListView` based on the state of the `PeerList`, which allows the user to open a remote workspace on any connected peer. For now, we automatically open the first workspace when connecting to a remote peer.

When we connect to a remote workspace, we send an `app::ServiceRequest::OpenWorkspace` message to the remote `AppService`. When handling this request in the `AppService` on the server, we call `add_service` on the connection with a `WorkspaceService` for the requested workspace, which returns us a `ServiceId` integer. We send that id to the client in the response. When handling the response on the client, we call `take_service` on root service with the id to take ownership of a handle to the remote service.

We can then create a `RemoteWorkspace` and pass it ownership of the service handle to the remote workspace. `RemoteWorkspace` and `LocalWorkspace` both implement the `Workspace` trait, which allows a `RemoteWorkspace` to be used in the system in all of the same ways that a `LocalWorkspace` can.

We create the illusion that remote domain objects are really local through a combination of state replication and remote procedure calls. Fuzzy finding on the project file trees is addressed through replication, since the data size is typically small and the task is latency sensitive. Project-wide search is implemented via RPC, since replicating the contents of the entire remote file system would be costly, especially for the in-browser use case. Buffer replication is implemented by relaying conflict-free representations of individual edit operations, which can be correctly integrated on remote replicas due to our use of a CRDT in Xray's underlying buffer implementation.
