const propTypes = require("prop-types");
const React = require("react");
const { Client: StyletronClient } = require("styletron-engine-atomic");
const { Provider: StyletronProvider } = require("styletron-react");
const { ActionDispatcher } = require("./action_dispatcher");
const TextEditor = require("./text_editor/text_editor");
const ThemeProvider = require("./theme_provider");
const View = require("./view");
const ViewRegistry = require("./view_registry");
const $ = React.createElement;

// TODO: Eventually, the theme should be provided to the view by the server
const theme = {
  editor: {
    fontFamily: "Menlo",
    backgroundColor: "white",
    baseTextColor: "black",
    fontSize: 14,
    lineHeight: 1.5
  },
  userColors: [
    { r: 31, g: 150, b: 255, a: 1 },
    { r: 64, g: 181, b: 87, a: 1 },
    { r: 206, g: 157, b: 59, a: 1 },
    { r: 216, g: 49, b: 176, a: 1 },
    { r: 235, g: 221, b: 91, a: 1 }
  ]
};

// TODO: Eventually, the keyBindings should be provided to the view by the server
const keyBindings = [
  { key: "cmd-t", context: "Workspace", action: "ToggleFileFinder" },
  { key: "ctrl-t", context: "Workspace", action: "ToggleFileFinder" },
  { key: "cmd-s", context: "Workspace", action: "SaveActiveBuffer" },
  { key: "up", context: "FileFinder", action: "SelectPrevious" },
  { key: "down", context: "FileFinder", action: "SelectNext" },
  { key: "enter", context: "FileFinder", action: "Confirm" },
  { key: "escape", context: "FileFinder", action: "Close" },
  { key: "alt-shift-up", context: "TextEditor", action: "AddSelectionAbove" },
  { key: "alt-shift-down", context: "TextEditor", action: "AddSelectionBelow" },
  { key: "shift-up", context: "TextEditor", action: "SelectUp" },
  { key: "shift-down", context: "TextEditor", action: "SelectDown" },
  { key: "shift-left", context: "TextEditor", action: "SelectLeft" },
  { key: "shift-right", context: "TextEditor", action: "SelectRight" },
  {
    key: "alt-shift-left",
    context: "TextEditor",
    action: "SelectToBeginningOfWord"
  },
  {
    key: "alt-shift-right",
    context: "TextEditor",
    action: "SelectToEndOfWord"
  },
  {
    key: "shift-cmd-left",
    context: "TextEditor",
    action: "SelectToBeginningOfLine"
  },
  {
    key: "shift-cmd-right",
    context: "TextEditor",
    action: "SelectToEndOfLine"
  },
  {
    key: "shift-cmd-up",
    context: "TextEditor",
    action: "SelectToTop"
  },
  {
    key: "shift-cmd-down",
    context: "TextEditor",
    action: "SelectToBottom"
  },
  { key: "up", context: "TextEditor", action: "MoveUp" },
  { key: "down", context: "TextEditor", action: "MoveDown" },
  { key: "left", context: "TextEditor", action: "MoveLeft" },
  { key: "right", context: "TextEditor", action: "MoveRight" },
  { key: "alt-left", context: "TextEditor", action: "MoveToBeginningOfWord" },
  { key: "alt-right", context: "TextEditor", action: "MoveToEndOfWord" },
  { key: "cmd-left", context: "TextEditor", action: "MoveToBeginningOfLine" },
  { key: "cmd-right", context: "TextEditor", action: "MoveToEndOfLine" },
  { key: "cmd-up", context: "TextEditor", action: "MoveToTop" },
  { key: "cmd-down", context: "TextEditor", action: "MoveToBottom" },
  { key: "backspace", context: "TextEditor", action: "Backspace" },
  { key: "delete", context: "TextEditor", action: "Delete" }
];

const styletronInstance = new StyletronClient();
class App extends React.Component {
  constructor(props) {
    super(props);
  }

  getChildContext() {
    return {
      inBrowser: this.props.inBrowser,
      viewRegistry: this.props.viewRegistry
    };
  }

  render() {
    return $(
      StyletronProvider,
      { value: styletronInstance },
      $(
        ThemeProvider,
        { theme },
        $(ActionDispatcher, { keyBindings }, $(View, { id: 0 }))
      )
    );
  }
}

App.childContextTypes = {
  inBrowser: propTypes.bool,
  viewRegistry: propTypes.instanceOf(ViewRegistry)
};

module.exports = App;
