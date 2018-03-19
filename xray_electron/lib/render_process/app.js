const fs = require("fs");
const path = require("path");
const propTypes = require("prop-types");
const React = require("react");
const Styletron = require("styletron-client");
const { StyletronProvider } = require("styletron-react");
const TextEditor = require("./text_editor/text_editor");
const ThemeProvider = require("./theme_provider");
const View = require('./view')
const ViewRegistry = require("./view_registry");
const $ = React.createElement;

const theme = {
  editor: {
    fontFamily: "Menlo",
    backgroundColor: "white",
    baseTextColor: "black",
    fontSize: 14,
    lineHeight: 1.5
  }
};

class App extends React.Component {
  constructor(props) {
    super(props);
  }

  getChildContext() {
    return { viewRegistry: this.props.viewRegistry };
  }

  render() {
    return $(
      StyletronProvider,
      { styletron: new Styletron() },
      $(ThemeProvider, { theme: theme }, $(View, { id: 0 }))
    );
  }
}

App.childContextTypes = {
  viewRegistry: propTypes.instanceOf(ViewRegistry)
};

module.exports = App;
