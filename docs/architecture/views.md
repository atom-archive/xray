## Communicating view updates and dispatching actions

In `xray_server`, a "view" is an object that represents the state of an on-screen component. Multiple views can be displayed in a window at any point in time, and all the views belonging to a window are owned by a `ViewRegistry` that is associated with the window client. Each view instance is assigned a unique id when it is registered with the `ViewRegistry`, and it can be referenced by this id in other views.

Whenever a view is added to or removed from the registry, the registry indicates that it is "dirty" by incrementing a `NotifyCell<usize>` called `version`. This causes an update message to be sent to the window client at the end of the current event loop cycle indicating which views were added or deleted. Registered views are required to implement the `View` trait, which includes a `render()` method that returns a snapshot of the view that can be serialized to JSON and sent to the window client.

While a view is present in the registry, the registry also observes the view for updates via a `NotifyCell` returned from the `version()` method on the `View` trait. Whenever any view indicates that it has updated by incrementing its `version`, it causes the registry to mark the view as dirty and update its own `version`, triggering updates for the dirty views to be sent to the window client.

Finally, the `View` trait has a required `dispatch(action: serde_json::Value)` method, which allows views to handle actions sent from the window client. Actions are used to indicate interactions by the user, and may cause changes to the state of the server-side view or any objects it references.

## Example

The primary view in any window is the `WorkspaceView`, which represents the "chrome" that fills the window and contain other views. The workspace is the first view registered for any window, and has an id of `0`.

To toggle the file-finder (bound to `cmd-t` in Atom), we dispatch an action to the `WorkspaceView` as follows in the window:

```js
viewRegistry.dispatch(0, {type: "ToggleFileFinder"})
```

This action gets sent over the window client's socket connection and arrives at the server as an `Action` message, which embeds the id of the target view. This view is looked up in the view registry associated with the connection's window, and the action is dispatched on the `WorkspaceView`.

In the `WorkspaceView`'s `dispatch` method, we deserialize the JSON value to a statically typed enum and pattern match it in a case statement to call the `WorkspaceView::toggle_file_finder` method. This method checks the `WorkspaceView::modal_panel_id: Option<usize>` field, and if it is `None`, creates and registers a `FileFinderView` with the view registry, obtaining a unique id. It then assigns this id to the field and increments the `version` cell to indicate the view has changed.

Registering the view and updating the workspace view's own version both cause the view registry to become dirty, sending an update to the client. In the client, the `WorkspaceComponent` now receives props that contain something like the following:

```js
{
  modalPanelId: 32
}
```

The workspace component performs a lookup on the client-side representation of the view registry to determine the component class and state associated with this view id. When the `FileFinderComponent` mounts, it sets up an observer on the client side view registry to watch for any updates:

```js
componentDidMount() {
  this.observation =
    this.props.viewRegistry.observe(this.props.viewId, (newProps) => {
      // This should probably happen in a container component or
      // in some more kosher way, but this is the general idea for now.
      this.setProps(newProps);
    });
}
componentDidUnmount() {
  this.observation.dispose()
}
```

If the user sends a second `ToggleFileFinder` action to the workspace component, the workspace *removes* the file finder from the view registry and sets the `modal_panel_id` field back to `None`.

In this way, we can communicate the latest state of an arbitrary number of views to the window process and also handle actions on any visible view.
