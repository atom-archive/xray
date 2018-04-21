export class JsSink {
  constructor({ send, close }) {
    if (send) this._send = send;
    if (close) this._close = close;
  }

  send(message) {
    if (this._send) this._send(message)
  }

  close() {
    if (this._close) this._close()
  }
}

let promise = Promise.resolve();
export function notifyOnNextTick(notifyHandle, id) {
  promise.then(() => notifyHandle.notify_from_js_on_next_tick(id));
}
