const test = require("tape");
const ViewRegistry = require("../lib/render_process/view_registry");

test("ViewRegistry", t => {
  t.test("props", t => {
    const registry = new ViewRegistry();

    // Adding initial views
    registry.update({
      updated: [
        { component_name: "component-1", view_id: 1, props: { a: 1 } },
        { component_name: "component-2", view_id: 2, props: { b: 2 } }
      ],
      removed: []
    });

    t.deepEqual(registry.getProps(1), { a: 1 });
    t.deepEqual(registry.getProps(2), { b: 2 });
    t.throws(() => registry.getProps(3));

    const propChanges = [];
    const disposeProps1Watch = registry.watchProps(1, () =>
      propChanges.push("component-1")
    );
    const disposeProps2Watch = registry.watchProps(2, () =>
      propChanges.push("component-2")
    );
    t.throws(() => registry.watchProps(3, () => {}));

    // Updating existing view, removing existing view, adding a new view
    registry.update({
      updated: [
        { component_name: "component-2", view_id: 2, props: { b: 3 } },
        { component_name: "component-3", view_id: 3, props: { c: 4 } }
      ],
      removed: [1]
    });

    t.throws(() => registry.getProps(1));
    t.deepEqual(registry.getProps(2), { b: 3 });
    t.deepEqual(registry.getProps(3), { c: 4 });

    t.throws(() => registry.watchProps(1, () => {}));
    t.deepEqual(propChanges, ["component-2"]);

    // Stop watching props for a view
    propChanges.length = 0;
    disposeProps2Watch();
    disposeProps2Watch(); // ensure disposing is idempotent
    registry.update({
      updated: [{ component_name: "component-2", view_id: 2, props: { b: 4 } }],
      removed: []
    });

    t.deepEqual(propChanges, []);

    t.end();
  });

  t.test("components", t => {
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
    t.equal(registry.getComponent(1), comp1A);
    t.equal(registry.getComponent(2), comp2A);
    t.throws(() => registry.getComponent(3));

    registry.removeComponent("comp-1");
    t.throws(() => registry.getComponent(1));
    t.equal(registry.getComponent(2), comp2A);

    const comp1B = () => {};
    const comp2B = () => {};
    registry.addComponent("comp-1", comp1B);
    t.throws(() => registry.addComponent("comp-2", comp2B));
    t.equal(registry.getComponent(1), comp1B);

    t.end();
  });

  t.test("dispatching actions", t => {
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
    t.throws(() => registry.dispatchAction(3, { d: 4 }));

    t.deepEqual(actions, [
      { view_id: 1, action: { a: 1, b: 2 } },
      { view_id: 2, action: { c: 3 } }
    ]);

    t.end();
  });
});
