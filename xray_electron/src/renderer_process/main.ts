import {TextBuffer, TextEditor} from 'xray';

const buffer = new TextBuffer(1);

global["editor"] = new TextEditor(buffer, () => {
  console.log(buffer.getText())
});

document.addEventListener("keydown", (event) => {
  buffer.splice(buffer.length, 0, event.key);
})
