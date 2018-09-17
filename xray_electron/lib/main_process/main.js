const { app, BrowserWindow } = require('electron');
const { spawn } = require('child_process');
const path = require('path');
const url = require('url');
const XrayClient = require('../shared/xray_client');

const SERVER_PATH = process.env.XRAY_SERVER_PATH;
if (!SERVER_PATH) {
  console.error('Missing XRAY_SERVER_PATH environment variable');
  process.exit(1);
}

const SOCKET_PATH = process.env.XRAY_SOCKET_PATH;
if (!SOCKET_PATH) {
  console.error('Missing XRAY_SOCKET_PATH environment variable');
  process.exit(1);
}

class XrayApplication {
  constructor(serverPath, socketPath) {
    this.serverPath = serverPath;
    this.socketPath = socketPath;
    this.windowsById = new Map();
    this.readyPromise = new Promise(resolve => app.on('ready', resolve));
    this.xrayClient = new XrayClient();
  }

  async start() {
    const serverProcess = spawn(this.serverPath, [], { stdio: ['ignore', 'pipe', 'inherit'] });
    app.on('before-quit', () => serverProcess.kill());

    serverProcess.on('error', console.error);
    serverProcess.on('exit', () => app.quit());

    await new Promise(resolve => {
      let serverStdout = '';
      serverProcess.stdout.on('data', data => {
        serverStdout += data.toString('utf8');
        if (serverStdout.includes('Listening\n')) resolve()
      });
    });

    await this.xrayClient.start(this.socketPath);
    this.xrayClient.addMessageListener(this._handleMessage.bind(this));
    this.xrayClient.sendMessage({ type: 'StartApp' });
  }

  async _handleMessage(message) {
    await this.readyPromise;
    switch (message.type) {
      case 'OpenWindow': {
        this._createWindow(message.window_id);
        break;
      }
    }
  }

  _createWindow(windowId) {
    const window = new BrowserWindow({
      width: 800, height: 600, webSecurity: false,
      icon: path.join(__dirname, '../../resources/app-icons/png/64.png')
    });
    window.loadURL(url.format({
      pathname: path.join(__dirname, '../../index.html'),
      search: `windowId=${windowId}&socketPath=${encodeURIComponent(this.socketPath)}`,
      protocol: 'file:',
      slashes: true
    }));
    this.windowsById.set(windowId, window);
    window.on('closed', () => {
      this.windowsById.delete(windowId);
      this.xrayClient.sendMessage({ type: 'CloseWindow', window_id: windowId });
    });
  }
}

app.commandLine.appendSwitch("enable-experimental-web-platform-features");

app.on('window-all-closed', function () {
  if (process.platform !== 'darwin') {
    app.quit();
  }
});

const application = new XrayApplication(SERVER_PATH, SOCKET_PATH);
application.start().then(() => {
  console.log('Listening');
});
