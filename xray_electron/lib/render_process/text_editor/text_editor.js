const React = require("react");
const ReactDOM = require("react-dom");
const { styled } = require("styletron-react");
const xray = require("xray");
const ContentCanvas = require("./content_canvas");
const $ = React.createElement;

module.exports = class TextEditorContainer extends React.Component {
  render() {
    return $(TextEditor, {
      offsetWidth: this.state ? this.state.offsetWidth : 0,
      offsetHeight: this.state? this.state.offsetHeight : 0,
      contentCanvasCreated: canvas => (this.contentCanvas = canvas)
    });
  }

  componentDidMount() {
    const node = ReactDOM.findDOMNode(this);

    this.setState({
      offsetWidth: node.offsetWidth,
      offsetHeight: node.offsetHeight
    });
  }
};

const ContentScroller = styled("div", {
  width: "100%",
  height: "100%",
  overflow: "auto"
});

const StickyContentCanvas = styled(ContentCanvas, {
  position: "sticky",
  top: 0,
  left: 0
});

const DummyContent = props => {
  return $("div", {
    style: {
      width: props.width + "px",
      height: props.height + "px"
    }
  });
};

function TextEditor(props) {
  return $(
    ContentScroller,
    null,
    $(StickyContentCanvas, {
      created: props.contentCanvasCreated,
      width: props.offsetWidth,
      height: props.offsetHeight,
      scale: window.devicePixelRatio
    }),
    DummyContent({ width: 3000, height: 3000 })
  );
}
