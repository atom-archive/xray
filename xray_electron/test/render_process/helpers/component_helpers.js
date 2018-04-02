const enzyme = require('enzyme');
const Adapter = require('enzyme-adapter-react-16');
const styletron = require("styletron-engine-atomic");

const styletronClient = new styletron.Client();
enzyme.configure({ adapter: new Adapter() });

module.exports = {
  shallow(node, options) {
    return enzyme.shallow(node, addStyletronOptions(options));
  },

  mount(node, options) {
    return enzyme.mount(node, addStyletronOptions(options));
  },

  setProps(wrapper, props) {
    return new Promise(resolve => wrapper.setProps(props, resolve));
  }
};

function addStyletronOptions(options = {}) {
  options.context = Object.assign({ styletron: styletronClient }, options.context);
  options.childContextTypes = Object.assign({ styletron: function() {} }, options.childContextTypes);
  return options;
}
