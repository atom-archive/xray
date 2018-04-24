import { xray as xrayPromise, JsSink } from "xray_wasm";

const serverPromise = xrayPromise.then(xray => new Server(xray));

global.addEventListener("message", handleMessage);

async function handleMessage(event) {
  const message = event.data;
  const server = await serverPromise;
  switch (message.type) {
    case "ConnectToWebsocket":
      server.connectToWebsocket(message.url);
      break;
    default:
      console.log("Received unknown message", message);
  }
}

class Server {
  constructor(xray) {
    this.xray = xray;
    this.xrayServer = xray.Server.new();
  }

  connectToWebsocket(url) {
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    const channel = this.xray.Channel.new();
    const sender = channel.take_sender();
    const receiver = channel.take_receiver();

    console.log("connect", url);

    socket.addEventListener('message', function (event) {
      const data = new Uint8Array(event.data);
      console.log("receive message", data);
      sender.send(data);
    });

    const sink = new JsSink({
      send(message) {
        console.log("send message", message);
        socket.send(message);
      },
    })

    this.xrayServer.connect_to_peer(receiver, sink)
  }
}
