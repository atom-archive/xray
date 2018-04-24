import XrayClient from "./client";

const server = new Worker("server.js");
const client = new XrayClient(server);
const websocketURL = "ws://127.0.0.1:9999";
client.sendMessage({ type: "ConnectToWebsocket", url: websocketURL });
client.onMessage(message => {});
