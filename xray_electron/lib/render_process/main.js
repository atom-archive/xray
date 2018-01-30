process.env.NODE_ENV = "production"

const fs = require("fs");
const path = require("path");
const xray = require("xray");
const React = require("react");
const ReactDOM = require("react-dom");
const Styletron = require("styletron-client");
const { StyletronProvider } = require("styletron-react");

const ThemeProvider = require("./theme_provider");
const TextEditor = require("./text_editor/text_editor");

const $ = React.createElement;

const theme = {
  editor: {
    fontFamily: "Monaco",
    backgroundColor: "white",
    baseTextColor: "black",
    fontSize: 20,
    lineHeight: 1.5
  }
}

ReactDOM.render(
  $(
    StyletronProvider,
    { styletron: new Styletron() },
    $(ThemeProvider, { theme: theme }, $(TextEditor, {
      initialText: fs.readFileSync(path.join(__dirname, '../../node_modules/react/cjs/react.development.js'), 'utf8')
    }))
  ),
  document.getElementById("app")
);
