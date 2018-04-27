const enzyme = require("enzyme");
const Adapter = require("enzyme-adapter-react-16");

enzyme.configure({ adapter: new Adapter() });

module.exports = {
  shallow(node, options) {
    return enzyme.shallow(node, addStyletronToContext(options));
  },

  mount(node, options) {
    return enzyme.mount(node, addStyletronToContext(options));
  },

  setProps(wrapper, props) {
    return new Promise(resolve => wrapper.setProps(props, resolve));
  }
};

function addStyletronToContext(options = {}) {
  options.context = Object.assign(
    { styletron: { renderStyle() {} } },
    options.context
  );
  options.childContextTypes = Object.assign(
    { styletron: function() {} },
    options.childContextTypes
  );
  return options;
}
