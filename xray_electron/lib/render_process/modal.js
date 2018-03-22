const React = require("react");
const { styled } = require("styletron-react");
const $ = React.createElement;

const Root = styled("div", {
  position: "absolute",
  top: 0,
  left: 0,
  right: 0,
  width: "min-content",
  margin: "auto"
});

module.exports = class Modal extends React.Component {
  render() {
    return $(Root, { tabIndex: -1 }, this.props.children);
  }
}
