const {TextBuffer, TextEditor} = require('./target/debug/xray_node')

const buffer = new TextBuffer(1)
buffer.splice(0, 0, 'Hello, world!')
console.log(buffer.length);
buffer.splice(6, 0, ' cruel')
console.log(buffer.length);

console.log(buffer.getText());

let editor1 = new TextEditor(buffer, () => {
  console.log("editor 1 changed!")
})
let editor2 = new TextEditor(buffer, () => {
  console.log("editor 2 changed!")
})

console.log("Splicing...")
buffer.splice(0, 0, 'foo')

setTimeout(() => {
  editor1.destroy()
  editor2.destroy()
}, 50)
