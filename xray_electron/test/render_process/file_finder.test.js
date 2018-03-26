const assert = require("assert");
const {mount, setProps} = require("./helpers/component_helpers");
const FileFinder = require("../../lib/render_process/file_finder");
const $ = require("react").createElement;

suite("FileFinderView", () => {
  test("basic rendering", async () => {
    const fileFinder = mount($(FileFinder, {
      query: '',
      results: []
    }));

    assert.equal(fileFinder.find("ol li").length, 0);

    await setProps(fileFinder, {
      query: 'ce',
      results: [
        {string: 'succeed', score: 3, match_indices: [3, 4]},
        {string: 'abcdef', score: 2, match_indices: [2, 4]},
      ]
    });

    assert.deepEqual(
      fileFinder.find("ol li").map(item => item.getDOMNode().innerHTML),
      [
        'suc<b>c</b><b>e</b>ed',
        'ab<b>c</b>d<b>e</b>f'
      ]
    )
  });
});
