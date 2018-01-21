const testModule = require('./target/release/test_module')

function testSpawn() {
  console.log('=== Test spawning a future on libuv event loop')
  return testModule.testSpawn()
}

function testThrow() {
  console.log('=== Test throwing from Rust')
  testModule.testThrow()
}

testSpawn().then(testThrow)

