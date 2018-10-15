var path = require("path");

const baseConfig = {
  target: "node",
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
  }
};

const libConfig = {
  ...baseConfig,
  entry: "./src/index.ts",
  mode: "production",
  devtool: "source-map",
  output: {
    path: path.resolve(__dirname, "dist"),
    filename: "index.node.js",
    library: "memo",
    libraryTarget: "commonjs"
  }
};

const testConfig = {
  ...baseConfig,
  entry: "./test/tests.ts",
  mode: "development",
  devtool: "cheap-eval-source-map",
  output: {
    path: path.resolve(__dirname, "test", "dist"),
    filename: "tests.js"
  }
};

module.exports = env => {
  if (env && env.test) {
    return testConfig;
  } else {
    return libConfig;
  }
};
