const {app, BrowserWindow} = require('electron');
const {spawn} = require('child_process');
const path = require('path');
const url = require('url');
const net = require('net');

const SOCKET_PATH = process.env.XRAY_SOCKET_PATH;
if (!SOCKET_PATH) {
  console.error('Missing XRAY_SOCKET_PATH environment variable');
  process.exit(1);
}

const SERVER_PATH = path.join(__dirname, '..', '..', '..', 'target', 'debug', 'xray_server');

const socketPromise = new Promise((resolve, reject) => {
  let serverProcess = spawn(SERVER_PATH, [], {stdio: ['ignore', 'pipe', 'inherit']});
  app.on('before-quit', () => serverProcess.kill());

  let serverStdout = '';
  serverProcess.stdout.on('data', data => {
    serverStdout += data.toString('utf8');
    if (serverStdout.includes('Listening\n')) {
      const socket = net.connect(SOCKET_PATH, () => resolve(socket));
      socket.on('error', reject);
    }
  });

  serverProcess.on('error', reject);

  serverProcess.on('exit', () => app.quit());
});

const readyPromise = new Promise(resolve =>
  app.on('ready', resolve)
);

Promise.all([socketPromise, readyPromise]).then(([socket]) =>
  (new XrayApplication(SOCKET_PATH, socket)).start()
);

class XrayApplication {
  constructor (socketPath, serverSocket) {
    this.socketPath;
    this.serverSocket = serverSocket;
    this.messageChunks = [];
    this.windowsByWorkspaceId = new Map();
  }

  start () {
    this.serverSocket.on('data', this.handleChunk);
    this.sendMessage({type: 'StartApplication'});

    const initialMessage = process.env.XRAY_INITIAL_MESSAGE;
    if (initialMessage) {
      this.sendMessage(JSON.parse(initialMessage));
    }
  }

  sendMessage (message) {
    this.serverSocket.write(JSON.stringify(message));
    this.serverSocket.write('\n');
  }

  handleMessage (message) {
    switch (message.type) {
      case 'OpenWindow': {
        const workspaceId = message.workspaceId;
        this.createWindow(workspaceId);
        break;
      }
    }
  }

  handleChunk (chunk) {
    const newlineIndex = chunk.indexOf('\n');
    if (newlineIndex !== -1) {
      this.messageChunks.push(chunk.slice(0, newlineIndex));
      this.handleMessage(JSON.parse(Buffer.concat(this.messageChunks)));
      this.messageChunks.length = 0;
      if (newlineIndex + 1 < chunk.length) {
        this.messageChunks.push(chunk.slice(newlineIndex + 1));
      }
    } else {
      this.messageChunks.push(chunk);
    }
  }

  createWindow (workspaceId) {
    const window = new BrowserWindow({width: 800, height: 600});
    window.loadURL(url.format({
      pathname: path.join(__dirname, `../../index.html?workspaceId=${workspaceId}&socketPath=${this.socketPath}`),
      protocol: 'file:',
      slashes: true
    }));
    window.on('closed', () => this.windowsByWorkspaceId.delete(workspaceId));
  }
}

app.commandLine.appendSwitch("enable-experimental-web-platform-features");

app.on('window-all-closed', function () {
  if (process.platform !== 'darwin') {
    app.quit();
  }
});
