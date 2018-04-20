export class JsSender {
  onMessage() {}

  onFinish() {}

  send(message) {
    this.onMessage(message);
  }

  finish() {
    this.onFinish();
  }
}

let promise = Promise.resolve();
export function notifyOnNextTick(notifyHandle, id) {
  promise.then(() => notifyHandle.notify_on_next_tick(id));
}
