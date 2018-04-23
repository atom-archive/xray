const React = require("react");
const PropTypes = require("prop-types");

class ThemeProvider extends React.Component {
  render() {
    return this.props.children
  }

  getChildContext() {
    return {
      theme: this.props.theme
    };
  }
}

ThemeProvider.childContextTypes = {
  theme: PropTypes.object
};

module.exports = ThemeProvider;
