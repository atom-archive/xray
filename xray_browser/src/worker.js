import { xray as xrayPromise, JsSink } from "xray_wasm";

const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8");
const serverPromise = xrayPromise.then(xray => new Server(xray));

global.addEventListener("message", async (event) => {
  const message = event.data;
  const server = await serverPromise;
  server.handleMessage(message);
});

class Server {
  constructor(xray) {
    this.xray = xray;
    this.xrayServer = xray.Server.new();

    this.xrayServer.start_app(
      new JsSink({
        send: buffer => {
          const message = JSON.parse(decoder.decode(buffer));
          if (message.type === "OpenWindow") {
            this.startWindow(message.window_id);
          } else {
            throw new Error("Expected first message type to be OpenWindow");
          }
        }
      })
    );
  }

  startWindow(windowId) {
    const channel = this.xray.Channel.new();
    this.windowSender = channel.take_sender();
    this.xrayServer.start_window(
      windowId,
      channel.take_receiver(),
      new JsSink({
        send(buffer) {
          global.postMessage(JSON.parse(decoder.decode(buffer)));
        }
      })
    );
  }

  connectToWebsocket(url) {
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    const channel = this.xray.Channel.new();
    const sender = channel.take_sender();

    socket.addEventListener("message", function(event) {
      const data = new Uint8Array(event.data);
      sender.send(data);
    });

    this.xrayServer.connect_to_peer(
      channel.take_receiver(),
      new JsSink({
        send(message) {
          socket.send(message);
        }
      })
    );
  }

  handleMessage(message) {
    switch (message.type) {
      case "ConnectToWebsocket":
        this.connectToWebsocket(message.url);
        break;
      case "Action":
        this.windowSender.send(encoder.encode(JSON.stringify(message)));
        break;
      default:
        console.error("Received unknown message", message);
    }
  }
}
