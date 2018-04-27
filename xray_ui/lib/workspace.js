const propTypes = require("prop-types");
const React = require("react");
const ReactDOM = require("react-dom");
const { styled } = require("styletron-react");
const Modal = require("./modal");
const View = require("./view");
const $ = React.createElement;

const Root = styled("div", {
  position: "relative",
  width: "100%",
  height: "100%",
  padding: 0,
  margin: 0
});

const BackgroundTip = styled("div", {
  fontFamily: "sans-serif",
  height: "100%",
  display: "flex",
  alignItems: "center",
  justifyContent: "center"
});

class Workspace extends React.Component {
  constructor() {
    super();
    this.didKeyDown = this.didKeyDown.bind(this);
  }

  render() {
    let modal;
    if (this.props.modal) {
      modal = $(Modal, null, $(View, { id: this.props.modal }));
    }

    let centerItem;
    if (this.props.center_pane) {
      centerItem = $(View, { id: this.props.center_pane });
    } else if (this.context.inBrowser) {
      centerItem = $(BackgroundTip, {}, "Press Ctrl-T to browse files");
    }

    return $(
      Root,
      {
        tabIndex: -1,
        onKeyDownCapture: this.didKeyDown
      },
      centerItem,
      modal
    );
  }

  componentDidMount() {
    ReactDOM.findDOMNode(this).focus();
  }

  didKeyDown(event) {
    if (event.metaKey || event.ctrlKey) {
      if (event.key === "t") {
        this.props.dispatch({ type: "ToggleFileFinder" });
        event.stopPropagation();
      }
    }
  }
}

Workspace.contextTypes = {
  inBrowser: propTypes.bool
};

module.exports = Workspace;
