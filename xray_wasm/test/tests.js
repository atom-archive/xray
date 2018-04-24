import assert from "assert";
import { xray as xrayPromise, JsSink } from "../lib/main";

suite("Server", () => {
  let xray;

  before(async () => {
    xray = await xrayPromise;
  });

  test("channels and sinks", endTest => {
    const test = xray.Test.new();

    const messages = [];
    const sink = new JsSink({
      send(message) {
        messages.push(message);
      },

      close() {
        assert.deepEqual(messages, [0, 1, 2, 3, 4]);
        endTest();
      }
    });

    const channel = xray.Channel.new();
    test.echo_stream(channel.take_receiver(), sink);

    const sender = channel.take_sender();
    let i = 0;
    let intervalId = setInterval(() => {
      if (i === 5) {
        sender.dispose();
        clearInterval(intervalId);
      }
      sender.send((i++).toString());
    }, 1);
  });
});
