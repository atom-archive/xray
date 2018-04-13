use futures::{Poll, Stream};
use std::rc::Rc;
use std::cell::RefCell;
use std::cmp::{self, Ordering};
use std::mem;
use std::ops::Range;
use serde_json;
use notify_cell::NotifyCell;
use buffer::{Anchor, Buffer, Point};
use movement;
use window::{View, WeakViewHandle, Window};

pub struct BufferView {
    buffer: Rc<RefCell<Buffer>>,
    updates: NotifyCell<()>,
    dropped: NotifyCell<bool>,
    selections: Vec<Selection>,
    height: Option<f64>,
    width: Option<f64>,
    line_height: f64,
    scroll_top: f64,
}

struct Selection {
    start: Anchor,
    end: Anchor,
    reversed: bool,
    goal_column: Option<u32>,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct SelectionProps {
    pub start: Point,
    pub end: Point,
    pub reversed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum BufferViewAction {
    UpdateScrollTop { delta: f64 },
    SetDimensions { width: u64, height: u64 },
    Edit { text: String },
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
}

impl BufferView {
    pub fn new(buffer: Rc<RefCell<Buffer>>) -> Self {
        let selections;

        {
            let buffer = buffer.borrow();
            selections = vec![
                Selection {
                    start: buffer.anchor_before_offset(0).unwrap(),
                    end: buffer.anchor_before_offset(0).unwrap(),
                    reversed: false,
                    goal_column: None,
                },
            ];
        }

        Self {
            updates: NotifyCell::new(()),
            buffer,
            selections,
            dropped: NotifyCell::new(false),
            height: None,
            width: None,
            line_height: 10.0,
            scroll_top: 0.0,
        }
    }

    pub fn set_height(&mut self, height: f64) -> &mut Self {
        self.height = Some(height);
        self.updated();
        self
    }

    pub fn set_width(&mut self, width: f64) -> &mut Self {
        self.width = Some(width);
        self.updated();
        self
    }

    pub fn set_line_height(&mut self, line_height: f64) -> &mut Self {
        self.line_height = line_height;
        self.updated();
        self
    }

    pub fn set_scroll_top(&mut self, scroll_top: f64) -> &mut Self {
        self.scroll_top = scroll_top;
        self.updated();
        self
    }

    pub fn edit(&mut self, text: &str) {
        {
            let mut buffer = self.buffer.borrow_mut();
            let mut offset_ranges = Vec::new();

            for selection in &self.selections {
                let start = buffer.offset_for_anchor(&selection.start).unwrap();
                let end = buffer.offset_for_anchor(&selection.end).unwrap();
                offset_ranges.push((start, end));
            }

            for &(start, end) in offset_ranges.iter().rev() {
                buffer.splice(start..end, text);
            }

            let mut delta = 0_isize;
            self.selections = offset_ranges
                .into_iter()
                .map(|(start, end)| {
                    let start = start as isize;
                    let end = end as isize;
                    let anchor = buffer
                        .anchor_before_offset((start + delta) as usize + text.len())
                        .unwrap();
                    let deleted_count = end - start;
                    delta += text.len() as isize - deleted_count;
                    Selection {
                        start: anchor.clone(),
                        end: anchor,
                        reversed: false,
                        goal_column: None,
                    }
                })
                .collect();
        }

        self.updated();
    }

    pub fn add_selection(&mut self, start: Point, end: Point) {
        debug_assert!(start <= end); // TODO: Reverse selection if end < start

        {
            let buffer = self.buffer.borrow();

            // TODO: Clip points or return a result.
            let start_anchor = buffer.anchor_before_point(start).unwrap();
            let end_anchor = buffer.anchor_before_point(end).unwrap();
            let index = match self.selections
                .binary_search_by(|probe| buffer.cmp_anchors(&probe.start, &start_anchor).unwrap())
            {
                Ok(index) => index,
                Err(index) => index,
            };
            self.selections.insert(
                index,
                Selection {
                    start: start_anchor,
                    end: end_anchor,
                    reversed: false,
                    goal_column: None,
                },
            );
        }

        self.merge_selections();
        self.updated();
    }

    pub fn add_selection_above(&mut self) {
        {
            let buffer = self.buffer.borrow();

            let mut new_selections = Vec::new();
            for selection in &self.selections {
                let selection_start = buffer.point_for_anchor(&selection.start).unwrap();
                let selection_end = buffer.point_for_anchor(&selection.end).unwrap();
                if selection_start.row != selection_end.row {
                    continue;
                }

                let goal_column = selection.goal_column.unwrap_or(selection_end.column);
                let mut row = selection_start.row;
                while row > 0 {
                    row -= 1;
                    let max_column = buffer.len_for_row(row).unwrap();

                    let start_column;
                    let end_column;
                    let add_selection;
                    if selection_start == selection_end {
                        start_column = cmp::min(goal_column, max_column);
                        end_column = cmp::min(goal_column, max_column);
                        add_selection = selection_end.column == 0 || end_column > 0;
                    } else {
                        start_column = cmp::min(selection_start.column, max_column);
                        end_column = cmp::min(goal_column, max_column);
                        add_selection = start_column != end_column;
                    }

                    if add_selection {
                        new_selections.push(Selection {
                            start: buffer
                                .anchor_before_point(Point::new(row, start_column))
                                .unwrap(),
                            end: buffer
                                .anchor_before_point(Point::new(row, end_column))
                                .unwrap(),
                            reversed: selection.reversed,
                            goal_column: Some(goal_column),
                        });
                        break;
                    }
                }
            }

            for selection in new_selections {
                let index = match self.selections.binary_search_by(|probe| {
                    buffer.cmp_anchors(&probe.start, &selection.start).unwrap()
                }) {
                    Ok(index) => index,
                    Err(index) => index,
                };
                self.selections.insert(index, selection);
            }
        }

        self.merge_selections();
        self.updated();
    }

    pub fn add_selection_below(&mut self) {
        {
            let buffer = self.buffer.borrow();
            let max_row = buffer.max_point().row;

            let mut new_selections = Vec::new();
            for selection in &self.selections {
                let selection_start = buffer.point_for_anchor(&selection.start).unwrap();
                let selection_end = buffer.point_for_anchor(&selection.end).unwrap();
                if selection_start.row != selection_end.row {
                    continue;
                }

                let goal_column = selection.goal_column.unwrap_or(selection_end.column);
                let mut row = selection_start.row;
                while row < max_row {
                    row += 1;
                    let max_column = buffer.len_for_row(row).unwrap();

                    let start_column;
                    let end_column;
                    let add_selection;
                    if selection_start == selection_end {
                        start_column = cmp::min(goal_column, max_column);
                        end_column = cmp::min(goal_column, max_column);
                        add_selection = selection_end.column == 0 || end_column > 0;
                    } else {
                        start_column = cmp::min(selection_start.column, max_column);
                        end_column = cmp::min(goal_column, max_column);
                        add_selection = start_column != end_column;
                    }

                    if add_selection {
                        new_selections.push(Selection {
                            start: buffer
                                .anchor_before_point(Point::new(row, start_column))
                                .unwrap(),
                            end: buffer
                                .anchor_before_point(Point::new(row, end_column))
                                .unwrap(),
                            reversed: selection.reversed,
                            goal_column: Some(goal_column),
                        });
                        break;
                    }
                }
            }

            for selection in new_selections {
                let index = match self.selections.binary_search_by(|probe| {
                    buffer.cmp_anchors(&probe.start, &selection.start).unwrap()
                }) {
                    Ok(index) => index,
                    Err(index) => index,
                };
                self.selections.insert(index, selection);
            }
        }

        self.merge_selections();
        self.updated();
    }

    pub fn move_left(&mut self) {
        {
            let buffer = self.buffer.borrow();
            for selection in &mut self.selections {
                let start = buffer.point_for_anchor(&selection.start).unwrap();
                let end = buffer.point_for_anchor(&selection.end).unwrap();

                if start != end {
                    selection.end = selection.start.clone();
                } else {
                    let cursor = buffer
                        .anchor_before_point(movement::left(&buffer, start))
                        .unwrap();
                    selection.start = cursor.clone();
                    selection.end = cursor;
                }
                selection.reversed = false;
                selection.goal_column = None;
            }
        }
        self.merge_selections();
        self.updated();
    }

    pub fn select_left(&mut self) {
        {
            let buffer = self.buffer.borrow();
            for selection in &mut self.selections {
                let head = buffer.point_for_anchor(selection.head()).unwrap();
                let cursor = buffer
                    .anchor_before_point(movement::left(&buffer, head))
                    .unwrap();
                selection.set_head(&buffer, cursor);
                selection.goal_column = None;
            }
        }
        self.merge_selections();
        self.updated();
    }

    pub fn move_right(&mut self) {
        {
            let buffer = self.buffer.borrow();
            for selection in &mut self.selections {
                let start = buffer.point_for_anchor(&selection.start).unwrap();
                let end = buffer.point_for_anchor(&selection.end).unwrap();

                if start != end {
                    selection.start = selection.end.clone();
                } else {
                    let cursor = buffer
                        .anchor_before_point(movement::right(&buffer, end))
                        .unwrap();
                    selection.start = cursor.clone();
                    selection.end = cursor;
                }
                selection.reversed = false;
                selection.goal_column = None;
            }
        }
        self.merge_selections();
        self.updated();
    }

    pub fn select_right(&mut self) {
        {
            let buffer = self.buffer.borrow();
            for selection in &mut self.selections {
                let head = buffer.point_for_anchor(selection.head()).unwrap();
                let cursor = buffer
                    .anchor_before_point(movement::right(&buffer, head))
                    .unwrap();
                selection.set_head(&buffer, cursor);
                selection.goal_column = None;
            }
        }
        self.merge_selections();
        self.updated();
    }

    pub fn move_up(&mut self) {
        {
            let buffer = self.buffer.borrow();
            for selection in &mut self.selections {
                let start = buffer.point_for_anchor(&selection.start).unwrap();
                let end = buffer.point_for_anchor(&selection.end).unwrap();
                if start != end {
                    selection.goal_column = None;
                }

                let (start, goal_column) = movement::up(&buffer, start, selection.goal_column);
                let cursor = buffer.anchor_before_point(start).unwrap();
                selection.start = cursor.clone();
                selection.end = cursor;
                selection.goal_column = goal_column;
                selection.reversed = false;
            }
        }
        self.merge_selections();
        self.updated();
    }

    pub fn select_up(&mut self) {
        {
            let buffer = self.buffer.borrow();
            for selection in &mut self.selections {
                let head = buffer.point_for_anchor(selection.head()).unwrap();
                let (head, goal_column) = movement::up(&buffer, head, selection.goal_column);
                selection.set_head(&buffer, buffer.anchor_before_point(head).unwrap());
                selection.goal_column = goal_column;
            }
        }
        self.merge_selections();
        self.updated();
    }

    pub fn move_down(&mut self) {
        {
            let buffer = self.buffer.borrow();
            for selection in &mut self.selections {
                let start = buffer.point_for_anchor(&selection.start).unwrap();
                let end = buffer.point_for_anchor(&selection.end).unwrap();
                if start != end {
                    selection.goal_column = None;
                }

                let (start, goal_column) = movement::down(&buffer, end, selection.goal_column);
                let cursor = buffer.anchor_before_point(start).unwrap();
                selection.start = cursor.clone();
                selection.end = cursor;
                selection.goal_column = goal_column;
                selection.reversed = false;
            }
        }
        self.merge_selections();
        self.updated();
    }

    pub fn select_down(&mut self) {
        {
            let buffer = self.buffer.borrow();
            for selection in &mut self.selections {
                let head = buffer.point_for_anchor(selection.head()).unwrap();
                let (head, goal_column) = movement::down(&buffer, head, selection.goal_column);
                selection.set_head(&buffer, buffer.anchor_before_point(head).unwrap());
                selection.goal_column = goal_column;
            }
        }
        self.merge_selections();
        self.updated();
    }

    fn merge_selections(&mut self) {
        let buffer = self.buffer.borrow();
        let mut i = 1;
        while i < self.selections.len() {
            if buffer
                .cmp_anchors(&self.selections[i - 1].end, &self.selections[i].start)
                .unwrap() >= Ordering::Equal
            {
                let removed = self.selections.remove(i);
                if buffer
                    .cmp_anchors(&removed.end, &self.selections[i - 1].end)
                    .unwrap() > Ordering::Equal
                {
                    self.selections[i - 1].end = removed.end;
                }
            } else {
                i += 1;
            }
        }
    }

    fn query_selections(&self, range: Range<Point>) -> &[Selection] {
        let buffer = self.buffer.borrow();

        let start = buffer.anchor_before_point(range.start).unwrap();
        let start_index = match self.selections
            .binary_search_by(|probe| buffer.cmp_anchors(&probe.start, &start).unwrap())
        {
            Ok(index) => index,
            Err(index) => {
                if index > 0
                    && buffer
                        .cmp_anchors(&self.selections[index - 1].end, &start)
                        .unwrap() == Ordering::Greater
                {
                    index - 1
                } else {
                    index
                }
            }
        };

        if range.end > buffer.max_point() {
            &self.selections[start_index..]
        } else {
            let end = buffer.anchor_after_point(range.end).unwrap();
            let end_index = match self.selections
                .binary_search_by(|probe| buffer.cmp_anchors(&probe.start, &end).unwrap())
            {
                Ok(index) => index,
                Err(index) => index,
            };

            &self.selections[start_index..end_index]
        }
    }

    fn updated(&mut self) {
        self.updates.set(());
    }
}

impl View for BufferView {
    fn component_name(&self) -> &'static str {
        "BufferView"
    }

    fn will_mount(&mut self, window: &mut Window, _: WeakViewHandle<Self>) {
        self.height = Some(window.height());
    }

    fn render(&self) -> serde_json::Value {
        let buffer = self.buffer.borrow();

        let max_scroll_top = buffer.max_point().row as f64 * self.line_height;
        let scroll_top = self.scroll_top.min(max_scroll_top);
        let scroll_bottom = scroll_top + self.height.unwrap_or(0.0);
        let start = Point::new((scroll_top / self.line_height).floor() as u32, 0);
        let end = Point::new((scroll_bottom / self.line_height).ceil() as u32, 0);

        let mut lines = Vec::new();
        let mut cur_line = Vec::new();
        let mut cur_row = start.row;
        for c in buffer.iter_starting_at_row(start.row) {
            if c == (b'\n' as u16) {
                lines.push(String::from_utf16_lossy(&cur_line));
                cur_line = Vec::new();
                cur_row += 1;
                if cur_row >= end.row {
                    break;
                }
            } else {
                cur_line.push(c);
            }
        }
        if cur_row < end.row {
            lines.push(String::from_utf16_lossy(&cur_line));
        }

        let visible_selections = self.query_selections(start..end);
        json!({
            "first_visible_row": start.row,
            "lines": lines,
            "scroll_top": self.scroll_top,
            "height": self.height,
            "width": self.width,
            "line_height": self.line_height,
            "selections": visible_selections.iter()
                .map(|selection| selection.render(&buffer))
                .collect::<Vec<_>>()
        })
    }

    fn dispatch_action(&mut self, action: serde_json::Value, _: &mut Window) {
        match serde_json::from_value(action) {
            Ok(BufferViewAction::UpdateScrollTop { delta }) => {
                let mut scroll_top = self.scroll_top + delta;
                if scroll_top < 0.0 {
                    scroll_top = 0.0;
                }
                self.set_scroll_top(scroll_top);
            }
            Ok(BufferViewAction::SetDimensions { width, height }) => {
                self.set_width(width as f64);
                self.set_height(height as f64);
            }
            Ok(BufferViewAction::Edit { text }) => self.edit(text.as_str()),
            Ok(BufferViewAction::MoveUp) => self.move_up(),
            Ok(BufferViewAction::MoveDown) => self.move_down(),
            Ok(BufferViewAction::MoveLeft) => self.move_left(),
            Ok(BufferViewAction::MoveRight) => self.move_right(),
            action @ _ => eprintln!("Unrecognized action {:?}", action),
        }
    }
}

impl Stream for BufferView {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.updates.poll()
    }
}

impl Drop for BufferView {
    fn drop(&mut self) {
        self.dropped.set(true);
    }
}

impl Selection {
    fn head(&self) -> &Anchor {
        if self.reversed {
            &self.start
        } else {
            &self.end
        }
    }

    fn set_head(&mut self, buffer: &Buffer, cursor: Anchor) {
        if buffer.cmp_anchors(&cursor, self.tail()).unwrap() < Ordering::Equal {
            if !self.reversed {
                mem::swap(&mut self.start, &mut self.end);
                self.reversed = true;
            }
            self.start = cursor;
        } else {
            if self.reversed {
                mem::swap(&mut self.start, &mut self.end);
                self.reversed = false;
            }
            self.end = cursor;
        }
    }

    fn tail(&self) -> &Anchor {
        if self.reversed {
            &self.end
        } else {
            &self.start
        }
    }

    fn render(&self, buffer: &Buffer) -> SelectionProps {
        SelectionProps {
            start: buffer.point_for_anchor(&self.start).unwrap(),
            end: buffer.point_for_anchor(&self.end).unwrap(),
            reversed: self.reversed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_movement() {
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new())));
        editor.buffer.borrow_mut().splice(0..0, "abc");
        editor.buffer.borrow_mut().splice(3..3, "\n");
        editor.buffer.borrow_mut().splice(4..4, "\ndef");
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 0)]);

        editor.move_right();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 1)]);

        // Wraps across lines moving right
        for _ in 0..3 {
            editor.move_right();
        }
        assert_eq!(render_selections(&editor), vec![empty_selection(1, 0)]);

        // Stops at end
        for _ in 0..4 {
            editor.move_right();
        }
        assert_eq!(render_selections(&editor), vec![empty_selection(2, 3)]);

        // Wraps across lines moving left
        for _ in 0..4 {
            editor.move_left();
        }
        assert_eq!(render_selections(&editor), vec![empty_selection(1, 0)]);

        // Stops at start
        for _ in 0..4 {
            editor.move_left();
        }
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 0)]);

        // Moves down and up at column 0
        editor.move_down();
        assert_eq!(render_selections(&editor), vec![empty_selection(1, 0)]);
        editor.move_up();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 0)]);

        // Maintains a goal column when moving down
        // This means we'll jump to the column we started with even after crossing a shorter line
        editor.move_right();
        editor.move_right();
        editor.move_down();
        assert_eq!(render_selections(&editor), vec![empty_selection(1, 0)]);
        editor.move_down();
        assert_eq!(render_selections(&editor), vec![empty_selection(2, 2)]);

        // Jumps to end when moving down on the last line.
        editor.move_down();
        assert_eq!(render_selections(&editor), vec![empty_selection(2, 3)]);

        // Stops at end
        editor.move_down();
        assert_eq!(render_selections(&editor), vec![empty_selection(2, 3)]);

        // Resets the goal column when moving horizontally
        editor.move_left();
        editor.move_left();
        editor.move_up();
        assert_eq!(render_selections(&editor), vec![empty_selection(1, 0)]);
        editor.move_up();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 1)]);

        // Jumps to start when moving up on the first line
        editor.move_up();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 0)]);

        // Preserves goal column after jumping to start/end
        editor.move_down();
        editor.move_down();
        assert_eq!(render_selections(&editor), vec![empty_selection(2, 1)]);
        editor.move_down();
        assert_eq!(render_selections(&editor), vec![empty_selection(2, 3)]);
        editor.move_up();
        editor.move_up();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 1)]);
    }

    #[test]
    fn test_selection_movement() {
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new())));
        editor.buffer.borrow_mut().splice(0..0, "abc");
        editor.buffer.borrow_mut().splice(3..3, "\n");
        editor.buffer.borrow_mut().splice(4..4, "\ndef");

        assert_eq!(render_selections(&editor), vec![empty_selection(0, 0)]);

        editor.select_right();
        assert_eq!(render_selections(&editor), vec![selection((0, 0), (0, 1))]);

        // Selecting right wraps across newlines
        for _ in 0..3 {
            editor.select_right();
        }
        assert_eq!(render_selections(&editor), vec![selection((0, 0), (1, 0))]);

        // Moving right with a non-empty selection clears the selection
        editor.move_right();
        assert_eq!(render_selections(&editor), vec![empty_selection(1, 0)]);
        editor.move_right();
        assert_eq!(render_selections(&editor), vec![empty_selection(2, 0)]);

        // Selecting left wraps across newlines
        editor.select_left();
        assert_eq!(
            render_selections(&editor),
            vec![rev_selection((1, 0), (2, 0))]
        );
        editor.select_left();
        assert_eq!(
            render_selections(&editor),
            vec![rev_selection((0, 3), (2, 0))]
        );

        // Moving left with a non-empty selection clears the selection
        editor.move_left();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 3)]);

        // Reverse is updated correctly when selecting left and right
        editor.select_left();
        assert_eq!(
            render_selections(&editor),
            vec![rev_selection((0, 2), (0, 3))]
        );
        editor.select_right();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 3)]);
        editor.select_right();
        assert_eq!(render_selections(&editor), vec![selection((0, 3), (1, 0))]);
        editor.select_left();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 3)]);
        editor.select_left();
        assert_eq!(
            render_selections(&editor),
            vec![rev_selection((0, 2), (0, 3))]
        );

        // Selecting vertically moves the head and updates the reversed property
        editor.select_left();
        assert_eq!(
            render_selections(&editor),
            vec![rev_selection((0, 1), (0, 3))]
        );
        editor.select_down();
        assert_eq!(render_selections(&editor), vec![selection((0, 3), (1, 0))]);
        editor.select_down();
        assert_eq!(render_selections(&editor), vec![selection((0, 3), (2, 1))]);
        editor.select_up();
        editor.select_up();
        assert_eq!(
            render_selections(&editor),
            vec![rev_selection((0, 1), (0, 3))]
        );

        // Favors selection end when moving down
        editor.move_down();
        editor.move_down();
        assert_eq!(render_selections(&editor), vec![empty_selection(2, 3)]);

        // Favors selection start when moving up
        editor.move_left();
        editor.move_left();
        editor.select_right();
        editor.select_right();
        assert_eq!(render_selections(&editor), vec![selection((2, 1), (2, 3))]);
        editor.move_up();
        editor.move_up();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 1)]);
    }

    #[test]
    fn test_add_selection() {
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new())));
        editor
            .buffer
            .borrow_mut()
            .splice(0..0, "abcd\nefgh\nijkl\nmnop");
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 0)]);

        // Adding non-overlapping selections
        editor.move_right();
        editor.move_right();
        editor.add_selection(Point::new(0, 0), Point::new(0, 1));
        editor.add_selection(Point::new(2, 2), Point::new(2, 3));
        editor.add_selection(Point::new(0, 3), Point::new(1, 2));
        assert_eq!(
            render_selections(&editor),
            vec![
                selection((0, 0), (0, 1)),
                selection((0, 2), (0, 2)),
                selection((0, 3), (1, 2)),
                selection((2, 2), (2, 3)),
            ]
        );

        // Adding a selection that starts at the start of an existing selection
        editor.add_selection(Point::new(0, 3), Point::new(1, 0));
        editor.add_selection(Point::new(0, 3), Point::new(1, 3));
        editor.add_selection(Point::new(0, 3), Point::new(1, 2));

        assert_eq!(
            render_selections(&editor),
            vec![
                selection((0, 0), (0, 1)),
                selection((0, 2), (0, 2)),
                selection((0, 3), (1, 3)),
                selection((2, 2), (2, 3)),
            ]
        );

        // Adding a selection that starts or ends inside an existing selection
        editor.add_selection(Point::new(0, 1), Point::new(0, 2));
        editor.add_selection(Point::new(1, 2), Point::new(1, 4));
        editor.add_selection(Point::new(2, 1), Point::new(2, 2));
        assert_eq!(
            render_selections(&editor),
            vec![
                selection((0, 0), (0, 2)),
                selection((0, 3), (1, 4)),
                selection((2, 1), (2, 3)),
            ]
        );
    }

    #[test]
    fn test_add_selection_above() {
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new())));
        editor.buffer.borrow_mut().splice(
            0..0,
            "\
             abcdefghijk\n\
             lmnop\n\
             \n\
             \n\
             qrstuvwxyz\n\
             ",
        );

        // Multi-line selections
        editor.move_down();
        editor.move_right();
        editor.move_right();
        editor.select_down();
        editor.select_down();
        editor.select_down();
        editor.select_right();
        editor.select_right();
        editor.add_selection_above();
        assert_eq!(render_selections(&editor), vec![selection((1, 2), (4, 4))]);

        // Single-line selections
        editor.move_up();
        editor.move_left();
        editor.move_left();
        editor.add_selection(Point::new(2, 0), Point::new(2, 0));
        editor.add_selection(Point::new(4, 1), Point::new(4, 3));
        editor.add_selection(Point::new(4, 6), Point::new(4, 6));
        editor.add_selection(Point::new(4, 7), Point::new(4, 9));
        editor.add_selection_above();
        assert_eq!(
            render_selections(&editor),
            vec![
                selection((0, 0), (0, 0)),
                selection((0, 7), (0, 9)),
                selection((1, 0), (1, 0)),
                selection((1, 1), (1, 3)),
                selection((1, 5), (1, 5)),
                selection((2, 0), (2, 0)),
                selection((4, 1), (4, 3)),
                selection((4, 6), (4, 6)),
                selection((4, 7), (4, 9)),
            ]
        );

        editor.add_selection_above();
        assert_eq!(
            render_selections(&editor),
            vec![
                selection((0, 0), (0, 0)),
                selection((0, 1), (0, 3)),
                selection((0, 6), (0, 6)),
                selection((0, 7), (0, 9)),
                selection((1, 0), (1, 0)),
                selection((1, 1), (1, 3)),
                selection((1, 5), (1, 5)),
                selection((2, 0), (2, 0)),
                selection((4, 1), (4, 3)),
                selection((4, 6), (4, 6)),
                selection((4, 7), (4, 9)),
            ]
        );
    }

    #[test]
    fn test_add_selection_below() {
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new())));
        editor.buffer.borrow_mut().splice(
            0..0,
            "\
             abcdefgh\n\
             ijklm\n\
             \n\
             \n\
             nopqrstuvwx\n\
             yz\
             ",
        );

        // Multi-line selections
        editor.select_down();
        editor.select_down();
        editor.select_down();
        editor.select_down();
        editor.select_right();
        editor.add_selection_below();
        assert_eq!(render_selections(&editor), vec![selection((0, 0), (4, 1))]);

        // Single-line selections
        editor.move_left();
        editor.add_selection(Point::new(0, 1), Point::new(0, 1));
        editor.add_selection(Point::new(0, 4), Point::new(0, 8));
        editor.add_selection(Point::new(4, 5), Point::new(4, 6));
        editor.add_selection_below();
        assert_eq!(
            render_selections(&editor),
            vec![
                selection((0, 0), (0, 0)),
                selection((0, 1), (0, 1)),
                selection((0, 4), (0, 8)),
                selection((1, 0), (1, 0)),
                selection((1, 1), (1, 1)),
                selection((1, 4), (1, 5)),
                selection((4, 5), (4, 6)),
            ]
        );

        editor.add_selection_below();
        assert_eq!(
            render_selections(&editor),
            vec![
                selection((0, 0), (0, 0)),
                selection((0, 1), (0, 1)),
                selection((0, 4), (0, 8)),
                selection((1, 0), (1, 0)),
                selection((1, 1), (1, 1)),
                selection((1, 4), (1, 5)),
                selection((2, 0), (2, 0)),
                selection((4, 1), (4, 1)),
                selection((4, 4), (4, 8)),
            ]
        );
    }

    #[test]
    fn test_edit() {
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new())));

        editor
            .buffer
            .borrow_mut()
            .splice(0..0, "abcdefgh\nhijklmno");

        // Three selections on the same line
        editor.select_right();
        editor.select_right();
        editor.add_selection(Point::new(0, 3), Point::new(0, 5));
        editor.add_selection(Point::new(0, 7), Point::new(1, 1));
        editor.edit("-");
        assert_eq!(editor.buffer.borrow().to_string(), "-c-fg-ijklmno");
        assert_eq!(
            render_selections(&editor),
            vec![
                selection((0, 1), (0, 1)),
                selection((0, 3), (0, 3)),
                selection((0, 6), (0, 6)),
            ]
        );
    }

    #[test]
    fn test_render() {
        let buffer = Rc::new(RefCell::new(Buffer::new()));
        buffer
            .borrow_mut()
            .splice(0..0, "abc\ndef\nghi\njkl\nmno\npqr\nstu\nvwx\nyz");
        let line_height = 6.0;

        {
            let mut editor = BufferView::new(buffer.clone());
            editor
                .set_height(3.0 * line_height)
                .set_line_height(line_height)
                .set_scroll_top(2.5 * line_height);
            // Selections starting or ending outside viewport
            editor.add_selection(Point::new(1, 2), Point::new(3, 1));
            editor.add_selection(Point::new(5, 2), Point::new(6, 0));
            // Selection fully inside viewport
            editor.add_selection(Point::new(3, 2), Point::new(4, 1));
            // Selection fully outside viewport
            editor.add_selection(Point::new(6, 3), Point::new(7, 2));

            let frame = editor.render();
            assert_eq!(frame["first_visible_row"], 2);
            assert_eq!(
                stringify_lines(&frame["lines"]),
                vec!["ghi", "jkl", "mno", "pqr"]
            );
            assert_eq!(
                frame["selections"],
                json!([
                    selection((1, 2), (3, 1)),
                    selection((3, 2), (4, 1)),
                    selection((5, 2), (6, 0))
                ])
            );
        }

        // Selection starting at the end of buffer
        {
            let mut editor = BufferView::new(buffer.clone());
            editor
                .set_height(8.0 * line_height)
                .set_line_height(line_height)
                .set_scroll_top(1.0 * line_height);
            editor.add_selection(Point::new(8, 2), Point::new(8, 2));

            let frame = editor.render();
            assert_eq!(frame["first_visible_row"], 1);
            assert_eq!(
                stringify_lines(&frame["lines"]),
                vec!["def", "ghi", "jkl", "mno", "pqr", "stu", "vwx", "yz"]
            );
            assert_eq!(frame["selections"], json!([selection((8, 2), (8, 2))]));
        }

        // Selection ending exactly at first visible row
        {
            let mut editor = BufferView::new(buffer.clone());
            editor
                .set_height(3.0 * line_height)
                .set_line_height(line_height)
                .set_scroll_top(1.0 * line_height);
            editor.add_selection(Point::new(0, 2), Point::new(1, 0));

            let frame = editor.render();
            assert_eq!(frame["first_visible_row"], 1);
            assert_eq!(stringify_lines(&frame["lines"]), vec!["def", "ghi", "jkl"]);
            assert_eq!(frame["selections"], json!([]));
        }
    }

    #[test]
    fn test_render_past_last_line() {
        let line_height = 4.0;
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new())));

        editor
            .set_height(3.0 * line_height)
            .set_line_height(line_height)
            .set_scroll_top(2.0 * line_height);
        editor.buffer.borrow_mut().splice(0..0, "abc\ndef\nghi");
        editor.add_selection(Point::new(2, 3), Point::new(2, 3));

        let frame = editor.render();
        assert_eq!(frame["first_visible_row"], 2);
        assert_eq!(stringify_lines(&frame["lines"]), vec!["ghi"]);
        assert_eq!(frame["selections"], json!([selection((2, 3), (2, 3))]));

        editor.set_scroll_top(3.0 * line_height);
        let frame = editor.render();
        assert_eq!(frame["first_visible_row"], 2);
        assert_eq!(stringify_lines(&frame["lines"]), vec!["ghi"]);
        assert_eq!(frame["selections"], json!([selection((2, 3), (2, 3))]));
    }

    fn stringify_lines(lines: &serde_json::Value) -> Vec<String> {
        lines
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line.as_str().unwrap().into())
            .collect()
    }

    fn render_selections(editor: &BufferView) -> Vec<SelectionProps> {
        editor
            .selections
            .iter()
            .map(|s| s.render(&editor.buffer.borrow()))
            .collect()
    }

    fn empty_selection(row: u32, column: u32) -> SelectionProps {
        SelectionProps {
            start: Point::new(row, column),
            end: Point::new(row, column),
            reversed: false,
        }
    }

    fn selection(start: (u32, u32), end: (u32, u32)) -> SelectionProps {
        SelectionProps {
            start: Point::new(start.0, start.1),
            end: Point::new(end.0, end.1),
            reversed: false,
        }
    }

    fn rev_selection(start: (u32, u32), end: (u32, u32)) -> SelectionProps {
        SelectionProps {
            start: Point::new(start.0, start.1),
            end: Point::new(end.0, end.1),
            reversed: true,
        }
    }
}
