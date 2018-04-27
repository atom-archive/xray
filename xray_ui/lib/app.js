const propTypes = require("prop-types");
const React = require("react");
const { Client: StyletronClient } = require("styletron-engine-atomic");
const { Provider: StyletronProvider } = require("styletron-react");
const TextEditor = require("./text_editor/text_editor");
const ThemeProvider = require("./theme_provider");
const View = require("./view");
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
      $(ThemeProvider, { theme: theme }, $(View, { id: 0 }))
    );
  }
}

App.childContextTypes = {
  inBrowser: propTypes.bool,
  viewRegistry: propTypes.instanceOf(ViewRegistry)
};

module.exports = App;
