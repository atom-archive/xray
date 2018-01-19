const {TextBuffer, TextEditor} = require('./target/debug/xray_node')

const buffer = new TextBuffer(1)
buffer.splice(0, 0, 'Hello, world!')
console.log(buffer.length);
buffer.splice(6, 0, ' cruel')
console.log(buffer.length);

console.log(buffer.getText());

const editor1 = new TextEditor(buffer)
const editor2 = new TextEditor(buffer)
