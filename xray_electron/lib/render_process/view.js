const propTypes = require("prop-types");
const React = require("react");
const $ = React.createElement;
const ViewRegistry = require("./view_registry");

class View extends React.Component {
  constructor(props) {
    super(props);
    this.state = {
      version: 0,
      viewId: props.id
    };
  }

  componentWillReceiveProps(props, context) {
    const { viewRegistry } = context;

    if (this.state.viewId !== props.id) {
      this.setState({ viewId: props.id });
      this.watch(props, context);
    }
  }

  componentDidMount() {
    this.watch(this.props, this.context);
  }

  componentWillUnmount() {
    if (this.disposePropsWatch) this.disposePropsWatch();
    if (this.disposeFocusWatch) this.disposeFocusWatch();
  }

  render() {
    const { viewRegistry } = this.context;
    const { id } = this.props;
    const component = viewRegistry.getComponent(id);
    const props = viewRegistry.getProps(id);
    const dispatch = action => viewRegistry.dispatchAction(id, action);
    return $(
      component,
      Object.assign({}, props, {
        ref: component => (this.component = component),
        dispatch,
        key: id
      })
    );
  }

  watch(props, context) {
    if (this.disposePropsWatch) this.disposePropsWatch();
    if (this.disposeFocusWatch) this.disposeFocusWatch();
    this.disposePropsWatch = context.viewRegistry.watchProps(props.id, () => {
      this.setState({ version: this.state.version + 1 });
    });
    this.disposeFocusWatch = context.viewRegistry.watchFocus(props.id, () => {
      if (this.component.focus) this.component.focus();
    });
  }
}

View.contextTypes = {
  viewRegistry: propTypes.instanceOf(ViewRegistry)
};

module.exports = View;
