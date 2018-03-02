use std::rc::Rc;
use std::cell::RefCell;
use std::cmp::{self, Ordering};
use std::mem;
use futures::future::Executor;
use futures::{Future, Stream};
use notify_cell::NotifyCell;
use buffer::{Buffer, Version, Point, Anchor};
use movement;

pub struct Editor {
    buffer: Rc<RefCell<Buffer>>,
    pub version: Rc<NotifyCell<Version>>,
    dropped: NotifyCell<bool>,
    selections: Vec<Selection>
}

struct Selection {
    start: Anchor,
    end: Anchor,
    reversed: bool,
    goal_column: Option<u32>
}

pub mod render {
    use super::Point;

    pub struct Params {
        pub scroll_top: f64,
        pub height: f64,
        pub line_height: f64
    }

    pub struct Frame {
        pub first_visible_row: u32,
        pub lines: Vec<Vec<u16>>,
        pub selections: Vec<Selection>
    }

    #[derive(Debug, Eq, PartialEq)]
    pub struct Selection {
        pub start: Point,
        pub end: Point,
        pub reversed: bool
    }
}

impl Editor {
    pub fn new(buffer: Rc<RefCell<Buffer>>) -> Self {
        let version;
        let selections;

        {
            let buffer = buffer.borrow();
            version = buffer.version.get().unwrap();
            selections = vec![Selection {
                start: buffer.anchor_before_offset(0).unwrap(),
                end: buffer.anchor_before_offset(0).unwrap(),
                reversed: false,
                goal_column: None
            }];
        }

        Self {
            version: Rc::new(NotifyCell::new(version)),
            buffer,
            selections,
            dropped: NotifyCell::new(false),
        }
    }

    pub fn run<E>(&self, executor: &E)
    where
        E: Executor<Box<Future<Item = (), Error = ()>>>,
    {
        let version_cell = self.version.clone();
        let buffer_observation = self.buffer.borrow().version.observe().for_each(
            move |buffer_version| {
                version_cell.set(buffer_version);
                Ok(())
            },
        );
        let drop_observation = self.dropped.observe().into_future();
        executor.execute(Box::new(
            buffer_observation
                .select2(drop_observation)
                .then(|_| Ok(())),
        )).unwrap();
    }

    pub fn render(&self, params: render::Params) -> render::Frame {
        let buffer = self.buffer.borrow();
        let mut lines = Vec::new();
        let mut cur_line = Vec::new();
        let scroll_bottom = params.scroll_top + params.height;
        let start_row = (params.scroll_top / params.line_height).floor() as u32;
        let end_row = (scroll_bottom / params.line_height).ceil() as u32;

        let mut cur_row = start_row;
        for c in buffer.iter_starting_at_row(start_row) {
            if c == (b'\n' as u16) {
                lines.push(cur_line);
                cur_line = Vec::new();
                cur_row += 1;
                if cur_row >= end_row {
                    break;
                }
            } else {
                cur_line.push(c);
            }
        }
        if cur_row < end_row {
            lines.push(cur_line);
        }

        render::Frame {
            first_visible_row: start_row,
            lines,
            selections: self.selections.iter().map(|selection| selection.render(&buffer)).collect()
        }
    }

    pub fn add_selection(&mut self, start: Point, end: Point) {
        debug_assert!(start <= end); // TODO: Reverse selection if end < start

        {
            let buffer = self.buffer.borrow();

            // TODO: Clip points or return a result.
            let start_anchor = buffer.anchor_before_point(start).unwrap();
            let end_anchor = buffer.anchor_before_point(end).unwrap();
            let index = match self.selections.binary_search_by(|probe| buffer.cmp_anchors(&probe.start, &start_anchor).unwrap()) {
                Ok(index) => index,
                Err(index) => index
            };
            self.selections.insert(index, Selection {
                start: start_anchor,
                end: end_anchor,
                reversed: false,
                goal_column: None
            });
        }

        self.merge_selections();
    }

    pub fn add_selection_above(&mut self) {
        {
            let buffer = self.buffer.borrow();

            let mut new_selections = Vec::new();
            for selection in &self.selections {
                let selection_start = buffer.point_for_anchor(&selection.start).unwrap();
                let selection_end = buffer.point_for_anchor(&selection.end).unwrap();

                let mut should_insert = false;
                let mut start = selection_start;
                let mut end = selection_end;
                let mut start_goal_column = if selection_start == selection_end {
                    selection.goal_column.clone()
                } else {
                    None
                };
                let mut end_goal_column = selection.goal_column.clone();
                while !should_insert && end.row > 0 {
                    let (new_start, new_start_goal_column) = movement::up(&buffer, start, start_goal_column);
                    let (new_end, new_end_goal_column) = movement::up(&buffer, end, end_goal_column);

                    if selection_start == selection_end {
                        should_insert = selection_end.column == 0 || new_end.column > 0;
                    } else {
                        should_insert = new_start.column != new_end.column;
                    }

                    start = new_start;
                    end = new_end;
                    start_goal_column = new_start_goal_column;
                    end_goal_column = new_end_goal_column;
                }

                if should_insert {
                    new_selections.push(Selection {
                        start: buffer.anchor_before_point(start).unwrap(),
                        end: buffer.anchor_before_point(end).unwrap(),
                        reversed: selection.reversed,
                        goal_column: end_goal_column
                    });
                }
            }

            for selection in new_selections {
                let index = match self.selections.binary_search_by(|probe| buffer.cmp_anchors(&probe.start, &selection.start).unwrap()) {
                    Ok(index) => index,
                    Err(index) => index
                };
                self.selections.insert(index, selection);
            }
        }

        self.merge_selections();
    }

    pub fn add_selection_below(&mut self) {
        {
            let buffer = self.buffer.borrow();
            let max_row = buffer.max_point().row;

            let mut new_selections = Vec::new();
            for selection in &self.selections {
                let selection_start = buffer.point_for_anchor(&selection.start).unwrap();
                let selection_end = buffer.point_for_anchor(&selection.end).unwrap();

                let mut should_insert = false;
                let mut start = selection_start;
                let mut end = selection_end;
                let mut start_goal_column = if selection_start == selection_end {
                    selection.goal_column.clone()
                } else {
                    None
                };
                let mut end_goal_column = selection.goal_column.clone();
                while !should_insert && end.row < max_row {
                    let (new_start, new_start_goal_column) = movement::down(&buffer, start, start_goal_column);
                    let (new_end, new_end_goal_column) = movement::down(&buffer, end, end_goal_column);

                    if selection_start == selection_end {
                        should_insert = selection_end.column == 0 || new_end.column > 0;
                    } else {
                        should_insert = new_start.column != new_end.column;
                    }

                    start = new_start;
                    end = new_end;
                    start_goal_column = new_start_goal_column;
                    end_goal_column = new_end_goal_column;
                }

                if should_insert {
                    new_selections.push(Selection {
                        start: buffer.anchor_before_point(start).unwrap(),
                        end: buffer.anchor_before_point(end).unwrap(),
                        reversed: selection.reversed,
                        goal_column: end_goal_column
                    });
                }
            }

            for selection in new_selections {
                let index = match self.selections.binary_search_by(|probe| buffer.cmp_anchors(&probe.start, &selection.start).unwrap()) {
                    Ok(index) => index,
                    Err(index) => index
                };
                self.selections.insert(index, selection);
            }
        }

        self.merge_selections();
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
                    let cursor = buffer.anchor_before_point(movement::left(&buffer, start)).unwrap();
                    selection.start = cursor.clone();
                    selection.end = cursor;
                }
                selection.reversed = false;
                selection.goal_column = None;
            }
        }
        self.merge_selections();
    }

    pub fn select_left(&mut self) {
        {
            let buffer = self.buffer.borrow();
            for selection in &mut self.selections {
                let head = buffer.point_for_anchor(selection.head()).unwrap();
                let cursor = buffer.anchor_before_point(movement::left(&buffer, head)).unwrap();
                selection.set_head(&buffer, cursor);
                selection.goal_column = None;
            }
        }
        self.merge_selections();
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
                    let cursor = buffer.anchor_before_point(movement::right(&buffer, end)).unwrap();
                    selection.start = cursor.clone();
                    selection.end = cursor;
                }
                selection.reversed = false;
                selection.goal_column = None;
            }
        }
        self.merge_selections();
    }

    pub fn select_right(&mut self) {
        {
            let buffer = self.buffer.borrow();
            for selection in &mut self.selections {
                let head = buffer.point_for_anchor(selection.head()).unwrap();
                let cursor = buffer.anchor_before_point(movement::right(&buffer, head)).unwrap();
                selection.set_head(&buffer, cursor);
                selection.goal_column = None;
            }
        }
        self.merge_selections();
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
    }

    fn merge_selections(&mut self) {
        let buffer = self.buffer.borrow();
        let mut i = 1;
        while i < self.selections.len() {
            if buffer.cmp_anchors(&self.selections[i - 1].end, &self.selections[i].start).unwrap() >= Ordering::Equal {
                let removed = self.selections.remove(i);
                if buffer.cmp_anchors(&removed.end, &self.selections[i - 1].end).unwrap() > Ordering::Equal {
                    self.selections[i - 1].end = removed.end;
                }
            } else {
                i += 1;
            }
        }
    }
}

impl Drop for Editor {
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

    fn render(&self, buffer: &Buffer) -> render::Selection {
        render::Selection {
            start: buffer.point_for_anchor(&self.start).unwrap(),
            end: buffer.point_for_anchor(&self.end).unwrap(),
            reversed: self.reversed
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate tokio_core;

    use super::*;
    use self::tokio_core::reactor::Core;
    use futures::future;

    #[test]
    fn test_version_updates() {
        let mut event_loop = Core::new().unwrap();
        let buffer = Rc::new(RefCell::new(Buffer::new(1)));
        let editor = Editor::new(buffer.clone());
        editor.run(&event_loop);
        buffer.borrow_mut().splice(0..0, "test");
        event_loop.run(editor.version.observe().take(1).into_future());
    }

    #[test]
    fn test_cursor_movement() {
        let mut editor = Editor::new(Rc::new(RefCell::new(Buffer::new(1))));
        editor.buffer.borrow_mut().splice(0..0, "abc\n\ndef");
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 0)]);

        editor.move_right();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 1)]);

        // Wraps across lines moving right
        for _ in 0..3 { editor.move_right(); }
        assert_eq!(render_selections(&editor), vec![empty_selection(1, 0)]);

        // Stops at end
        for _ in 0..4 { editor.move_right(); }
        assert_eq!(render_selections(&editor), vec![empty_selection(2, 3)]);

        // Wraps across lines moving left
        for _ in 0..4 { editor.move_left(); }
        assert_eq!(render_selections(&editor), vec![empty_selection(1, 0)]);

        // Stops at start
        for _ in 0..4 { editor.move_left(); }
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
        let mut editor = Editor::new(Rc::new(RefCell::new(Buffer::new(1))));
        editor.buffer.borrow_mut().splice(0..0, "abc\n\ndef");
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 0)]);

        editor.select_right();
        assert_eq!(render_selections(&editor), vec![selection((0, 0), (0, 1))]);

        // Selecting right wraps across newlines
        for _ in 0..3 { editor.select_right(); }
        assert_eq!(render_selections(&editor), vec![selection((0, 0), (1, 0))]);

        // Moving right with a non-empty selection clears the selection
        editor.move_right();
        assert_eq!(render_selections(&editor), vec![empty_selection(1, 0)]);
        editor.move_right();
        assert_eq!(render_selections(&editor), vec![empty_selection(2, 0)]);

        // Selecting left wraps across newlines
        editor.select_left();
        assert_eq!(render_selections(&editor), vec![rev_selection((1, 0), (2, 0))]);
        editor.select_left();
        assert_eq!(render_selections(&editor), vec![rev_selection((0, 3), (2, 0))]);

        // Moving left with a non-empty selection clears the selection
        editor.move_left();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 3)]);

        // Reverse is updated correctly when selecting left and right
        editor.select_left();
        assert_eq!(render_selections(&editor), vec![rev_selection((0, 2), (0, 3))]);
        editor.select_right();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 3)]);
        editor.select_right();
        assert_eq!(render_selections(&editor), vec![selection((0, 3), (1, 0))]);
        editor.select_left();
        assert_eq!(render_selections(&editor), vec![empty_selection(0, 3)]);
        editor.select_left();
        assert_eq!(render_selections(&editor), vec![rev_selection((0, 2), (0, 3))]);

        // Selecting vertically moves the head and updates the reversed property
        editor.select_left();
        assert_eq!(render_selections(&editor), vec![rev_selection((0, 1), (0, 3))]);
        editor.select_down();
        assert_eq!(render_selections(&editor), vec![selection((0, 3), (1, 0))]);
        editor.select_down();
        assert_eq!(render_selections(&editor), vec![selection((0, 3), (2, 1))]);
        editor.select_up();
        editor.select_up();
        assert_eq!(render_selections(&editor), vec![rev_selection((0, 1), (0, 3))]);

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
        let mut editor = Editor::new(Rc::new(RefCell::new(Buffer::new(1))));
        editor.buffer.borrow_mut().splice(0..0, "abcd\nefgh\nijkl\nmnop");
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
                selection((2, 2), (2, 3))
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
                selection((2, 2), (2, 3))
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
                selection((2, 1), (2, 3))
            ]
        );
    }

    #[test]
    fn test_add_selection_above() {
        let mut editor = Editor::new(Rc::new(RefCell::new(Buffer::new(1))));
        editor.buffer.borrow_mut().splice(0..0, "\
            abcdefghijk\n\
            lmnop\n\
            \n\
            \n\
            qrstuvwxyz\n\
        ");

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
                selection((4, 7), (4, 9))
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
                selection((4, 7), (4, 9))
            ]
        );
    }

    #[test]
    fn test_add_selection_below() {
        let mut editor = Editor::new(Rc::new(RefCell::new(Buffer::new(1))));
        editor.buffer.borrow_mut().splice(0..0, "\
            abcdefgh\n\
            ijklm\n\
            \n\
            \n\
            nopqrstuvwx\n\
            yz\
        ");

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
                selection((4, 5), (4, 6))
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
                selection((4, 4), (4, 8))
            ]
        );
    }

    fn render_selections(editor: &Editor) -> Vec<render::Selection> {
        editor.selections.iter().map(|s| s.render(&editor.buffer.borrow())).collect()
    }

    fn empty_selection(row: u32, column: u32) -> render::Selection {
        render::Selection {
            start: Point::new(row, column),
            end: Point::new(row, column),
            reversed: false
        }
    }

    fn selection(start: (u32, u32), end: (u32, u32)) -> render::Selection {
        render::Selection {
            start: Point::new(start.0, start.1),
            end: Point::new(end.0, end.1),
            reversed: false
        }
    }

    fn rev_selection(start: (u32, u32), end: (u32, u32)) -> render::Selection {
        render::Selection {
            start: Point::new(start.0, start.1),
            end: Point::new(end.0, end.1),
            reversed: true
        }
    }
}
