const React = require("react");
const PropTypes = require("prop-types");
const { styled } = require("styletron-react");
const $ = React.createElement;

class TextPlane extends React.Component {
  constructor(props) {
    super(props);
    this.handleCanvas = this.handleCanvas.bind(this);
  }

  render() {
    return $("canvas", {
      ref: this.handleCanvas,
      className: this.props.className,
      width: this.props.width * window.devicePixelRatio,
      height: this.props.height * window.devicePixelRatio,
      style: {
        width: this.props.width + "px",
        height: this.props.height + "px"
      }
    });
  }

  handleCanvas(canvas) {
    this.canvas = canvas;
  }

  async componentDidUpdate() {
    if (this.canvas == null) return;

    const { userColors } = this.context.theme;
    const {
      fontFamily,
      fontSize,
      backgroundColor,
      baseTextColor
    } = this.context.theme.editor;

    const cursorColors = userColors;
    const selectionColors = cursorColors.map(color => {
      color = Object.assign({}, color);
      color.a = 0.5;
      return color;
    });

    const computedLineHeight = this.props.lineHeight;

    if (!this.gl) {
      this.gl = this.canvas.getContext("webgl2");
      this.renderer = new Renderer(this.gl, {
        fontFamily,
        fontSize,
        backgroundColor,
        baseTextColor,
        computedLineHeight,
        dpiScale: window.devicePixelRatio
      });
    }

    this.renderer.draw({
      canvasWidth: this.props.width * window.devicePixelRatio,
      canvasHeight: this.props.height * window.devicePixelRatio,
      scrollTop: this.props.scrollTop,
      scrollLeft: this.props.scrollLeft,
      paddingLeft: this.props.paddingLeft || 0,
      firstVisibleRow: this.props.firstVisibleRow,
      lines: this.props.lines,
      selections: this.props.selections,
      showLocalCursors: this.props.showLocalCursors,
      selectionColors,
      cursorColors,
      computedLineHeight
    });
  }

  measureLine(line, column) {
    return this.renderer.measureLine(line, column);
  }

  isReady() {
    return this.renderer != null;
  }
}

TextPlane.contextTypes = {
  theme: PropTypes.object
};

module.exports = TextPlane;

const shaders = require("./shaders");
const UNIT_QUAD_VERTICES = new Float32Array([1, 1, 1, 0, 0, 0, 0, 1]);
const UNIT_QUAD_ELEMENT_INDICES = new Uint8Array([0, 1, 3, 1, 2, 3]);
const MAX_INSTANCES = 1 << 16;
const GLYPH_INSTANCE_SIZE = 12;
const GLYPH_INSTANCE_SIZE_IN_BYTES =
  GLYPH_INSTANCE_SIZE * Float32Array.BYTES_PER_ELEMENT;
const SOLID_INSTANCE_SIZE = 8;
const SOLID_INSTANCE_SIZE_IN_BYTES =
  SOLID_INSTANCE_SIZE * Float32Array.BYTES_PER_ELEMENT;
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
    const solidVertexShader = this.createShader(
      shaders.solidVertex,
      this.gl.VERTEX_SHADER
    );
    const solidFragmentShader = this.createShader(
      shaders.solidFragment,
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
    this.solidProgram = this.createProgram(
      solidVertexShader,
      solidFragmentShader
    );

    this.textBlendPass1ViewportScaleLocation = this.gl.getUniformLocation(
      this.textBlendPass1Program,
      "viewportScale"
    );
    this.textBlendPass1ScrollLeftLocation = this.gl.getUniformLocation(
      this.textBlendPass1Program,
      "scrollLeft"
    );
    this.textBlendPass2ViewportScaleLocation = this.gl.getUniformLocation(
      this.textBlendPass2Program,
      "viewportScale"
    );
    this.textBlendPass2ScrollLeftLocation = this.gl.getUniformLocation(
      this.textBlendPass2Program,
      "scrollLeft"
    );
    this.solidViewportScaleLocation = this.gl.getUniformLocation(
      this.solidProgram,
      "viewportScale"
    );
    this.solidScrollLeftLocation = this.gl.getUniformLocation(
      this.solidProgram,
      "scrollLeft"
    );

    this.createBuffers();
    this.textBlendVAO = this.createTextBlendVAO();
    this.solidVAO = this.createSolidVAO();
  }

  createBuffers() {
    this.unitQuadVerticesBuffer = this.gl.createBuffer();
    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.unitQuadVerticesBuffer);
    this.gl.bufferData(
      this.gl.ARRAY_BUFFER,
      UNIT_QUAD_VERTICES,
      this.gl.STATIC_DRAW
    );

    this.unitQuadElementIndicesBuffer = this.gl.createBuffer();
    this.gl.bindBuffer(
      this.gl.ELEMENT_ARRAY_BUFFER,
      this.unitQuadElementIndicesBuffer
    );
    this.gl.bufferData(
      this.gl.ELEMENT_ARRAY_BUFFER,
      UNIT_QUAD_ELEMENT_INDICES,
      this.gl.STATIC_DRAW
    );

    this.glyphInstances = new Float32Array(MAX_INSTANCES * GLYPH_INSTANCE_SIZE);
    this.glyphInstancesBuffer = this.gl.createBuffer();

    this.selectionSolidInstances = new Float32Array(
      MAX_INSTANCES * SOLID_INSTANCE_SIZE
    );
    this.cursorSolidInstances = new Float32Array(
      MAX_INSTANCES * SOLID_INSTANCE_SIZE
    );
    this.solidInstancesBuffer = this.gl.createBuffer();
  }

  createTextBlendVAO() {
    const vao = this.gl.createVertexArray();
    this.gl.bindVertexArray(vao);

    this.gl.bindBuffer(
      this.gl.ELEMENT_ARRAY_BUFFER,
      this.unitQuadElementIndicesBuffer
    );

    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.unitQuadVerticesBuffer);
    this.gl.enableVertexAttribArray(shaders.textBlendAttributes.unitQuadVertex);
    this.gl.vertexAttribPointer(
      shaders.textBlendAttributes.unitQuadVertex,
      2,
      this.gl.FLOAT,
      false,
      0,
      0
    );

    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.glyphInstancesBuffer);

    this.gl.enableVertexAttribArray(shaders.textBlendAttributes.targetOrigin);
    this.gl.vertexAttribPointer(
      shaders.textBlendAttributes.targetOrigin,
      2,
      this.gl.FLOAT,
      false,
      GLYPH_INSTANCE_SIZE_IN_BYTES,
      0
    );
    this.gl.vertexAttribDivisor(shaders.textBlendAttributes.targetOrigin, 1);

    this.gl.enableVertexAttribArray(shaders.textBlendAttributes.targetSize);
    this.gl.vertexAttribPointer(
      shaders.textBlendAttributes.targetSize,
      2,
      this.gl.FLOAT,
      false,
      GLYPH_INSTANCE_SIZE_IN_BYTES,
      2 * Float32Array.BYTES_PER_ELEMENT
    );
    this.gl.vertexAttribDivisor(shaders.textBlendAttributes.targetSize, 1);

    this.gl.enableVertexAttribArray(shaders.textBlendAttributes.textColorRGBA);
    this.gl.vertexAttribPointer(
      shaders.textBlendAttributes.textColorRGBA,
      4,
      this.gl.FLOAT,
      false,
      GLYPH_INSTANCE_SIZE_IN_BYTES,
      4 * Float32Array.BYTES_PER_ELEMENT
    );
    this.gl.vertexAttribDivisor(shaders.textBlendAttributes.textColorRGBA, 1);

    this.gl.enableVertexAttribArray(shaders.textBlendAttributes.atlasOrigin);
    this.gl.vertexAttribPointer(
      shaders.textBlendAttributes.atlasOrigin,
      2,
      this.gl.FLOAT,
      false,
      GLYPH_INSTANCE_SIZE_IN_BYTES,
      8 * Float32Array.BYTES_PER_ELEMENT
    );
    this.gl.vertexAttribDivisor(shaders.textBlendAttributes.atlasOrigin, 1);

    this.gl.enableVertexAttribArray(shaders.textBlendAttributes.atlasSize);
    this.gl.vertexAttribPointer(
      shaders.textBlendAttributes.atlasSize,
      2,
      this.gl.FLOAT,
      false,
      GLYPH_INSTANCE_SIZE_IN_BYTES,
      10 * Float32Array.BYTES_PER_ELEMENT
    );
    this.gl.vertexAttribDivisor(shaders.textBlendAttributes.atlasSize, 1);

    return vao;
  }

  createSolidVAO() {
    const vao = this.gl.createVertexArray();
    this.gl.bindVertexArray(vao);

    this.gl.bindBuffer(
      this.gl.ELEMENT_ARRAY_BUFFER,
      this.unitQuadElementIndicesBuffer
    );

    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.unitQuadVerticesBuffer);
    this.gl.enableVertexAttribArray(shaders.solidAttributes.unitQuadVertex);
    this.gl.vertexAttribPointer(
      shaders.solidAttributes.unitQuadVertex,
      2,
      this.gl.FLOAT,
      false,
      0,
      0
    );

    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.solidInstancesBuffer);

    this.gl.enableVertexAttribArray(shaders.solidAttributes.targetOrigin);
    this.gl.vertexAttribPointer(
      shaders.solidAttributes.targetOrigin,
      2,
      this.gl.FLOAT,
      false,
      SOLID_INSTANCE_SIZE_IN_BYTES,
      0
    );
    this.gl.vertexAttribDivisor(shaders.solidAttributes.targetOrigin, 1);

    this.gl.enableVertexAttribArray(shaders.solidAttributes.targetSize);
    this.gl.vertexAttribPointer(
      shaders.solidAttributes.targetSize,
      2,
      this.gl.FLOAT,
      false,
      SOLID_INSTANCE_SIZE_IN_BYTES,
      2 * Float32Array.BYTES_PER_ELEMENT
    );
    this.gl.vertexAttribDivisor(shaders.solidAttributes.targetSize, 1);

    this.gl.enableVertexAttribArray(shaders.solidAttributes.colorRGBA);
    this.gl.vertexAttribPointer(
      shaders.solidAttributes.colorRGBA,
      4,
      this.gl.FLOAT,
      false,
      SOLID_INSTANCE_SIZE_IN_BYTES,
      4 * Float32Array.BYTES_PER_ELEMENT
    );
    this.gl.vertexAttribDivisor(shaders.solidAttributes.colorRGBA, 1);

    return vao;
  }

  measureLine(line, column = line.length) {
    let x = 0;
    for (let i = 0; i < column; i++) {
      const variantIndex = Math.round(x * SUBPIXEL_DIVISOR) % SUBPIXEL_DIVISOR;
      const glyph = this.atlas.getGlyph(line[i], variantIndex);
      x += glyph.subpixelWidth;
    }
    return x / this.style.dpiScale;
  }

  draw({
    canvasHeight,
    canvasWidth,
    scrollTop,
    scrollLeft,
    paddingLeft,
    firstVisibleRow,
    lines,
    selections,
    showLocalCursors,
    selectionColors,
    cursorColors
  }) {
    const { dpiScale } = this.style;
    const viewportScaleX = 2 / canvasWidth;
    const viewportScaleY = -2 / canvasHeight;
    scrollLeft = Math.round(scrollLeft * dpiScale);

    const textColor = { r: 0, g: 0, b: 0, a: 255 };
    const cursorWidth = 2;

    const xPositions = new Map();
    for (let i = 0; i < selections.length; i++) {
      const { start, end } = selections[i];
      xPositions.set(keyForPoint(start), 0);
      xPositions.set(keyForPoint(end), 0);
    }

    const glyphCount = this.populateGlyphInstances(
      scrollTop,
      firstVisibleRow,
      paddingLeft,
      lines,
      selections,
      textColor,
      xPositions
    );

    const {
      selectionSolidCount,
      cursorSolidCount
    } = this.populateSelectionSolidInstances(
      scrollTop,
      canvasWidth,
      paddingLeft,
      selections,
      xPositions,
      selectionColors,
      cursorColors,
      cursorWidth,
      showLocalCursors
    );
    this.atlas.uploadTexture();

    this.gl.clearColor(1, 1, 1, 1);
    this.gl.clear(this.gl.COLOR_BUFFER_BIT);
    this.gl.viewport(0, 0, canvasWidth, canvasHeight);

    this.drawSelections(
      selectionSolidCount,
      scrollLeft,
      viewportScaleX,
      viewportScaleY
    );
    this.drawText(glyphCount, scrollLeft, viewportScaleX, viewportScaleY);
    this.drawCursors(
      cursorSolidCount,
      scrollLeft,
      viewportScaleX,
      viewportScaleY
    );
  }

  drawSelections(
    selectionSolidCount,
    scrollLeft,
    viewportScaleX,
    viewportScaleY
  ) {
    this.gl.bindVertexArray(this.solidVAO);
    this.gl.enable(this.gl.BLEND);
    this.gl.useProgram(this.solidProgram);
    this.gl.uniform2f(
      this.solidViewportScaleLocation,
      viewportScaleX,
      viewportScaleY
    );
    this.gl.uniform1f(this.solidScrollLeftLocation, scrollLeft);
    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.solidInstancesBuffer);
    this.gl.bufferData(
      this.gl.ARRAY_BUFFER,
      this.selectionSolidInstances,
      this.gl.STREAM_DRAW
    );
    this.gl.blendFuncSeparate(
      this.gl.SRC_ALPHA,
      this.gl.ONE_MINUS_SRC_ALPHA,
      this.gl.ONE,
      this.gl.ONE
    );
    this.gl.drawElementsInstanced(
      this.gl.TRIANGLES,
      6,
      this.gl.UNSIGNED_BYTE,
      0,
      selectionSolidCount
    );
  }

  drawText(glyphCount, scrollLeft, viewportScaleX, viewportScaleY) {
    this.gl.bindVertexArray(this.textBlendVAO);
    this.gl.enable(this.gl.BLEND);
    this.gl.useProgram(this.textBlendPass1Program);
    this.gl.uniform2f(
      this.textBlendPass1ViewportScaleLocation,
      viewportScaleX,
      viewportScaleY
    );
    this.gl.uniform1f(this.textBlendPass1ScrollLeftLocation, scrollLeft);
    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.glyphInstancesBuffer);
    this.gl.bufferData(
      this.gl.ARRAY_BUFFER,
      this.glyphInstances,
      this.gl.STREAM_DRAW
    );
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
      glyphCount
    );

    this.gl.useProgram(this.textBlendPass2Program);
    this.gl.blendFuncSeparate(
      this.gl.ONE,
      this.gl.ONE,
      this.gl.ZERO,
      this.gl.ONE
    );
    this.gl.uniform2f(
      this.textBlendPass2ViewportScaleLocation,
      viewportScaleX,
      viewportScaleY
    );
    this.gl.uniform1f(this.textBlendPass2ScrollLeftLocation, scrollLeft);
    this.gl.drawElementsInstanced(
      this.gl.TRIANGLES,
      6,
      this.gl.UNSIGNED_BYTE,
      0,
      glyphCount
    );
  }

  drawCursors(cursorSolidCount, scrollLeft, viewportScaleX, viewportScaleY) {
    this.gl.bindVertexArray(this.solidVAO);
    this.gl.disable(this.gl.BLEND);
    this.gl.useProgram(this.solidProgram);
    this.gl.uniform2f(
      this.solidViewportScaleLocation,
      viewportScaleX,
      viewportScaleY
    );
    this.gl.uniform1f(this.solidScrollLeftLocation, scrollLeft);
    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.solidInstancesBuffer);
    this.gl.bufferData(
      this.gl.ARRAY_BUFFER,
      this.cursorSolidInstances,
      this.gl.STREAM_DRAW
    );
    this.gl.drawElementsInstanced(
      this.gl.TRIANGLES,
      6,
      this.gl.UNSIGNED_BYTE,
      0,
      cursorSolidCount
    );
  }

  populateGlyphInstances(
    scrollTop,
    firstVisibleRow,
    paddingLeft,
    lines,
    selections,
    textColor,
    xPositions
  ) {
    const firstVisibleRowY = firstVisibleRow * this.style.computedLineHeight;

    let glyphCount = 0;
    let selectionIndex = 0;
    let y = Math.round((firstVisibleRowY - scrollTop) * this.style.dpiScale);
    const position = {};

    for (var i = 0; i < lines.length; i++) {
      position.row = firstVisibleRow + i;
      let x = paddingLeft;
      const line = lines[i];

      for (
        position.column = 0;
        position.column <= line.length;
        position.column++
      ) {
        {
          const key = keyForPoint(position);
          if (xPositions.has(key)) xPositions.set(key, x);
        }

        if (position.column < line.length) {
          const char = line[position.column];
          const variantIndex =
            Math.round(x * SUBPIXEL_DIVISOR) % SUBPIXEL_DIVISOR;
          const glyph = this.atlas.getGlyph(char, variantIndex);

          this.updateGlyphInstance(
            glyphCount++,
            Math.round(x - glyph.variantOffset),
            y,
            glyph,
            textColor
          );

          x += glyph.subpixelWidth;
        }
      }

      y += Math.round(this.style.computedLineHeight * this.style.dpiScale);
    }

    if (glyphCount > MAX_INSTANCES) {
      console.error(
        `glyphCount of ${glyphCount} exceeds MAX_INSTANCES of ${MAX_INSTANCES}`
      );
    }

    return glyphCount;
  }

  updateGlyphInstance(i, x, y, glyph, color) {
    const startOffset = 12 * i;
    // targetOrigin
    this.glyphInstances[0 + startOffset] = x;
    this.glyphInstances[1 + startOffset] = y;
    // targetSize
    this.glyphInstances[2 + startOffset] = glyph.width;
    this.glyphInstances[3 + startOffset] = glyph.height;
    // textColorRGBA
    this.glyphInstances[4 + startOffset] = color.r;
    this.glyphInstances[5 + startOffset] = color.g;
    this.glyphInstances[6 + startOffset] = color.b;
    this.glyphInstances[7 + startOffset] = color.a;
    // atlasOrigin
    this.glyphInstances[8 + startOffset] = glyph.textureU;
    this.glyphInstances[9 + startOffset] = glyph.textureV;
    // atlasSize
    this.glyphInstances[10 + startOffset] = glyph.textureWidth;
    this.glyphInstances[11 + startOffset] = glyph.textureHeight;
  }

  populateSelectionSolidInstances(
    scrollTop,
    canvasWidth,
    paddingLeft,
    selections,
    xPositions,
    selectionColors,
    cursorColors,
    cursorWidth,
    showLocalCursors
  ) {
    const { dpiScale, computedLineHeight } = this.style;

    let selectionSolidCount = 0;
    let cursorSolidCount = 0;

    for (var i = 0; i < selections.length; i++) {
      const selection = selections[i];
      const colorIndex = selection.user_id % selectionColors.length;
      const selectionColor = selectionColors[colorIndex];
      const cursorColor = cursorColors[colorIndex];

      if (comparePoints(selection.start, selection.end) !== 0) {
        const rowSpan = selection.end.row - selection.start.row;
        const startX = xPositions.get(keyForPoint(selection.start));
        const endX = xPositions.get(keyForPoint(selection.end));

        if (rowSpan === 0) {
          this.updateSolidInstance(
            this.selectionSolidInstances,
            selectionSolidCount++,
            Math.round(startX),
            yForRow(selection.start.row),
            Math.round(endX - startX),
            yForRow(selection.start.row + 1) - yForRow(selection.start.row),
            selectionColor
          );
        } else {
          // First line of selection
          this.updateSolidInstance(
            this.selectionSolidInstances,
            selectionSolidCount++,
            Math.round(startX),
            yForRow(selection.start.row),
            Math.round(canvasWidth - startX),
            yForRow(selection.start.row + 1) - yForRow(selection.start.row),
            selectionColor
          );

          // Lines entirely spanned by selection
          if (rowSpan > 1) {
            this.updateSolidInstance(
              this.selectionSolidInstances,
              selectionSolidCount++,
              paddingLeft,
              yForRow(selection.start.row + 1),
              Math.round(canvasWidth),
              yForRow(selection.end.row) - yForRow(selection.start.row + 1),
              selectionColor
            );
          }

          // Last line of selection
          this.updateSolidInstance(
            this.selectionSolidInstances,
            selectionSolidCount++,
            paddingLeft,
            yForRow(selection.end.row),
            Math.round(endX - paddingLeft),
            yForRow(selection.end.row + 1) - yForRow(selection.end.row),
            selectionColor
          );
        }
      }

      if (showLocalCursors || selection.remote) {
        const cursorPoint = selection.reversed
          ? selection.start
          : selection.end;
        const startX = xPositions.get(keyForPoint(cursorPoint));
        const endX = startX + cursorWidth;
        this.updateSolidInstance(
          this.cursorSolidInstances,
          cursorSolidCount++,
          Math.round(startX),
          yForRow(cursorPoint.row),
          Math.round(endX - startX),
          yForRow(cursorPoint.row + 1) - yForRow(cursorPoint.row),
          cursorColor
        );
      }
    }

    function yForRow(row) {
      return Math.round((row * computedLineHeight - scrollTop) * dpiScale);
    }

    if (selectionSolidCount > MAX_INSTANCES) {
      console.error(
        `selectionSolidCount of ${selectionSolidCount} exceeds MAX_INSTANCES of ${MAX_INSTANCES}`
      );
    }
    if (cursorSolidCount > MAX_INSTANCES) {
      console.error(
        `cursorSolidCount of ${cursorSolidCount} exceeds MAX_INSTANCES of ${MAX_INSTANCES}`
      );
    }

    return { selectionSolidCount, cursorSolidCount };
  }

  updateSolidInstance(arrayBuffer, i, x, y, width, height, color) {
    const startOffset = 8 * i;
    // targetOrigin
    arrayBuffer[0 + startOffset] = x;
    arrayBuffer[1 + startOffset] = y;
    // targetSize
    arrayBuffer[2 + startOffset] = width;
    arrayBuffer[3 + startOffset] = height;
    // colorRGBA
    arrayBuffer[4 + startOffset] = color.r;
    arrayBuffer[5 + startOffset] = color.g;
    arrayBuffer[6 + startOffset] = color.b;
    arrayBuffer[7 + startOffset] = color.a;
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
    this.textureSize = 512 * style.dpiScale;
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

    this.shouldUploadTexture = false;
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
    this.shouldUploadTexture = true;

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

  uploadTexture() {
    if (this.shouldUploadTexture) {
      this.gl.texImage2D(
        this.gl.TEXTURE_2D,
        0,
        this.gl.RGBA,
        this.textureSize,
        this.textureSize,
        0,
        this.gl.RGBA,
        this.gl.UNSIGNED_BYTE,
        this.glyphCanvas
      );
      this.shouldUploadTexture = false;
    }
  }
}

function comparePoints(a, b) {
  return a.row - b.row || a.column - b.column;
}

function keyForPoint(point) {
  return `${point.row}.${point.column}`;
}
