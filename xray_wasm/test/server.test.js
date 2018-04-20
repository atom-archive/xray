import assert from "assert";
import xrayPromise from "../lib/index";
import { JsSender } from "../lib/support";

suite("Server", () => {
  let xray = null;

  before(async () => {
    xray = await xrayPromise;
  });

  test("smoke test", finish => {
    const pair = xray.ChannelPair.new();
    const test = xray.Test.new();
    const outgoing = new JsSender();
    const messages = [];
    outgoing.onMessage = m => messages.push(parseInt(m));
    outgoing.onFinish = () => {
      assert.deepEqual(messages, [0, 1, 2, 3, 4]);
      finish();
    };
    test.echo_stream(outgoing, pair.rx());

    const tx = pair.tx();
    let i = 0;
    let intervalId = setInterval(() => {
      if (i === 5) {
        tx.dispose();
        clearInterval(intervalId);
      }

      tx.send((i++).toString());
    }, 1);
  });
});
