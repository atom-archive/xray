const React = require("react");
const ReactDOM = require("react-dom");
const { styled } = require("styletron-react");
const $ = React.createElement;

const Root = styled("div", {
  position: "absolute",
  top: 0,
  left: 0,
  right: 0,
  width: "min-content",
  margin: "auto",
  outline: "none"
});

module.exports = class Modal extends React.Component {
  render() {
    return $(Root, { tabIndex: -1 }, this.props.children);
  }

  componentDidMount() {
    this.previouslyFocusedElement = document.activeElement;
  }

  componentWillUnmount() {
    const element = ReactDOM.findDOMNode(this);
    if (element.contains(document.activeElement)) {
      this.previouslyFocusedElement.focus();
      this.previouslyFocusedElement = null
    }
  }
};
