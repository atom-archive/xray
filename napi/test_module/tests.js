const testModule = require('./target/release/test_module')

console.log('=== Test spawning a future on libuv event loop');
testModule.testSpawn()
