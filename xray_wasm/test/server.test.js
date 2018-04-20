const Server = require('..');

suite("Server", () => {
  test("smoke test", () => {
    Server.greet("World");
  });
});
