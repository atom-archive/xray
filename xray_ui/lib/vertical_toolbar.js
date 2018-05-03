const propTypes = require("prop-types");
const React = require("react");
const { styled } = require("styletron-react");
const $ = React.createElement;
const Octicon = require("react-component-octicons").default;

const Root = styled("div", {
  backgroundColor: "rgb(234, 234, 235)",
  width: "36px",
  display: "flex",
  flexDirection: "column",
  alignItems: "center"
});

const Icon = styled(Octicon, {
  ":hover": {
    fill: "rgba(31, 150, 255, 1.0)"
  },
  cursor: "pointer",
  marginTop: "8px"
});

module.exports = class VerticalToolbar extends React.Component {
  render() {
    return $(
      Root,
      null,
      $(
        "div",
        { onClick: this.props.onToggleDiscussion },
        $(Icon, {
          name: "comment",
          zoom: "x1.25"
        })
      )
    );
  }
};
