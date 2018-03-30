const assert = require("assert");
const ViewRegistry = require("../../lib/render_process/view_registry");

suite("ViewRegistry", () => {
  test("props", () => {
    const registry = new ViewRegistry();

    // Adding initial views
    registry.update({
      updated: [
        { component_name: "component-1", view_id: 1, props: { a: 1 } },
        { component_name: "component-2", view_id: 2, props: { b: 2 } }
      ],
      removed: []
    });

    assert.deepEqual(registry.getProps(1), { a: 1 });
    assert.deepEqual(registry.getProps(2), { b: 2 });
    assert.throws(() => registry.getProps(3));

    const propChanges = [];
    const disposeProps1Watch = registry.watchProps(1, () =>
      propChanges.push("component-1")
    );
    const disposeProps2Watch = registry.watchProps(2, () =>
      propChanges.push("component-2")
    );
    assert.throws(() => registry.watchProps(3, () => {}));

    // Updating existing view, removing existing view, adding a new view
    registry.update({
      updated: [
        { component_name: "component-2", view_id: 2, props: { b: 3 } },
        { component_name: "component-3", view_id: 3, props: { c: 4 } }
      ],
      removed: [1]
    });

    assert.throws(() => registry.getProps(1));
    assert.deepEqual(registry.getProps(2), { b: 3 });
    assert.deepEqual(registry.getProps(3), { c: 4 });

    assert.throws(() => registry.watchProps(1, () => {}));
    assert.deepEqual(propChanges, ["component-2"]);

    // Stop watching props for a view
    propChanges.length = 0;
    disposeProps2Watch();
    disposeProps2Watch(); // ensure disposing is idempotent
    registry.update({
      updated: [{ component_name: "component-2", view_id: 2, props: { b: 4 } }],
      removed: []
    });

    assert.deepEqual(propChanges, []);
  });

  test("components", () => {
    const registry = new ViewRegistry();
    registry.update({
      updated: [
        { component_name: "comp-1", view_id: 1, props: {} },
        { component_name: "comp-2", view_id: 2, props: {} },
        { component_name: "comp-3", view_id: 3, props: {} }
      ],
      removed: []
    });

    const comp1A = () => {};
    const comp2A = () => {};
    registry.addComponent("comp-1", comp1A);
    registry.addComponent("comp-2", comp2A);
    assert.equal(registry.getComponent(1), comp1A);
    assert.equal(registry.getComponent(2), comp2A);
    assert.throws(() => registry.getComponent(3));

    registry.removeComponent("comp-1");
    assert.throws(() => registry.getComponent(1));
    assert.equal(registry.getComponent(2), comp2A);

    const comp1B = () => {};
    const comp2B = () => {};
    registry.addComponent("comp-1", comp1B);
    assert.throws(() => registry.addComponent("comp-2", comp2B));
    assert.equal(registry.getComponent(1), comp1B);
  });

  test("dispatching actions", () => {
    const actions = [];
    const registry = new ViewRegistry({ onAction: a => actions.push(a) });

    registry.update({
      updated: [
        { component_name: "component-1", view_id: 1, props: {} },
        { component_name: "component-2", view_id: 2, props: {} }
      ],
      removed: []
    });

    registry.dispatchAction(1, { a: 1, b: 2 });
    registry.dispatchAction(2, { c: 3 });
    assert.throws(() => registry.dispatchAction(3, { d: 4 }));

    assert.deepEqual(actions, [
      { view_id: 1, action: { a: 1, b: 2 } },
      { view_id: 2, action: { c: 3 } }
    ]);
  });

  test("focus", () => {
    const registry = new ViewRegistry({ onAction: a => actions.push(a) });

    const focusRequests = [];
    registry.update({ updated: [], removed: [], focus: 2 });
    registry.update({ updated: [], removed: [], focus: 1 });
    registry.update({ updated: [], removed: [], focus: 1 });
    const disposeWatch1 = registry.watchFocus(1, () => focusRequests.push(1));
    registry.watchFocus(2, () => focusRequests.push(2));
    registry.update({ updated: [], removed: [], focus: 1 });
    registry.update({ updated: [], removed: [], focus: 2 });

    assert.deepEqual(focusRequests, [1, 1, 2]);
    assert.throws(() => registry.watchFocus(1));

    disposeWatch1()
    assert.doesNotThrow(() => registry.watchFocus(1))
  });
});
