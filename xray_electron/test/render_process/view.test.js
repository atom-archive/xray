const test = require("tape");
const React = require("react");
const $ = React.createElement;
const enzyme = require("./helpers/enzyme");
const propTypes = require("prop-types");
const View = require("../../lib/render_process/view");
const ViewRegistry = require("../../lib/render_process/view_registry");

test("View", t => {
  t.test("basic rendering", t => {
    const viewRegistry = new ViewRegistry();
    viewRegistry.addComponent("comp-1", props => $("div", {}, props.text));
    viewRegistry.addComponent("comp-2", props => $("label", {}, props.text));
    viewRegistry.update({
      updated: [
        { component_name: "comp-1", view_id: 1, props: { text: "text-1" } },
        { component_name: "comp-2", view_id: 2, props: { text: "text-2" } }
      ],
      removed: []
    });

    // Initial rendering
    const view = enzyme.shallow($(View, { id: 1 }), {
      context: { viewRegistry }
    });
    t.equal(view.html(), "<div>text-1</div>");

    // Changing view id
    view.setProps({ id: 2 });
    t.equal(view.html(), "<label>text-2</label>");

    // Updating view props
    viewRegistry.update({
      updated: [
        { component_name: "comp-2", view_id: 2, props: { text: "text-3" } }
      ],
      removed: []
    });
    view.update();
    t.equal(view.html(), "<label>text-3</label>");

    t.end();
  });

  t.test("action dispatching", t => {
    const actions = [];
    const viewRegistry = new ViewRegistry({ onAction: a => actions.push(a) });
    viewRegistry.update({
      updated: [{ component_name: "component", view_id: 42, props: {} }],
      removed: []
    });

    let dispatch;
    viewRegistry.addComponent("component", props => {
      dispatch = props.dispatch;
      return $("div");
    });

    const view = enzyme.shallow($(View, { id: 42 }), {
      context: { viewRegistry }
    });
    t.equal(view.html(), "<div></div>");

    dispatch({ type: "foo" });
    dispatch({ type: "bar" });
    t.deepEqual(actions, [
      { view_id: 42, action: { type: "foo" } },
      { view_id: 42, action: { type: "bar" } }
    ]);

    t.end()
  });
});
