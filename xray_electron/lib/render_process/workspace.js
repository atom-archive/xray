const React = require("react");
const ReactDOM = require("react-dom");
const View = require("./view");
const $ = React.createElement;

module.exports = class Workspace extends React.Component {
  render() {
    const modalView =
      this.props.modal == null ? null : $(View, { id: this.props.modal });

    return $("div", { id: "workspace" }, modalView);
  }
};
