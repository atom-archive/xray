const assert = require("assert");
const { mount } = require("./helpers/component_helpers");
const React = require("react");
const $ = React.createElement;
const Modal = require("../../lib/render_process/modal");

suite("Modal", () => {
  let attachedElements;

  beforeEach(() => {
    attachedElements = [];
  });

  afterEach(() => {
    while ((element = attachedElements.pop())) {
      element.remove();
    }
  });

  test("closing dialog while it's focused", () => {
    const outerComponent = mount($(FocusableComponent), {
      attachTo: buildAndAttachElement("div")
    });
    outerComponent.getDOMNode().focus();
    assert.equal(document.activeElement, outerComponent.getDOMNode());

    const modal = mount($(Modal, {}, $(FocusableComponent)), {
      attachTo: buildAndAttachElement("div")
    });
    const innerComponent = modal.find(FocusableComponent);
    innerComponent.getDOMNode().focus();
    assert.equal(document.activeElement, innerComponent.getDOMNode());

    modal.unmount();
    assert.equal(document.activeElement, outerComponent.getDOMNode());
  });

  test("closing dialog when it's not focused", () => {
    const outerComponent1 = mount($(FocusableComponent, { id: 1 }), {
      attachTo: buildAndAttachElement("div")
    });
    const outerComponent2 = mount($(FocusableComponent, { id: 2 }), {
      attachTo: buildAndAttachElement("div")
    });
    outerComponent2.getDOMNode().focus();
    assert.equal(document.activeElement, outerComponent2.getDOMNode());

    const modal = mount($(Modal, {}, $(FocusableComponent)), {
      attachTo: buildAndAttachElement("div")
    });
    outerComponent1.getDOMNode().focus();
    assert.equal(document.activeElement, outerComponent1.getDOMNode());

    modal.unmount();
    assert.equal(document.activeElement, outerComponent1.getDOMNode());
  });

  class FocusableComponent extends React.Component {
    render() {
      return $("div", { id: this.props.id, tabIndex: -1 });
    }
  }

  function buildAndAttachElement(tagName) {
    const element = document.createElement(tagName);
    document.body.appendChild(element);
    attachedElements.push(element);
    return element;
  }
});
