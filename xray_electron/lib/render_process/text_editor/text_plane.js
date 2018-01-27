const React = require("react");
const PropTypes = require("prop-types");
const { styled } = require("styletron-react");
const $ = React.createElement;



class TextPlane extends React.Component {
  constructor(props) {
    super(props)
  }

  render() {
    return $("canvas", {
      ref: "canvas",
      className: this.props.className,
      width: this.props.width,
      height: this.props.height,
      style: {
        width: this.props.width + "px",
        height: this.props.height + "px"
      }
    });
  }

  async componentDidUpdate() {
    if (!this.gl) {
      this.gl = this.refs.canvas.getContext("webgl2");
      this.renderer = new Renderer(this.gl)
    }

    const {
      fontFamily,
      fontSize,
      lineHeight,
      backgroundColor,
      baseTextColor
    } = this.context.theme.editor;

    const computedLineHeight = Math.ceil(lineHeight * fontSize);

    const ctx = this.ctx

    // Fill background

    // Render lines
    const lines = this.props.frameState.lines;
  }

  getLineHeight() {}
}

TextPlane.contextTypes = {
  theme: PropTypes.object
};

module.exports = TextPlane;

const shaders = require('./shaders')
const TEXT_BLEND_ATTRIBUTE_NAMES = [
  "unitQuadCorner",
  "targetOrigin",
  "targetSize",
  "textColorRGBA",
  "atlasOrigin",
  "atlasSize"
]
const UNIT_QUAD_CORNERS = new Float32Array([
  1, 1,
  1, 0,
  0, 0,
  0, 1
])

class Renderer {
  constructor (gl) {
    this.gl = gl;

    const textBlendVertexShader = this.createShader(shaders.textBlendVertex, this.gl.VERTEX_SHADER)
    const textBlendPass1FragmentShader = this.createShader(shaders.textBlendPass1Fragment, this.gl.FRAGMENT_SHADER)
    const textBlendPass2FragmentShader = this.createShader(shaders.textBlendPass2Fragment, this.gl.FRAGMENT_SHADER)

    this.textBlendPass1Program = this.createProgram(textBlendVertexShader, textBlendPass1FragmentShader);
    this.textBlendPass2Program = this.createProgram(textBlendVertexShader, textBlendPass2FragmentShader);
    // Both programs declare the same attributes in the same order, so we can
    // use the same attribute locations for both.
    this.textBlendAttributeLocations = this.getAttributeLocations(this.textBlendPass1Program, TEXT_BLEND_ATTRIBUTE_NAMES)

    this.unitQuadCornersBuffer = this.gl.createBuffer();
    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.unitQuadCornersBuffer);
    this.gl.bufferData(this.gl.ARRAY_BUFFER, UNIT_QUAD_CORNERS, gl.STATIC_DRAW);
    this.gl.vertexAttribPointer(this.textBlendAttributeLocations.unitQuadCorner, 2, this.gl.FLOAT, false, 2 * UNIT_QUAD_CORNERS.BYTES_PER_ELEMENT, 0);
    this.gl.enableVertexAttribArray(this.textBlendAttributeLocations.unitQuadCorner)
  }

  createProgram (vertexShader, fragmentShader) {
    const program = this.gl.createProgram();
    this.gl.attachShader(program, vertexShader);
    this.gl.attachShader(program, fragmentShader);
    this.gl.linkProgram(program);
    this.gl.useProgram(program);
    return program
  }

  createShader (source, type) {
    const shader = this.gl.createShader(type);
    this.gl.shaderSource(shader, source);
    this.gl.compileShader(shader);
    return shader
  }

  getAttributeLocations (program, attributeNames) {
    const locations = {}
    for (let i = 0; i < attributeNames.length; i++) {
      const name = attributeNames[i];
      locations[name] = this.gl.getAttribLocation(program, name);
    }
    return locations;
  }
}
