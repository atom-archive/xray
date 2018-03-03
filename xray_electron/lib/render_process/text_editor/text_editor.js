const React = require("react");
const ReactDOM = require("react-dom");
const PropTypes = require("prop-types");
const { styled } = require("styletron-react");
const xray = require("xray");
const TextPlane = require("./text_plane");
const $ = React.createElement;

class TextEditorContainer extends React.Component {
  constructor(props) {
    super(props);

    const buffer = new xray.TextBuffer(1);
    const editor = new xray.TextEditor(buffer, this.editorChanged.bind(this));

    if (props.initialText) {
      buffer.splice(0, 0, props.initialText);
    }

    this.state = {
      resizeObserver: new ResizeObserver((entries) => this.componentDidResize(entries[0].contentRect)),
      editor: editor,
      scrollTop: 0,
      height: 0,
      width: 0,
      editorVersion: 0
    };
  }

  componentDidMount() {
    const element = ReactDOM.findDOMNode(this);
    this.state.resizeObserver.observe(element);
    this.componentDidResize({
      width: element.offsetWidth,
      height: element.offsetHeight
    });
  }

  componentWillUnmount() {
    this.state.editor.destroy();
    this.state.resizeObserver.disconnect();
  }

  componentDidResize({width, height}) {
    this.setState({width, height});
  }

  editorChanged() {
    this.setState({
      editorVersion: this.state.editorVersion + 1
    });
  }

  computeFrameState() {
    const { scrollTop, height } = this.state;
    const { fontSize, lineHeight } = this.context.theme.editor;
    return this.state.editor.render({
      scrollTop,
      height,
      lineHeight: fontSize * lineHeight
    });
  }

  render() {
    const { scrollTop, width, height } = this.state;

    return $(TextEditor, {
      scrollTop,
      width,
      height,
      frameState: this.computeFrameState(),
      onWheel: event => {
        this.setState({
          scrollTop: Math.max(0, this.state.scrollTop + event.deltaY)
        });
      }
    });
  }
}

TextEditorContainer.contextTypes = {
  theme: PropTypes.object
};

const Root = styled("div", {
  width: "100%",
  height: "100%",
  overflow: "hidden"
});

function TextEditor(props) {
  return $(
    Root,
    {onWheel: props.onWheel},
    $(TextPlane, {
      scrollTop: props.scrollTop,
      width: props.width,
      height: props.height,
      frameState: props.frameState
    })
  );
}

module.exports = TextEditorContainer;
