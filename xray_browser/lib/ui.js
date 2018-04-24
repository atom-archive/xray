import XrayClient from "./client";

const client = new XrayClient(new Worker("server.js"));
const websocketURL = "ws://127.0.0.1:9999";
client.sendMessage({ type: "ConnectToWebsocket", url: websocketURL });

client.onMessage(message => {
  console.log("message", message);
});
