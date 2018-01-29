const React = require("react");
const PropTypes = require("prop-types");
const { styled } = require("styletron-react");
const $ = React.createElement;

class TextPlane extends React.Component {
  constructor(props) {
    super(props);
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
      this.renderer = new Renderer(this.gl);
    }

    const {
      fontFamily,
      fontSize,
      lineHeight,
      backgroundColor,
      baseTextColor
    } = this.context.theme.editor;

    const computedLineHeight = Math.ceil(lineHeight * fontSize);

    const ctx = this.ctx;

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

const shaders = require("./shaders");
const UNIT_QUAD_VERTICES = new Float32Array([1, 1, 1, 0, 0, 0, 0, 1]);
const UNIT_QUAD_ELEMENT_INDICES = new Float32Array([0, 1, 3, 1, 2, 3]);
const MAX_GLYPH_INSTANCES = 1 << 16;
const GLYPH_INSTANCE_SIZE_IN_BYTES = 12 * Float32Array.BYTES_PER_ELEMENT;

class Renderer {
  constructor(gl) {
    this.gl = gl;
    this.atlas = new Atlas(gl);

    const textBlendVertexShader = this.createShader(
      shaders.textBlendVertex,
      this.gl.VERTEX_SHADER
    );
    const textBlendPass1FragmentShader = this.createShader(
      shaders.textBlendPass1Fragment,
      this.gl.FRAGMENT_SHADER
    );
    const textBlendPass2FragmentShader = this.createShader(
      shaders.textBlendPass2Fragment,
      this.gl.FRAGMENT_SHADER
    );

    this.textBlendPass1Program = this.createProgram(
      textBlendVertexShader,
      textBlendPass1FragmentShader
    );
    this.textBlendPass2Program = this.createProgram(
      textBlendVertexShader,
      textBlendPass2FragmentShader
    );

    this.textBlendVAO = this.gl.createVertexArray();
    this.gl.bindVertexArray(this.textBlendVAO);

    this.unitQuadVerticesBuffer = this.gl.createBuffer();
    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.unitQuadVerticesBuffer);
    this.gl.bufferData(
      this.gl.ARRAY_BUFFER,
      UNIT_QUAD_VERTICES,
      gl.STATIC_DRAW
    );

    this.unitQuadElementIndicesBuffer = this.gl.createBuffer();
    this.gl.bindBuffer(
      this.gl.ELEMENT_ARRAY_BUFFER,
      this.unitQuadElementIndicesBuffer
    );
    this.gl.bufferData(
      this.gl.ELEMENT_ARRAY_BUFFER,
      UNIT_QUAD_ELEMENT_INDICES,
      gl.STATIC_DRAW
    );

    this.gl.enableVertexAttribArray(shaders.attributes.unitQuadVertex);
    this.gl.vertexAttribPointer(
      shaders.attributes.unitQuadVertex,
      2,
      this.gl.FLOAT,
      false,
      0,
      0
    );

    this.glyphInstances = new Float32Array(MAX_GLYPH_INSTANCES);
    this.glyphInstancesBuffer = this.gl.createBuffer();
    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.glyphInstancesBuffer);
    this.gl.bufferData(
      this.gl.ARRAY_BUFFER,
      this.glyphInstances,
      this.gl.STREAM_DRAW
    );

    this.gl.enableVertexAttribArray(shaders.attributes.targetOrigin);
    this.gl.vertexAttribPointer(
      shaders.attributes.targetOrigin,
      2,
      this.gl.FLOAT,
      false,
      GLYPH_INSTANCE_SIZE_IN_BYTES,
      0
    );
    this.gl.vertexAttribDivisor(shaders.attributes.targetOrigin, 1);

    this.gl.enableVertexAttribArray(shaders.attributes.targetSize);
    this.gl.vertexAttribPointer(
      shaders.attributes.targetSize,
      2,
      this.gl.FLOAT,
      false,
      GLYPH_INSTANCE_SIZE_IN_BYTES,
      2 * Float32Array.BYTES_PER_ELEMENT
    );
    this.gl.vertexAttribDivisor(shaders.attributes.targetSize, 1);

    this.gl.enableVertexAttribArray(shaders.attributes.textColorRGBA);
    this.gl.vertexAttribPointer(
      shaders.attributes.textColorRGBA,
      4,
      this.gl.FLOAT,
      false,
      GLYPH_INSTANCE_SIZE_IN_BYTES,
      4 * Float32Array.BYTES_PER_ELEMENT
    );
    this.gl.vertexAttribDivisor(shaders.attributes.textColorRGBA, 1);

    this.gl.enableVertexAttribArray(shaders.attributes.atlasOrigin);
    this.gl.vertexAttribPointer(
      shaders.attributes.atlasOrigin,
      2,
      this.gl.FLOAT,
      false,
      GLYPH_INSTANCE_SIZE_IN_BYTES,
      8 * Float32Array.BYTES_PER_ELEMENT
    );
    this.gl.vertexAttribDivisor(shaders.attributes.atlasOrigin, 1);

    this.gl.enableVertexAttribArray(shaders.attributes.atlasSize);
    this.gl.vertexAttribPointer(
      shaders.attributes.atlasSize,
      2,
      this.gl.FLOAT,
      false,
      GLYPH_INSTANCE_SIZE_IN_BYTES,
      10 * Float32Array.BYTES_PER_ELEMENT
    );
    this.gl.vertexAttribDivisor(shaders.attributes.atlasSize, 1);
  }

  createProgram(vertexShader, fragmentShader) {
    const program = this.gl.createProgram();
    this.gl.attachShader(program, vertexShader);
    this.gl.attachShader(program, fragmentShader);
    this.gl.linkProgram(program);
    if (!this.gl.getProgramParameter(program, this.gl.LINK_STATUS)) {
      var info = this.gl.getProgramInfoLog(program);
      throw "Could not compile WebGL program: \n\n" + info;
    }
    this.gl.useProgram(program);
    return program;
  }

  createShader(source, type) {
    const shader = this.gl.createShader(type);
    this.gl.shaderSource(shader, source);
    this.gl.compileShader(shader);

    if (!this.gl.getShaderParameter(shader, this.gl.COMPILE_STATUS)) {
      var info = this.gl.getShaderInfoLog(shader);
      throw "Could not compile WebGL program: \n\n" + info;
    }

    return shader;
  }

  getAttributeLocations(program, attributeNames) {
    const locations = {};
    for (let i = 0; i < attributeNames.length; i++) {
      const name = attributeNames[i];
      locations[name] = this.gl.getAttribLocation(program, name);
    }
    return locations;
  }
}

class Atlas {
  constructor(gl) {
    const size = 512;

    this.texture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.SRGB8,
      size,
      size,
      0,
      gl.RGB,
      gl.UNSIGNED_BYTE,
      new ImageData(size, size)
    );

    this.gl = gl;
    this.glyphCanvas = document.createElement("canvas");
    this.glyphCanvas.width = size;
    this.glyphCanvas.height = size;
    this.glyphCtx = this.glyphCanvas.getContext("2d", { alpha: false });
    this.uvScale = 1 / size;
  }

  getGlyph(text) {
    this.glyphCtx.fillStyle = "white";
    this.glyphCtx.fillRect(
      0,
      0,
      this.glyphCanvas.width,
      this.glyphCanvas.height
    );
    this.glyphCtx.fillStyle = "black";
    this.glyphCtx.fillText(text, 0, 0);
  }
}
