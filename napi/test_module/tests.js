const testModule = require('./target/debug/test_module')

function testSpawn() {
  console.log('=== Test spawning a future on libuv event loop')
  return testModule.testSpawn()
}

function testThrow() {
  console.log('=== Test throwing from Rust')
  try {
    testModule.testThrow()
  } catch (e) {
    return
  }
  console.error('Expected function to throw an error')
  process.exit(1)
}

testSpawn().then(testThrow)
