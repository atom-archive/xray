const propTypes = require("prop-types");
const React = require("react");
const ReactDOM = require("react-dom");
const { styled } = require("styletron-react");
const Modal = require("./modal");
const View = require("./view");
const { ActionContext, Action } = require("./keymap");
const $ = React.createElement;

const Root = styled("div", {
  position: "relative",
  width: "100%",
  height: "100%",
  padding: 0,
  margin: 0,
  display: "flex"
});

const LeftPanel = styled("div", {
  width: "300px",
  height: "100%"
});

const Pane = styled("div", {
  flex: 1,
  position: "relative"
});

const PaneInner = styled("div", {
  position: "absolute",
  left: 0,
  top: 0,
  bottom: 0,
  right: 0
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
  }

  render() {
    let modal;
    if (this.props.modal) {
      modal = $(Modal, null, $(View, { id: this.props.modal }));
    }

    let leftPanel;
    if (this.props.left_panel) {
      leftPanel = $(LeftPanel, null, $(View, { id: this.props.left_panel }));
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
        tabIndex: -1
      },
      $(
        ActionContext,
        { context: "Workspace" },
        leftPanel,
        $(Pane, null, $(PaneInner, null, centerItem)),
        modal,
        $(Action, { type: "ToggleFileFinder" }),
        $(Action, { type: "SaveActiveBuffer" })
      )
    );
  }

  componentDidMount() {
    ReactDOM.findDOMNode(this).focus();
  }
}

Workspace.contextTypes = {
  inBrowser: propTypes.bool
};

module.exports = Workspace;
