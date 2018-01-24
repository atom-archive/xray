const React = require("react");
const PropTypes = require("prop-types");
const { styled } = require("styletron-react");
const $ = React.createElement;

class ContentCanvas extends React.Component {
  render() {
    return $("canvas", {
      ref: "canvas",
      className: this.props.className,
      width: this.props.width * this.props.scale,
      height: this.props.height * this.props.scale,
      style: {
        width: this.props.width + "px",
        height: this.props.height + "px"
      }
    });
  }

  componentDidUpdate() {
    this.props.created(this);

    const {
      fontFamily,
      fontSize,
      lineHeight,
      backgroundColor,
      baseTextColor
    } = this.context.theme.editor;

    const ctx = this.refs.canvas.getContext("2d", { alpha: false });
    ctx.scale(this.props.scale, this.props.scale);

    // Fill background
    ctx.fillStyle = backgroundColor;
    ctx.fillRect(0, 0, this.props.width, this.props.height);

    // Fill text
    ctx.fillStyle = baseTextColor;
    ctx.font = `${fontSize}px ${fontFamily}`;
    ctx.fillText("Hello, world!", 0, fontSize * lineHeight * 1);
    ctx.fillText("This is text rendered to a canvas. 🎨 ==> 💥", 0, fontSize * lineHeight * 2);
  }

  getLineHeight() {}
};

ContentCanvas.contextTypes = {
  theme: PropTypes.object
}

module.exports = ContentCanvas;
