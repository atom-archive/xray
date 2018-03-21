process.env.NODE_ENV = "production";

const App = require("./app");
const FileFinder = require("./file_finder");
const QueryString = require("querystring");
const React = require("react");
const ReactDOM = require("react-dom");
const ViewRegistry = require("./view_registry");
const Workspace = require("./workspace");
const TextEditorView = require("./text_editor/text_editor");
const XrayClient = require("../shared/xray_client");
const $ = React.createElement;

async function start() {
  const url = window.location.search.replace("?", "");
  const { socketPath, windowId } = QueryString.parse(url);

  const xrayClient = new XrayClient();
  await xrayClient.start(socketPath);
  const viewRegistry = buildViewRegistry(xrayClient);

  let initialRender = true;
  xrayClient.addMessageListener(message => {
    switch (message.type) {
      case "UpdateWindow":
        viewRegistry.update(message);
        if (initialRender) {
          ReactDOM.render(
            $(App, { viewRegistry }),
            document.getElementById("app")
          );
          initialRender = false;
        }
        break;
      default:
        console.warn("Received unexpected message", message);
    }
  });

  xrayClient.sendMessage({
    type: "StartWindow",
    window_id: Number(windowId),
    height: window.innerHeight
  });
}

function buildViewRegistry(client) {
  const viewRegistry = new ViewRegistry({
    onAction: action => {
      action.type = "Action";
      client.sendMessage(action);
    }
  });
  viewRegistry.addComponent("Workspace", Workspace);
  viewRegistry.addComponent("FileFinder", FileFinder);
  viewRegistry.addComponent("BufferView", TextEditorView);
  return viewRegistry;
}

start();
