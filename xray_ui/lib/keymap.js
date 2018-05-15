const React = require("react");
const ReactDOM = require("react-dom");
const propTypes = require("prop-types");
const { styled } = require("styletron-react");
const $ = React.createElement;

const Root = styled("div", { width: "100%", height: "100%" });

class ActionSet {
  constructor() {
    this.context = null;
    this.actions = new Map();
  }
}

class KeymapProvider extends React.Component {
  constructor() {
    super();
    this.handleKeyDown = this.handleKeyDown.bind(this);
    this.actionSets = new WeakMap();
    this.defaultActionSet = new ActionSet();
  }

  render() {
    return $(Root, { onKeyDown: this.handleKeyDown }, this.props.children);
  }

  handleKeyDown(event) {
    const keyBindings = this.props.keyBindings;
    const keystrokeString = keystrokeStringForEvent(event);

    let element = event.target;
    while (element) {
      let actionSet = this.actionSets.get(element);
      if (actionSet) {
        for (let i = keyBindings.length - 1; i >= 0; i--) {
          const keyBinding = keyBindings[i];
          if (
            keyBinding.key === keystrokeString &&
            actionSet.actions.has(keyBinding.action) &&
            contextMatches(actionSet.context, keyBinding.context)
          ) {
            const dispatchAction = actionSet.actions.get(keyBinding.action);
            dispatchAction({ type: keyBinding.action });
          }
        }
      }

      element = element.parentElement;
    }
  }

  getChildContext() {
    return {
      actionSets: this.actionSets,
      currentActionSet: this.defaultActionSet
    };
  }
}

KeymapProvider.childContextTypes = {
  actionSets: propTypes.instanceOf(WeakMap),
  currentActionSet: propTypes.instanceOf(ActionSet)
};

class ActionContext extends React.Component {
  constructor() {
    super();
    this.actionSet = new ActionSet();
  }

  componentDidMount() {
    const { context } = this.props;
    this.actionSet.context = new Set(
      Array.isArray(context) ? context : [context]
    );
    this.context.actionSets.set(
      ReactDOM.findDOMNode(this).parentElement,
      this.actionSet
    );
  }

  render() {
    return this.props.children;
  }

  getChildContext() {
    return {
      currentActionSet: this.actionSet
    };
  }
}

ActionContext.contextTypes = {
  actionSets: propTypes.instanceOf(WeakMap)
};

ActionContext.childContextTypes = {
  currentActionSet: propTypes.instanceOf(ActionSet)
};

class Action extends React.Component {
  render() {
    return null;
  }

  componentDidMount() {
    this.context.currentActionSet.actions.set(
      this.props.type,
      this.context.dispatchAction
    );
  }
}

Action.contextTypes = {
  currentActionSet: propTypes.instanceOf(ActionSet),
  dispatchAction: propTypes.func
};

function keystrokeStringForEvent(event) {
  let keystroke = "";
  if (event.ctrlKey) keystroke = "ctrl";
  if (event.altKey) keystroke = appendKeystrokeElement(keystroke, "ctrl");
  if (event.shiftKey) keystroke = appendKeystrokeElement(keystroke, "shift");
  if (event.metaKey) keystroke = appendKeystrokeElement(keystroke, "cmd");
  return appendKeystrokeElement(keystroke, event.key);
}

function appendKeystrokeElement(keyString, element) {
  if (keyString.length > 0) keyString += "-";
  keyString += element;
  return keyString;
}

function contextMatches(context, expression) {
  // TODO: Support arbitrary boolean expressions
  return context.has(expression);
}

module.exports = { KeymapProvider, ActionContext, Action, contextMatches };
