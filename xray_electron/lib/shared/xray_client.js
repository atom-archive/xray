const net = require("net");
const EventEmitter = require('events');

module.exports =
class XrayClient {
  constructor () {
    this.socket = null;
    this.emitter = new EventEmitter();
    this.currentMessageFragments = [];
  }

  start (socketPath) {
    return new Promise((resolve, reject) => {
      this.socket = net.connect(socketPath, resolve);
      this.socket.on('data', this._handleInput.bind(this));
      this.socket.on('error', reject)
    })
  }

  sendMessage (message) {
    this.socket.write(JSON.stringify(message));
    this.socket.write('\n');
  }

  addMessageListener (callback) {
    this.emitter.on('message', callback);
  }

  removeMessageListener (callback) {
    this.emitter.removeListener('message', callback);
  }

  _handleInput (input) {
    let searchStartIndex = 0;
    while (searchStartIndex < input.length) {
      const newlineIndex = input.indexOf('\n', searchStartIndex);
      if (newlineIndex !== -1) {
        this.currentMessageFragments.push(input.slice(searchStartIndex, newlineIndex));
        this.emitter.emit('message', JSON.parse(Buffer.concat(this.currentMessageFragments)));
        this.currentMessageFragments.length = 0;
        searchStartIndex = newlineIndex + 1;
      } else {
        this.currentMessageFragments.push(input.slice(searchStartIndex));
        break;
      }
    }
  }
}
