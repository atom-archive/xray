export default class XrayClient {
  constructor(worker) {
    this.worker = worker;
  }

  onMessage(callback) {
    this.worker.addEventListener("message", message => {
      callback(message.data);
    });
  }

  sendMessage(message) {
    this.worker.postMessage(message);
  }
}
