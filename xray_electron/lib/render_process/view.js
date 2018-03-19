const propTypes = require("prop-types");
const React = require("react");
const $ = React.createElement;
const ViewRegistry = require("./view_registry");

class View extends React.Component {
  constructor(props, context) {
    super(props);
    this.state = {
      version: 0,
      viewId: props.id,
      disposePropsWatch: context.viewRegistry.watchProps(props.id, () => {
        this.setState({ version: this.state.version++ });
      })
    };
  }

  componentWillReceiveProps(props, context) {
    const { viewRegistry } = context;

    if (this.state.viewId !== props.id) {
      this.state.disposePropsWatch();
      this.setState({
        viewId: props.id,
        disposePropsWatch: viewRegistry.watchProps(props.id, () => {
          this.setState({ version: this.state.version + 1 });
        })
      });
    }
  }

  componentWillUnmount() {
    if (this.state.disposePropsWatch) this.state.disposePropsWatch();
  }

  render() {
    const { viewRegistry } = this.context;
    const { id } = this.props;
    const component = viewRegistry.getComponent(id);
    const props = viewRegistry.getProps(id);
    const dispatch = action => viewRegistry.dispatchAction(id, action);
    return $(component, Object.assign({ dispatch }, props));
  }
}

View.contextTypes = {
  viewRegistry: propTypes.instanceOf(ViewRegistry)
};

module.exports = View;
