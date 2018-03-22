const React = require("react");
const ReactDOM = require("react-dom");
const PropTypes = require("prop-types");
const { styled } = require("styletron-react");
const TextPlane = require("./text_plane");
const debounce = require('../debounce');
const $ = React.createElement;

const CURSOR_BLINK_RESUME_DELAY = 300;
const CURSOR_BLINK_PERIOD = 800;

const Root = styled("div", {
  width: "100%",
  height: "100%",
  overflow: "hidden"
});

class TextEditor extends React.Component {
  constructor(props) {
    super(props);
    this.handleMouseWheel = this.handleMouseWheel.bind(this);
    this.handleKeyDown = this.handleKeyDown.bind(this);
    this.debouncedStartCursorBlinking = debounce(
      this.startCursorBlinking.bind(this),
      CURSOR_BLINK_RESUME_DELAY
    );

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

    element.addEventListener('wheel', this.handleMouseWheel, {passive: true});

    this.startCursorBlinking();
  }

  componentWillUnmount() {
    this.stopCursorBlinking();
    const element = ReactDOM.findDOMNode(this);
    element.removeEventListener('wheel', this.handleMouseWheel, {passive: true});
    this.state.resizeObserver.disconnect();
  }

  componentDidResize(measurements) {
    this.props.dispatch({
      type: 'SetDimensions',
      width: measurements.width,
      height: measurements.height
    })
  }

  render() {
    return $(
      Root,
      {
        tabIndex: -1,
        onKeyDown: this.handleKeyDown
      },
      $(TextPlane, {
        showCursors: this.state.showCursors,
        lineHeight: this.props.line_height,
        scrollTop: this.props.scroll_top,
        height: this.props.height,
        width: this.props.width,
        selections: this.props.selections,
        firstVisibleRow: this.props.first_visible_row,
        lines: this.props.lines
      })
    );
  }

  handleMouseWheel(event) {
    this.props.dispatch({type: 'UpdateScrollTop', delta: event.deltaY});
  }

  handleKeyDown(event) {
    if (event.key.length === 1) {
      this.props.dispatch({type: 'Edit', text: event.key});
      return;
    }

    switch (event.key) {
      case 'ArrowUp':
        this.pauseCursorBlinking();
        this.props.dispatch({type: 'MoveUp'});
        break;
      case 'ArrowDown':
        this.pauseCursorBlinking();
        this.props.dispatch({type: 'MoveDown'});
        break;
      case 'ArrowLeft':
        this.pauseCursorBlinking();
        this.props.dispatch({type: 'MoveLeft'});
        break;
      case 'ArrowRight':
        this.pauseCursorBlinking();
        this.props.dispatch({type: 'MoveRight'});
        break;
    }
  }

  pauseCursorBlinking () {
    this.stopCursorBlinking()
    this.debouncedStartCursorBlinking()
  }

  stopCursorBlinking () {
    if (this.state.cursorsBlinking) {
      window.clearInterval(this.cursorBlinkIntervalHandle)
      this.cursorBlinkIntervalHandle = null
      this.setState({
        showCursors: true,
        cursorsBlinking: false
      });
    }
  }

  startCursorBlinking () {
    if (!this.state.cursorsBlinking) {
      this.cursorBlinkIntervalHandle = window.setInterval(() => {
        this.setState({ showCursors: !this.state.showCursors });
      }, CURSOR_BLINK_PERIOD / 2);

      this.setState({
        cursorsBlinking: true,
        showCursors: false
      });
    }
  }
}

TextEditor.contextTypes = {
  theme: PropTypes.object
};

module.exports = TextEditor;
