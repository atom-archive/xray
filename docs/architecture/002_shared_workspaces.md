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

Here's the current thinking:

On the server, maintain a pool of "services" for each connected client. A service maps to an entity in the domain model that we want to share. The client connection is created with one or more initial services, and the first service added is considered the "bootstrap" service.

When a client connects, they build a client side representation of this pool of services. They retrieve a *client* for the bootstrap service. Clients have no understanding of the type of the underlying service. They expose an initial snapshot for the service's state, a stream of updates, and a method to make requests to the service and receive responses back. All of these facilities are untyped and deal with raw bytes. We could potentially expose a convenient facility for imposing a typed interface on the raw values.

The client is intended to be wrapped by a higher level object that is coded to the type of the service that the client interfaces with. We could potentially add some kind of facility for ensuring the client is interfacing to the service we expect, possibly by just giving services unique type names. When higher level code calls methods on the client's wrapper object, it may result in that object performing a request via the client.

The wrapper object should also spawn a future to process incoming updates and incorporate them into locally-replicated state as necessary.

When the server wants to give the client access to a new service, we expect this to occur when handling a request. In the `request` method's interface, we could pass a reference to the service pool just like we do in the window. This would allow the service to add additional services and retrieve their ids. We could then include these ids as necessary in our response.

On the client side, when we receive a response, we could call into the client-side representation of the service pool to take ownership of the client side of the service. When we drop the client, we can send a signal to the server that it can also drop the server side of the service.

So long as the client-side code always takes ownership of any service added on the server side, this should avoid leaks. What if the client side doesn't ever take ownership? With every response, we could potentially include the id of the most-recently-added service. After the response is processed, we could schedule a cleanup of any unclaimed services that were added during that request cycle and possible emit some kind of warning.

So to summarize, I see `Service` as a trait that server-side domain objects will implement that explains how to connect that object to RPC clients. `ServiceClient` is a concrete implementation that we wrap with domain objects.
