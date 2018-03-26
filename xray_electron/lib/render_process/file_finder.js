const React = require("react");
const { styled } = require("styletron-react");
const $ = React.createElement;

const Root = styled("div", {
  boxShadow: '0 0 8px black',
  backgroundColor: 'white',
  width: 500 + 'px',
  padding: "10px"
});

const QueryInput = styled("input", {
  width: "100%",
  boxSizing: "border-box"
});

const SearchResultList = styled("ol", {
  listStyleType: 'none',
  height: '200px',
  overflow: 'auto',
  padding: 0,
});

const SearchResultListItem = styled("li", {
  listStyleType: 'none',
  marginTop: '10px'
});

module.exports = class FileFinder extends React.Component {
  constructor() {
    super();
    this.didChangeQuery = this.didChangeQuery.bind(this);
  }

  render() {
    return $(Root, null,
      $(QueryInput, {
        $ref: (inputNode) => this.queryInput = inputNode,
        value: this.props.query,
        onChange: this.didChangeQuery
      }),
      $(SearchResultList, {}, ...this.props.results.map(this.renderSearchResult))
    );
  }

  renderSearchResult(result) {
    const path = result.string;
    const matchIndices = result.match_indices;

    let pathIndex = 0;
    let queryIndex = 0;
    const children = [];
    while (true) {
      if (pathIndex === matchIndices[queryIndex]) {
        children.push($('b', null, path[pathIndex]));
        pathIndex++;
        queryIndex++;
      } else if (queryIndex < matchIndices.length) {
        const nextPathIndex = matchIndices[queryIndex];
        children.push(path.slice(pathIndex, nextPathIndex));
        pathIndex = nextPathIndex;
      } else {
        children.push(path.slice(pathIndex));
        break;
      }
    }

    return $(SearchResultListItem, null, ...children);
  }

  componentDidMount() {
    this.queryInput.focus();
  }

  didChangeQuery(event) {
    this.props.dispatch({
      type: "UpdateQuery",
      query: event.target.value
    });
  }
};
