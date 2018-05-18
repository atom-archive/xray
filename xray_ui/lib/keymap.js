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
    const { keyBindings } = this.props;
    const keystrokeString = keystrokeStringForEvent(event);

    let element = event.target;
    while (element) {
      let actionSet = this.actionSets.get(element);
      if (actionSet) {
        for (let i = keyBindings.length - 1; i >= 0; i--) {
          const keyBinding = keyBindings[i];
          const action = actionSet.actions.get(keyBinding.action);
          if (
            keyBinding.key === keystrokeString &&
            action &&
            contextMatches(actionSet.context, keyBinding.context)
          ) {
            if (action.onWillDispatch) action.onWillDispatch();
            action.dispatch();
            return;
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

  componentWillMount() {
    this.actionSet.context = this.context.currentActionSet
      ? new Set(this.context.currentActionSet.context)
      : new Set();

    if (this.props.add) {
      if (Array.isArray(this.props.add)) {
        for (let i = 0; i < this.props.add.length; i++) {
          this.actionSet.context.add(this.props.add[i]);
        }
      } else {
        this.actionSet.context.add(this.props.add[i]);
      }
    }

    if (this.props.remove) {
      if (Array.isArray(this.props.remove)) {
        for (let i = 0; i < this.props.remove.length; i++) {
          this.actionSet.context.delete(this.props.remove[i]);
        }
      } else {
        this.actionSet.context.delete(this.props.remove[i]);
      }
    }
  }

  componentDidMount() {
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
  actionSets: propTypes.instanceOf(WeakMap),
  currentActionSet: propTypes.instanceOf(ActionSet)
};

ActionContext.childContextTypes = {
  currentActionSet: propTypes.instanceOf(ActionSet)
};

class Action extends React.Component {
  constructor() {
    super();
    this.dispatch = this.dispatch.bind(this);
  }

  render() {
    return null;
  }

  componentDidMount() {
    this.context.currentActionSet.actions.set(this.props.type, {
      onWillDispatch: this.props.onWillDispatch,
      dispatch: this.dispatch
    });
  }

  dispatch() {
    this.context.dispatchAction({ type: this.props.type });
  }
}

Action.contextTypes = {
  currentActionSet: propTypes.instanceOf(ActionSet),
  dispatchAction: propTypes.func
};

function keystrokeStringForEvent(event) {
  let keystroke = "";
  if (event.ctrlKey) keystroke = "ctrl";
  if (event.altKey) keystroke = appendKeystrokeElement(keystroke, "alt");
  if (event.shiftKey) keystroke = appendKeystrokeElement(keystroke, "shift");
  if (event.metaKey) keystroke = appendKeystrokeElement(keystroke, "cmd");
  switch (event.key) {
    case "ArrowDown":
      return appendKeystrokeElement(keystroke, "down");
    case "ArrowUp":
      return appendKeystrokeElement(keystroke, "up");
    case "ArrowLeft":
      return appendKeystrokeElement(keystroke, "left");
    case "ArrowRight":
      return appendKeystrokeElement(keystroke, "right");
    default:
      return appendKeystrokeElement(keystroke, event.key.toLowerCase());
  }
}

function appendKeystrokeElement(keyString, element) {
  if (keyString.length > 0) keyString += "-";
  keyString += element;
  return keyString;
}

function contextMatches(context, expression) {
  // TODO: Support arbitrary boolean expressions
  let expressionStartIndex = 0;
  for (let i = 0; i < expression.length; i++) {
    if (expression[i] == " ") {
      const component = expression.slice(expressionStartIndex, i);
      if (!context.has(component)) {
        return false;
      }
    }
  }
  return true;
}

module.exports = {
  KeymapProvider,
  ActionContext,
  Action,
  keystrokeStringForEvent,
  contextMatches
};
