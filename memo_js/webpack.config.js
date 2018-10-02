var path = require("path");

const baseConfig = {
  entry: "./src/index.ts",
  module: {
    rules: [
      {
        test: /\.ts$/,
        use: "ts-loader",
        exclude: /node_modules/
      }
    ]
  },
  resolve: {
    extensions: [".ts", ".js", ".wasm"]
  },
  output: {
    path: path.resolve(__dirname, "dist"),
    library: "memo",
    libraryTarget: "umd"
  }
};

const nodeConfig = {
  ...baseConfig,
  target: "node",
  output: {
    ...baseConfig.output,
    filename: "index.node.js"
  }
};

const webConfig = {
  ...baseConfig,
  target: "web",
  output: {
    ...baseConfig.output,
    filename: "index.web.js"
  }
};

module.exports = [nodeConfig, webConfig];
