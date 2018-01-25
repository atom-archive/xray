const React = require("react");
const PropTypes = require("prop-types");
const { styled } = require("styletron-react");
const $ = React.createElement;

class ContentCanvas extends React.Component {
  constructor(props) {
    super(props)

    this.glyphAtlas = new GlyphAtlas();
  }

  render() {
    return $("canvas", {
      ref: "canvas",
      className: this.props.className,
      width: this.props.width * window.devicePixelRatio,
      height: this.props.height * window.devicePixelRatio,
      style: {
        width: this.props.width + "px",
        height: this.props.height + "px"
      }
    });
  }

  async componentDidUpdate() {
    this.props.created(this);

    if (!this.ctx) {
      this.ctx = this.refs.canvas.getContext("2d", { alpha: false });
      this.ctx.scale(window.devicePixelRatio, window.devicePixelRatio);
    }

    const {
      fontFamily,
      fontSize,
      lineHeight,
      backgroundColor,
      baseTextColor
    } = this.context.theme.editor;

    const computedLineHeight = Math.ceil(lineHeight * fontSize);

    if (!this.glyphAtlas.bitmap) {
      await this.glyphAtlas.generate({
        font: `${fontSize}px ${fontFamily}`,
        computedLineHeight,
        fillStyle: baseTextColor
      });
    }

    const ctx = this.ctx

    // Fill background
    ctx.fillStyle = backgroundColor;
    ctx.fillRect(0, 0, this.props.width, this.props.height);

    // Render lines
    const lines = this.props.frameState.lines;
    for (let i = 0; i < lines.length; i++) {
      let x = 0;
      const y = computedLineHeight * i;
      const line = lines[i];
      for (let j = 0; j < line.length; j++) {
        const {
          x: sourceX,
          y: sourceY,
          width,
          height
        } = this.glyphAtlas.get(line[j]);

        ctx.drawImage(
          this.glyphAtlas.bitmap,
          sourceX,
          sourceY,
          width,
          height,
          x,
          y,
          width,
          height
        );

        x += width;
      }
    }
  }

  getLineHeight() {}
}

ContentCanvas.contextTypes = {
  theme: PropTypes.object
};

class GlyphAtlas {
  constructor(style) {
    const width = 1000;
    const height = 1000;

    this.atlasEntriesByString = new Map();

    const canvas = document.createElement("canvas");
    this.canvas = canvas;
    canvas.width = width * 2;
    canvas.height = height * 2;
    canvas.style.width = width;
    canvas.style.height = height;
  }

  async generate (style) {
    const canvas = this.canvas
    this.context = canvas.getContext("2d", { alpha: false });
    this.context.scale(window.devicePixelRatio, window.devicePixelRatio);
    this.context.fillStyle = "white";
    this.context.fillRect(0, 0, canvas.width, canvas.height);

    this.context.font = style.font;
    this.context.fillStyle = style.fillStyle;

    let x = 0;
    for (let i = 0; i < 255; i++) {
      const s = String.fromCharCode(i);
      const { width } = this.context.measureText(s);
      this.context.fillText(s, x, style.computedLineHeight);
      this.atlasEntriesByString.set(s, { x, y: 0, width, height: style.computedLineHeight });
      x += width;
    }

    this.bitmap = await window.createImageBitmap(this.canvas);
  }

  get(string, style) {
    if (!this.bitmap) throw new Error("Must wait for bitmap to be generated");

    let atlasEntry = this.atlasEntriesByString.get(string);
    if (!atlasEntry) {
      this.bitmap = null
    }

    return atlasEntry
  }
}

module.exports = ContentCanvas;
