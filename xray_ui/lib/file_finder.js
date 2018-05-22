const React = require("react");
const ReactDOM = require("react-dom");
const { styled } = require("styletron-react");
const $ = React.createElement;
const { ActionContext, Action } = require("./action_dispatcher");

const Root = styled("div", {
  boxShadow: "0 6px 12px -2px rgba(0, 0, 0, 0.4)",
  backgroundColor: "#f2f2f2",
  borderRadius: "6px",
  width: 500 + "px",
  padding: "10px",
  marginTop: "20px"
});

const QueryInput = styled("input", {
  width: "100%",
  boxSizing: "border-box",
  padding: "5px",
  fontSize: "10pt",
  outline: "none",
  border: "1px solid #556de8",
  boxShadow: "0 0 0 1px #556de8",
  backgroundColor: "#ebeeff",
  borderRadius: "3px",
  color: "#232324"
});

const SearchResultList = styled("ol", {
  listStyleType: "none",
  height: "200px",
  overflow: "auto",
  padding: 0
});

const SearchResultListItem = styled("li", {
  listStyleType: "none",
  padding: "0.75em 1em",
  lineHeight: "2em",
  fontSize: "10pt",
  fontFamily: "sans-serif",
  borderBottom: "1px solid #dbdbdc"
});

const SearchResultMatchedQuery = styled("b", {
  color: "#304ee2",
  fontWeight: "bold"
});

class SelectedSearchResultListItem extends React.Component {
  render() {
    return $(
      styled(SearchResultListItem, {
        backgroundColor: "#dbdbdc"
      }),
      {},
      ...this.props.children
    );
  }

  componentDidMount() {
    this.scrollIntoViewIfNeeded();
  }

  componentDidUpdate() {
    this.scrollIntoViewIfNeeded();
  }

  scrollIntoViewIfNeeded() {
    const domNode = ReactDOM.findDOMNode(this);
    if (domNode) domNode.scrollIntoViewIfNeeded();
  }
}

module.exports = class FileFinder extends React.Component {
  constructor() {
    super();
    this.didChangeQuery = this.didChangeQuery.bind(this);
    this.didChangeIncludeIgnored = this.didChangeIncludeIgnored.bind(this);
  }

  render() {
    return $(
      ActionContext,
      { add: "FileFinder" },
      $(
        Root,
        null,
        $(QueryInput, {
          $ref: inputNode => (this.queryInput = inputNode),
          value: this.props.query,
          onChange: this.didChangeQuery
        }),
        $(
          SearchResultList,
          {},
          ...this.props.results.map((result, i) =>
            this.renderSearchResult(result, i === this.props.selected_index)
          )
        )
      ),
      $(Action, { type: "SelectPrevious" }),
      $(Action, { type: "SelectNext" }),
      $(Action, { type: "Confirm" }),
      $(Action, { type: "Close" })
    );
  }

  renderSearchResult({ positions, display_path }, isSelected) {
    let pathIndex = 0;
    let queryIndex = 0;
    const children = [];
    while (true) {
      if (pathIndex === positions[queryIndex]) {
        children.push(
          $(SearchResultMatchedQuery, null, display_path[pathIndex])
        );
        pathIndex++;
        queryIndex++;
      } else if (queryIndex < positions.length) {
        const nextPathIndex = positions[queryIndex];
        children.push(display_path.slice(pathIndex, nextPathIndex));
        pathIndex = nextPathIndex;
      } else {
        children.push(display_path.slice(pathIndex));
        break;
      }
    }

    const item = isSelected
      ? SelectedSearchResultListItem
      : SearchResultListItem;
    return $(item, null, ...children);
  }

  focus() {
    this.queryInput.focus();
  }

  didChangeQuery(event) {
    this.props.dispatch({
      type: "UpdateQuery",
      query: event.target.value
    });
  }

  didChangeIncludeIgnored(event) {
    this.props.dispatch({
      type: "UpdateIncludeIgnored",
      include_ignored: event.target.checked
    });
  }
};
