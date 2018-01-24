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
    backgroundColor: "black",
    baseTextColor: "white",
    fontFamily: "Fira Code",
    fontSize: 20,
    lineHeight: 1.3
  }
}

ReactDOM.render(
  $(
    StyletronProvider,
    { styletron: new Styletron() },
    $(ThemeProvider, { theme: theme }, $(TextEditor))
  ),
  document.getElementById("app")
);
