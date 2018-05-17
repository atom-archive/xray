const assert = require("assert");
const propTypes = require("prop-types");
const React = require("react");
const { mount } = require("./helpers/component_helpers");
const $ = React.createElement;
const { KeymapProvider, ActionContext, Action } = require("../lib/keymap");

suite("Keymap", () => {
  test("dispatching an action via a keystroke", () => {
    class Component extends React.Component {
      render() {
        return $(
          KeymapProvider,
          { keyBindings: this.props.keyBindings },
          $(
            "div",
            null,
            $(
              ActionContext,
              { add: ["a", "b"] },
              $(Action, { type: "Action1" }),
              $(Action, { type: "Action2" }),
              $(
                "div",
                null,
                $(
                  ActionContext,
                  { add: ["c"], remove: ["a"] },
                  $(Action, { type: "Action3" }),
                  $("div", { id: "target" })
                )
              )
            )
          )
        );
      }
    }

    let dispatchedActions;
    const keyBindings = [
      { key: "ctrl-alt-a", context: "a b", action: "Action1" },
      { key: "ctrl-alt-a", context: "b c", action: "Action3" },
      { key: "ctrl-alt-b", context: "a b", action: "Action2" },
      { key: "ctrl-alt-c", context: "a b", action: "UnregisteredAction" }
    ];
    const component = mount($(Component, { keyBindings }), {
      context: {
        dispatchAction: action => dispatchedActions.push(action.type)
      },
      childContextTypes: { dispatchAction: propTypes.func }
    });
    const target = component.find("#target");

    dispatchedActions = [];
    target.simulate("keyDown", { ctrlKey: true, altKey: true, key: "a" });
    assert.deepEqual(dispatchedActions, ["Action3"]);

    dispatchedActions = [];
    target.simulate("keyDown", { ctrlKey: true, altKey: true, key: "b" });
    assert.deepEqual(dispatchedActions, ["Action2"]);

    dispatchedActions = [];
    target.simulate("keyDown", { ctrlKey: true, altKey: true, key: "c" });
    assert.deepEqual(dispatchedActions, []);
  });
});
