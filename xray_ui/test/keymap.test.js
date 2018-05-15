const assert = require("assert");
const propTypes = require("prop-types");
const React = require("react");
const { mount } = require("./helpers/component_helpers");
const $ = React.createElement;
const {
  Keymap,
  KeymapProvider,
  ActionContext,
  Action
} = require("../lib/keymap");

suite("Keymap", () => {
  test.only("dispatching an action via a keystroke", () => {
    class Component extends React.Component {
      render() {
        return $(
          KeymapProvider,
          { keymap },
          $(
            "div",
            null,
            $(
              ActionContext,
              { add: ["a"] },
              $(Action, { type: "Action1" }),
              $(
                "div",
                null,
                $(
                  ActionContext,
                  { add: ["b"] },
                  $(Action, { type: "Action2" }),
                  $("div", { id: "target" })
                )
              )
            )
          )
        );
      }
    }

    const dispatchedActions = [];
    const keymap = new Keymap();

    const component = mount($(Component), {
      context: { dispatch: action => dispatchedActions.push(action) },
      childContextTypes: { dispatch: propTypes.func }
    });

    component.find("#target").simulate("keyDown");
  });
});
