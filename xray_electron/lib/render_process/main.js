process.env.NODE_ENV = "production"

const fs = require("fs");
const QueryString = require("querystring");
const path = require("path");
const xray = require("xray");
const React = require("react");
const ReactDOM = require("react-dom");
const Styletron = require("styletron-client");
const { StyletronProvider } = require("styletron-react");
const XrayClient = require('../shared/xray_client');

const ThemeProvider = require("./theme_provider");
const TextEditor = require("./text_editor/text_editor");

const {socketPath, windowId} = QueryString.parse(window.location.search);

const xrayClient = new XrayClient();
xrayClient.start(socketPath).then(() => {
  console.log('started!!!');

  xrayClient.addMessageListener(message => {
    console.log("MESSAGE", message);
  });

  xrayClient.sendMessage({
    type: 'StartWindow',
    window_id: windowId
  });

  setInterval(() => {
    xrayClient.sendMessage({
      type: 'Action',
      view_id: 0,
      action: {
        type: 'ToggleFileFinder'
      }
    });
  }, 1000);
});

const $ = React.createElement;

const theme = {
  editor: {
    fontFamily: "Menlo",
    backgroundColor: "white",
    baseTextColor: "black",
    fontSize: 14,
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
