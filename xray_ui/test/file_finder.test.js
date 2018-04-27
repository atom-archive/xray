const assert = require("assert");
const {mount, setProps} = require("./helpers/component_helpers");
const FileFinder = require("../lib/file_finder");
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
        {display_path: 'succeed', score: 3, positions: [3, 4]},
        {display_path: 'abcdef', score: 2, positions: [2, 4]},
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
