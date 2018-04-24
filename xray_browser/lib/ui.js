import { React, ReactDOM, App, buildViewRegistry } from "xray_web"
import XrayClient from "./client";
const $ = React.createElement;

const client = new XrayClient(new Worker("server.js"));
const websocketURL = "ws://127.0.0.1:9999";
client.sendMessage({ type: "ConnectToWebsocket", url: websocketURL });

const viewRegistry = buildViewRegistry(client);

let initialRender = true;
client.onMessage(message => {
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
