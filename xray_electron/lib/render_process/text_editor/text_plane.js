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
      width: this.props.width * window.devicePixelRatio,
      height: this.props.height * window.devicePixelRatio,
      style: {
        width: this.props.width + "px",
        height: this.props.height + "px"
      }
    });
  }

  async componentDidUpdate() {
    const {
      fontFamily,
      fontSize,
      lineHeight,
      backgroundColor,
      baseTextColor
    } = this.context.theme.editor;

    const computedLineHeight = Math.ceil(lineHeight * fontSize);

    if (!this.gl) {
      this.gl = this.refs.canvas.getContext("webgl2");
      this.renderer = new Renderer(this.gl, {
        fontFamily,
        fontSize,
        backgroundColor,
        baseTextColor,
        computedLineHeight,
        dpiScale: window.devicePixelRatio
      });
    }

    this.renderer.drawLines(this.props.frameState.lines);
  }

  getLineHeight() {}
}

TextPlane.contextTypes = {
  theme: PropTypes.object
};

module.exports = TextPlane;

const shaders = require("./shaders");
const UNIT_QUAD_VERTICES = new Float32Array([1, 1, 1, 0, 0, 0, 0, 1]);
const UNIT_QUAD_ELEMENT_INDICES = new Uint8Array([0, 1, 3, 1, 2, 3]);
const MAX_GLYPH_INSTANCES = 1 << 16;
const GLYPH_INSTANCE_SIZE_IN_BYTES = 12 * Float32Array.BYTES_PER_ELEMENT;
const SUBPIXEL_DIVISOR = 4;

class Renderer {
  constructor(gl, style) {
    this.gl = gl;
    this.gl.enable(this.gl.BLEND);
    this.atlas = new Atlas(gl, style);
    this.style = style;

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

    // this.textBlendVAO = this.gl.createVertexArray();
    // this.gl.bindVertexArray(this.textBlendVAO);

    this.unitQuadVerticesBuffer = this.gl.createBuffer();
    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.unitQuadVerticesBuffer);
    this.gl.bufferData(
      this.gl.ARRAY_BUFFER,
      UNIT_QUAD_VERTICES,
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

  drawLines(lines) {
    let instances = 0;
    let y = 0;
    for (var i = 0; i < lines.length; i++) {
      let x = 0;
      const line = lines[i];
      for (var j = 0; j < line.length; j++) {
        const char = line[j];
        const variantIndex =
          Math.round(x * SUBPIXEL_DIVISOR) % SUBPIXEL_DIVISOR;
        const glyph = this.atlas.getGlyph(char, variantIndex);

        // targetOrigin
        this.glyphInstances[0 + 12 * instances] = Math.round(x - glyph.variantOffset);
        this.glyphInstances[1 + 12 * instances] = y;
        // targetSize
        this.glyphInstances[2 + 12 * instances] = glyph.width;
        this.glyphInstances[3 + 12 * instances] = glyph.height;
        // textColorRGBA
        this.glyphInstances[4 + 12 * instances] = 0;
        this.glyphInstances[5 + 12 * instances] = 0;
        this.glyphInstances[6 + 12 * instances] = 0;
        this.glyphInstances[7 + 12 * instances] = 1;
        // atlasOrigin
        this.glyphInstances[8 + 12 * instances] = glyph.textureU;
        this.glyphInstances[9 + 12 * instances] = glyph.textureV;
        // atlasSize
        this.glyphInstances[10 + 12 * instances] = glyph.textureWidth;
        this.glyphInstances[11 + 12 * instances] = glyph.textureHeight;

        x += glyph.subpixelWidth;
        instances++;
      }

      x = 0;
      y += Math.round(this.style.computedLineHeight * window.devicePixelRatio);
    }

    this.gl.useProgram(this.textBlendPass1Program);
    this.gl.viewport(0, 0, this.gl.canvas.width, this.gl.canvas.height);
    const viewportScaleLocation = this.gl.getUniformLocation(
      this.textBlendPass1Program,
      "viewportScale"
    );
    this.gl.uniform2f(
      viewportScaleLocation,
      2 / this.gl.canvas.width,
      -2 / this.gl.canvas.height
    );

    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.glyphInstancesBuffer);
    this.gl.bufferData(
      this.gl.ARRAY_BUFFER,
      this.glyphInstances,
      this.gl.STREAM_DRAW
    );

    this.gl.clearColor(1, 1, 1, 1);
    this.gl.clear(this.gl.COLOR_BUFFER_BIT);

    this.gl.useProgram(this.textBlendPass1Program);
    this.gl.blendFuncSeparate(
      this.gl.ZERO,
      this.gl.ONE_MINUS_SRC_COLOR,
      this.gl.ZERO,
      this.gl.ONE
    );
    this.gl.drawElementsInstanced(
      this.gl.TRIANGLES,
      6,
      this.gl.UNSIGNED_BYTE,
      0,
      instances
    );

    this.gl.useProgram(this.textBlendPass2Program);
    this.gl.blendFuncSeparate(
      this.gl.ONE,
      this.gl.ONE,
      this.gl.ZERO,
      this.gl.ONE
    );
    const viewportScaleLocation2 = this.gl.getUniformLocation(
      this.textBlendPass2Program,
      "viewportScale"
    );
    this.gl.uniform2f(
      viewportScaleLocation2,
      2 / this.gl.canvas.width,
      -2 / this.gl.canvas.height
    );
    this.gl.drawElementsInstanced(
      this.gl.TRIANGLES,
      6,
      this.gl.UNSIGNED_BYTE,
      0,
      instances
    );
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
}

class Atlas {
  constructor(gl, style) {
    this.textureSize = 512;
    this.uvScale = 1 / this.textureSize;
    this.style = style;
    this.glyphPadding = 2;
    this.nextX = 0;
    this.nextY = 0;
    this.glyphs = new Map();

    this.gl = gl;
    this.glyphCanvas = document.createElement("canvas");
    this.glyphCanvas.width = this.textureSize;
    this.glyphCanvas.height = this.textureSize;
    this.glyphCtx = this.glyphCanvas.getContext("2d", { alpha: false });
    this.glyphCtx.fillStyle = "white";
    this.glyphCtx.fillRect(
      0,
      0,
      this.glyphCanvas.width,
      this.glyphCanvas.height
    );
    this.glyphCtx.font = `${this.style.fontSize}px ${this.style.fontFamily}`;
    this.glyphCtx.fillStyle = "black";
    this.glyphCtx.textBaseline = "bottom";
    this.glyphCtx.scale(style.dpiScale, style.dpiScale);

    this.texture = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA,
      this.textureSize,
      this.textureSize,
      0,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      this.glyphCanvas
    );
    // document.body.appendChild(this.glyphCanvas)
    // this.glyphCanvas.style.position = 'absolute'
    // this.glyphCanvas.style.top = 0
    // this.glyphCanvas.style.right = 0
  }

  getGlyph(text, variantIndex) {
    let glyphVariants = this.glyphs.get(text);
    if (!glyphVariants) {
      glyphVariants = new Map();
      this.glyphs.set(text, glyphVariants);
    }

    let glyph = glyphVariants.get(variantIndex);
    if (!glyph) {
      glyph = this.rasterizeGlyph(text, variantIndex);
      glyphVariants.set(variantIndex, glyph);
    }

    return glyph;
  }

  rasterizeGlyph(text, variantIndex) {
    const { dpiScale, computedLineHeight } = this.style;
    const variantOffset = variantIndex / SUBPIXEL_DIVISOR;

    const height = computedLineHeight;
    const { width: subpixelWidth } = this.glyphCtx.measureText(text);
    const width = Math.ceil(variantOffset) + Math.ceil(subpixelWidth);

    if ((this.nextX + width) * dpiScale > this.textureSize) {
      this.nextX = 0;
      this.nextY = Math.ceil(this.nextY + height + this.glyphPadding);
    }

    if ((this.nextY + height) * dpiScale > this.textureSize) {
      throw new Error("Texture is too small");
    }

    const x = this.nextX;
    const y = this.nextY;
    this.glyphCtx.fillText(text, x + variantOffset, y + height);
    this.gl.texSubImage2D(
      this.gl.TEXTURE_2D,
      0,
      x,
      y,
      width,
      height,
      this.gl.RGBA,
      this.gl.UNSIGNED_BYTE,
      this.glyphCtx.getImageData(x, y, width, height)
    );

    this.nextX += width;

    return {
      textureU: x * dpiScale * this.uvScale,
      textureV: y * dpiScale * this.uvScale,
      textureWidth: width * dpiScale * this.uvScale,
      textureHeight: height * dpiScale * this.uvScale,
      width: width * dpiScale,
      height: height * dpiScale,
      subpixelWidth: subpixelWidth * dpiScale,
      variantOffset
    };
  }
}
