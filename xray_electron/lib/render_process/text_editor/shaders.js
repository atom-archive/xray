exports.textBlendVertex = `
  attribute vec2 unitQuadCorner;
  attribute vec2 targetOrigin;
  attribute vec2 targetSize;
  attribute vec4 textColorRGBA;
  attribute vec2 atlasOrigin;
  attribute vec2 atlasSize;

  uniform vec2 posScale;

  varying vec4 textColor;
  varying vec2 atlasPosition;

  void main() {
      vec2 targetPixelPosition = targetOrigin + unitQuadCorner * targetSize;
      // vec2 pos = pixelPos * posScale + vec2(-1.0, 1.0);
      gl_Position = vec4(targetPixelPosition, 0.0, 1.0);
      textColor = textColorRGBA * vec4(1.0 / 255.0);
      // Conversion to sRGB.
      textColor = textColor * textColor;
      atlasPosition = atlasOrigin + targetPixelPosition * atlasSize;
  }
`

exports.textBlendPass1Fragment = `
  precision mediump float;

  varying vec4 textColor;
  varying vec2 atlasPosition;

  uniform sampler2D atlasTexture;

  void main() {
    vec3 atlasColor = texture2D(atlasTexture, atlasPosition).rgb;
    vec3 textColorRGB = textColor.rgb;
    vec3 correctedAtlasColor = mix(vec3(1.0) - atlasColor, sqrt(vec3(1.0) - atlasColor * atlasColor), textColorRGB);
    gl_FragColor = vec4(correctedAtlasColor, 1.0);
  }
`

exports.textBlendPass2Fragment = `
  precision mediump float;

  varying vec4 textColor;
  varying vec2 atlasPosition;

  uniform sampler2D atlasTexture;

  void main() {
    vec3 atlasColor = texture2D(atlasTexture, atlasPosition).rgb;
    vec3 textColorRGB = textColor.rgb;
    vec3 correctedAtlasColor = mix(vec3(1.0) - atlasColor, sqrt(vec3(1.0) - atlasColor * atlasColor), textColorRGB);
    vec3 adjustedForegroundColor = textColorRGB * correctedAtlasColor;
    gl_FragColor = vec4(adjustedForegroundColor, 1.0);
  }
`
