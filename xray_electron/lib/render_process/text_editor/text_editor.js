const React = require("react");
const ReactDOM = require("react-dom");
const PropTypes = require("prop-types");
const { styled } = require("styletron-react");
const TextPlane = require("./text_plane");
const $ = React.createElement;

class TextEditorContainer extends React.Component {
  constructor(props) {
    super(props);
    this.onWheel = this.onWheel.bind(this);

    if (props.initialText) {
      buffer.splice(0, 0, props.initialText);
    }

    this.state = {
      resizeObserver: new ResizeObserver(([{contentRect}]) =>
        this.componentDidResize({width: contentRect.width, height: contentRect.height})
      ),
      scrollTop: 0,
      height: 0,
      width: 0,
      showCursors: true
    };
  }

  componentDidMount() {
    const element = ReactDOM.findDOMNode(this);
    this.state.resizeObserver.observe(element);
    this.componentDidResize({
      width: element.offsetWidth,
      height: element.offsetHeight
    });

    element.addEventListener('wheel', this.onWheel, {passive: true});

    this.state.cursorBlinkIntervalHandle = window.setInterval(() => {
      this.setState({ showCursors: !this.state.showCursors });
    }, 500);
  }

  componentWillUnmount() {
    const element = ReactDOM.findDOMNode(this);
    element.removeEventListener('wheel', this.onWheel, {passive: true});
    this.state.resizeObserver.disconnect();
    window.clearInterval(this.state.cursorBlinkIntervalHandle);
  }

  componentDidResize(measurements) {
    this.setState(measurements)
    this.props.dispatch({
      type: 'SetDimensions',
      width: measurements.width,
      height: measurements.height
    })
  }

  render() {
    const { scrollTop, width, height, showCursors } = this.state;

    return $(TextEditor, {
      scrollTop,
      width,
      height,
      showCursors,
      frameState: this.props
    })
  }

  onWheel (event) {
    const scrollTop = Math.max(0, this.state.scrollTop + event.deltaY)
    this.setState({scrollTop})
    this.props.dispatch({type: 'SetScrollTop', value: scrollTop})
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
      showCursors: props.showCursors,
      frameState: props.frameState
    })
  );
}

module.exports = TextEditorContainer;
