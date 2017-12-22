const proton = require('./target/debug/proton.node')

let buffer = new proton.TextBuffer(1)
buffer.splice(0, 0, 'Hello, world!')
console.log(buffer.length);
buffer.splice(6, 0, ' cruel')
console.log(buffer.length);

console.log(buffer.getText());
