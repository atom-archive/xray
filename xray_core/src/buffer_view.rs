use buffer::{self, Buffer, BufferId, Point, Selection, SelectionSetId};
use futures::{Future, Poll, Stream};
use movement;
use notify_cell::NotifyCell;
use serde_json;
use std::cell::Ref;
use std::cell::RefCell;
use std::cmp::{self, Ordering};
use std::ops::Range;
use std::rc::Rc;
use window::{View, WeakViewHandle, Window};
use UserId;

pub trait BufferViewDelegate {
    fn set_active_buffer_view(&mut self, buffer_view: WeakViewHandle<BufferView>);
}

pub struct BufferView {
    user_id: UserId,
    buffer: Rc<RefCell<Buffer>>,
    updates_tx: NotifyCell<()>,
    updates_rx: Box<Stream<Item = (), Error = ()>>,
    dropped: NotifyCell<bool>,
    selection_set_id: SelectionSetId,
    height: Option<f64>,
    width: Option<f64>,
    line_height: f64,
    scroll_top: f64,
    vertical_margin: u32,
    pending_autoscroll: Option<AutoScrollRequest>,
    delegate: Option<WeakViewHandle<BufferViewDelegate>>,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct SelectionProps {
    pub user_id: UserId,
    pub start: Point,
    pub end: Point,
    pub reversed: bool,
    pub remote: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum BufferViewAction {
    UpdateScrollTop { delta: f64 },
    SetDimensions { width: u64, height: u64 },
    SetLongestLineWidth { width: f64 },
    Edit { text: String },
    Backspace,
    Delete,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    SelectUp,
    SelectDown,
    SelectLeft,
    SelectRight,
    AddSelectionAbove,
    AddSelectionBelow,
}

struct AutoScrollRequest {
    range: Range<buffer::Anchor>,
    center: bool,
}

impl BufferView {
    pub fn new(
        buffer: Rc<RefCell<Buffer>>,
        user_id: UserId,
        delegate: Option<WeakViewHandle<BufferViewDelegate>>,
    ) -> Self {
        let selection_set_id = {
            let mut buffer = buffer.borrow_mut();
            let start = buffer.anchor_before_offset(0).unwrap();
            let end = buffer.anchor_before_offset(0).unwrap();
            buffer.add_selection_set(
                user_id,
                vec![Selection {
                    start,
                    end,
                    reversed: false,
                    goal_column: None,
                }],
            )
        };

        let updates_tx = NotifyCell::new(());
        let updates_rx = Box::new(updates_tx.observe().select(buffer.borrow().updates()));
        Self {
            user_id,
            updates_tx,
            updates_rx,
            buffer,
            selection_set_id,
            dropped: NotifyCell::new(false),
            height: None,
            width: None,
            line_height: 10.0,
            scroll_top: 0.0,
            vertical_margin: 2,
            pending_autoscroll: None,
            delegate,
        }
    }

    pub fn set_height(&mut self, height: f64) -> &mut Self {
        debug_assert!(height >= 0_f64);
        self.height = Some(height);
        self.autoscroll_to_cursor(false);
        self.updated();
        self
    }

    pub fn set_width(&mut self, width: f64) -> &mut Self {
        debug_assert!(width >= 0_f64);
        self.width = Some(width);
        self.autoscroll_to_cursor(false);
        self.updated();
        self
    }

    pub fn set_line_height(&mut self, line_height: f64) -> &mut Self {
        debug_assert!(line_height > 0_f64);
        self.line_height = line_height;
        self.autoscroll_to_cursor(false);
        self.updated();
        self
    }

    pub fn set_scroll_top(&mut self, scroll_top: f64) -> &mut Self {
        debug_assert!(scroll_top >= 0_f64);
        self.scroll_top = scroll_top;
        self.pending_autoscroll = None;
        self.updated();
        self
    }

    fn scroll_top(&self) -> f64 {
        let max_scroll_top = f64::from(self.buffer.borrow().max_point().row) * self.line_height;
        self.scroll_top.min(max_scroll_top)
    }

    fn scroll_bottom(&self) -> f64 {
        self.scroll_top() + self.height.unwrap_or(0.0)
    }

    pub fn save(&self) -> Option<Box<Future<Item = (), Error = buffer::Error>>> {
        self.buffer.borrow().save()
    }

    pub fn edit(&mut self, text: &str) {
        {
            let mut offset_ranges = Vec::new();
            {
                let buffer = self.buffer.borrow();
                for selection in self.selections().iter() {
                    let start = buffer.offset_for_anchor(&selection.start).unwrap();
                    let end = buffer.offset_for_anchor(&selection.end).unwrap();
                    offset_ranges.push(start..end);
                }
            }

            let mut buffer = self.buffer.borrow_mut();
            buffer.edit(&offset_ranges, text);

            let mut delta = 0_isize;
            buffer
                .mutate_selections(self.selection_set_id, |buffer, selections| {
                    *selections = offset_ranges
                        .into_iter()
                        .map(|range| {
                            let start = range.start as isize;
                            let end = range.end as isize;
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
                })
                .unwrap();
        }

        self.autoscroll_to_cursor(false);
        self.updated();
    }

    pub fn backspace(&mut self) {
        if self.all_selections_are_empty() {
            self.select_left();
        }
        self.edit("");
    }

    pub fn delete(&mut self) {
        if self.all_selections_are_empty() {
            self.select_right();
        }
        self.edit("");
    }

    fn all_selections_are_empty(&self) -> bool {
        let buffer = self.buffer.borrow();
        self.selections()
            .iter()
            .all(|selection| selection.is_empty(&buffer))
    }

    pub fn set_selected_anchor_range(
        &mut self,
        range: Range<buffer::Anchor>,
    ) -> Result<(), buffer::Error> {
        {
            let mut buffer = self.buffer.borrow_mut();
            // Ensure the supplied anchors are valid to preserve invariants.
            buffer.offset_for_anchor(&range.start)?;
            buffer.offset_for_anchor(&range.end)?;
            buffer.mutate_selections(self.selection_set_id, |_, selections| {
                selections.clear();
                selections.push(Selection {
                    start: range.start,
                    end: range.end,
                    reversed: false,
                    goal_column: None,
                });
            })?;
        }
        self.autoscroll_to_selection(true);
        self.updated();
        Ok(())
    }

    pub fn add_selection(&mut self, start: Point, end: Point) {
        debug_assert!(start <= end); // TODO: Reverse selection if end < start

        self.buffer
            .borrow_mut()
            .mutate_selections(self.selection_set_id, |buffer, selections| {
                // TODO: Clip points or return a result.
                let start_anchor = buffer.anchor_before_point(start).unwrap();
                let end_anchor = buffer.anchor_before_point(end).unwrap();

                let index = match selections.binary_search_by(|probe| {
                    buffer.cmp_anchors(&probe.start, &start_anchor).unwrap()
                }) {
                    Ok(index) => index,
                    Err(index) => index,
                };
                selections.insert(
                    index,
                    Selection {
                        start: start_anchor,
                        end: end_anchor,
                        reversed: false,
                        goal_column: None,
                    },
                );
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn add_selection_above(&mut self) {
        self.buffer
            .borrow_mut()
            .insert_selections(self.selection_set_id, |buffer, selections| {
                let mut new_selections = Vec::new();
                for selection in selections.iter() {
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
                new_selections
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn add_selection_below(&mut self) {
        self.buffer
            .borrow_mut()
            .insert_selections(self.selection_set_id, |buffer, selections| {
                let max_row = buffer.max_point().row;

                let mut new_selections = Vec::new();
                for selection in selections.iter() {
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
                new_selections
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn move_left(&mut self) {
        self.buffer
            .borrow_mut()
            .mutate_selections(self.selection_set_id, |buffer, selections| {
                for selection in selections.iter_mut() {
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
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn select_left(&mut self) {
        self.buffer
            .borrow_mut()
            .mutate_selections(self.selection_set_id, |buffer, selections| {
                for selection in selections.iter_mut() {
                    let head = buffer.point_for_anchor(selection.head()).unwrap();
                    let cursor = buffer
                        .anchor_before_point(movement::left(&buffer, head))
                        .unwrap();
                    selection.set_head(&buffer, cursor);
                    selection.goal_column = None;
                }
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn move_right(&mut self) {
        self.buffer
            .borrow_mut()
            .mutate_selections(self.selection_set_id, |buffer, selections| {
                for selection in selections.iter_mut() {
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
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn select_right(&mut self) {
        self.buffer
            .borrow_mut()
            .mutate_selections(self.selection_set_id, |buffer, selections| {
                for selection in selections.iter_mut() {
                    let head = buffer.point_for_anchor(selection.head()).unwrap();
                    let cursor = buffer
                        .anchor_before_point(movement::right(&buffer, head))
                        .unwrap();
                    selection.set_head(&buffer, cursor);
                    selection.goal_column = None;
                }
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn move_up(&mut self) {
        self.buffer
            .borrow_mut()
            .mutate_selections(self.selection_set_id, |buffer, selections| {
                for selection in selections.iter_mut() {
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
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn select_up(&mut self) {
        self.buffer
            .borrow_mut()
            .mutate_selections(self.selection_set_id, |buffer, selections| {
                for selection in selections.iter_mut() {
                    let head = buffer.point_for_anchor(selection.head()).unwrap();
                    let (head, goal_column) = movement::up(&buffer, head, selection.goal_column);
                    selection.set_head(&buffer, buffer.anchor_before_point(head).unwrap());
                    selection.goal_column = goal_column;
                }
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn move_down(&mut self) {
        self.buffer
            .borrow_mut()
            .mutate_selections(self.selection_set_id, |buffer, selections| {
                for selection in selections.iter_mut() {
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
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn select_down(&mut self) {
        self.buffer
            .borrow_mut()
            .mutate_selections(self.selection_set_id, |buffer, selections| {
                for selection in selections.iter_mut() {
                    let head = buffer.point_for_anchor(selection.head()).unwrap();
                    let (head, goal_column) = movement::down(&buffer, head, selection.goal_column);
                    selection.set_head(&buffer, buffer.anchor_before_point(head).unwrap());
                    selection.goal_column = goal_column;
                }
            })
            .unwrap();
        self.autoscroll_to_cursor(false);
    }

    pub fn selections(&self) -> Ref<[Selection]> {
        Ref::map(self.buffer.borrow(), |buffer| {
            buffer.selections(self.selection_set_id).unwrap()
        })
    }

    pub fn buffer_id(&self) -> BufferId {
        self.buffer.borrow().id()
    }

    fn render_selections(&self, range: Range<Point>) -> Vec<SelectionProps> {
        let buffer = self.buffer.borrow();
        let mut rendered_selections = Vec::new();

        for (user_id, selections) in buffer.remote_selections() {
            for selection in self.query_selections(selections, &range) {
                rendered_selections.push(SelectionProps {
                    user_id,
                    start: buffer.point_for_anchor(&selection.start).unwrap(),
                    end: buffer.point_for_anchor(&selection.end).unwrap(),
                    reversed: selection.reversed,
                    remote: true,
                });
            }
        }

        for selection in
            self.query_selections(&buffer.selections(self.selection_set_id).unwrap(), &range)
        {
            rendered_selections.push(SelectionProps {
                user_id: self.user_id,
                start: buffer.point_for_anchor(&selection.start).unwrap(),
                end: buffer.point_for_anchor(&selection.end).unwrap(),
                reversed: selection.reversed,
                remote: false,
            });
        }

        rendered_selections
    }

    fn query_selections<'a>(
        &self,
        selections: &'a [Selection],
        range: &Range<Point>,
    ) -> &'a [Selection] {
        let buffer = self.buffer.borrow();
        let start = buffer.anchor_before_point(range.start).unwrap();
        let start_index = match selections
            .binary_search_by(|probe| buffer.cmp_anchors(&probe.start, &start).unwrap())
        {
            Ok(index) => index,
            Err(index) => {
                if index > 0
                    && buffer
                        .cmp_anchors(&selections[index - 1].end, &start)
                        .unwrap() == Ordering::Greater
                {
                    index - 1
                } else {
                    index
                }
            }
        };

        if range.end > buffer.max_point() {
            &selections[start_index..]
        } else {
            let end = buffer.anchor_after_point(range.end).unwrap();
            let end_index = match selections
                .binary_search_by(|probe| buffer.cmp_anchors(&probe.start, &end).unwrap())
            {
                Ok(index) => index,
                Err(index) => index,
            };

            &selections[start_index..end_index]
        }
    }

    fn autoscroll_to_cursor(&mut self, center: bool) {
        let anchor = {
            let selections = self.selections();
            let selection = selections.last().unwrap();
            if selection.reversed {
                selection.start.clone()
            } else {
                selection.end.clone()
            }
        };

        self.autoscroll_to_range(anchor.clone()..anchor, center)
            .unwrap();
    }

    fn autoscroll_to_selection(&mut self, center: bool) {
        let range = {
            let selections = self.selections();
            let selection = selections.last().unwrap();
            selection.start.clone()..selection.end.clone()
        };

        self.autoscroll_to_range(range, center).unwrap();
    }

    fn flush_pending_autoscroll_to_selection(&mut self) {
        if let Some(request) = self.pending_autoscroll.take() {
            self.autoscroll_to_range(request.range, request.center)
                .unwrap();
        }
    }

    fn autoscroll_to_range(
        &mut self,
        range: Range<buffer::Anchor>,
        center: bool,
    ) -> Result<(), buffer::Error> {
        // Ensure points are valid even if we can't autoscroll immediately because
        // flush_pending_autoscroll_to_selection unwraps.
        let (start, end) = {
            let buffer = self.buffer.borrow();
            let start = buffer.point_for_anchor(&range.start)?;
            let end = buffer.point_for_anchor(&range.end)?;
            (start, end)
        };
        if let Some(height) = self.height {
            let desired_top;
            let desired_bottom;
            if center {
                let center_position = ((start.row + end.row) as f64 / 2_f64) * self.line_height;
                desired_top = 0_f64.max(center_position - height / 2_f64);
                desired_bottom = center_position + height / 2_f64;
            } else {
                desired_top =
                    start.row.saturating_sub(self.vertical_margin) as f64 * self.line_height;
                desired_bottom =
                    end.row.saturating_add(self.vertical_margin) as f64 * self.line_height;
            }

            if self.scroll_top() > desired_top {
                self.set_scroll_top(desired_top);
            } else if self.scroll_bottom() < desired_bottom {
                self.set_scroll_top(desired_bottom - height);
            }
        } else {
            self.pending_autoscroll = Some(AutoScrollRequest { range, center });
        }

        Ok(())
    }

    fn updated(&mut self) {
        self.updates_tx.set(());
    }
}

impl View for BufferView {
    fn component_name(&self) -> &'static str {
        "BufferView"
    }

    fn will_mount(&mut self, window: &mut Window, self_handle: WeakViewHandle<Self>) {
        self.height = Some(window.height());
        self.flush_pending_autoscroll_to_selection();
        if let Some(ref delegate) = self.delegate {
            delegate.map(|delegate| delegate.set_active_buffer_view(self_handle));
        }
    }

    fn render(&self) -> serde_json::Value {
        let buffer = self.buffer.borrow();
        let start = Point::new((self.scroll_top() / self.line_height).floor() as u32, 0);
        let end = Point::new((self.scroll_bottom() / self.line_height).ceil() as u32, 0);

        let mut lines = Vec::new();
        let mut cur_line = Vec::new();
        let mut cur_row = start.row;
        for c in buffer.iter_starting_at_row(start.row) {
            if c == u16::from(b'\n') {
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

        let mut longest_line = Vec::new();
        for c in buffer.iter_starting_at_row(buffer.longest_row()) {
            if c == u16::from(b'\n') {
                break;
            } else {
                longest_line.push(c);
            }
        }

        json!({
            "first_visible_row": start.row,
            "lines": lines,
            "longest_line": String::from_utf16_lossy(&longest_line),
            "scroll_top": self.scroll_top(),
            "height": self.height,
            "width": self.width,
            "line_height": self.line_height,
            "selections": self.render_selections(start..end),
        })
    }

    fn dispatch_action(&mut self, action: serde_json::Value, _: &mut Window) {
        match serde_json::from_value(action) {
            Ok(BufferViewAction::UpdateScrollTop { delta }) => {
                let mut scroll_top = self.scroll_top() + delta;
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
            Ok(BufferViewAction::Backspace) => self.backspace(),
            Ok(BufferViewAction::Delete) => self.delete(),
            Ok(BufferViewAction::MoveUp) => self.move_up(),
            Ok(BufferViewAction::MoveDown) => self.move_down(),
            Ok(BufferViewAction::MoveLeft) => self.move_left(),
            Ok(BufferViewAction::MoveRight) => self.move_right(),
            Ok(BufferViewAction::SelectUp) => self.select_up(),
            Ok(BufferViewAction::SelectDown) => self.select_down(),
            Ok(BufferViewAction::SelectLeft) => self.select_left(),
            Ok(BufferViewAction::SelectRight) => self.select_right(),
            Ok(BufferViewAction::AddSelectionAbove) => self.add_selection_above(),
            Ok(BufferViewAction::AddSelectionBelow) => self.add_selection_below(),
            action @ _ => eprintln!("Unrecognized action {:?}", action),
        }
    }
}

impl Stream for BufferView {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.updates_rx.poll()
    }
}

impl Drop for BufferView {
    fn drop(&mut self) {
        self.buffer
            .borrow_mut()
            .remove_selection_set(self.selection_set_id)
            .unwrap();
        self.dropped.set(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use IntoShared;

    #[test]
    fn test_cursor_movement() {
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new(0))), 0, None);
        editor.buffer.borrow_mut().edit(&[0..0], "abc");
        editor.buffer.borrow_mut().edit(&[3..3], "\n");
        editor.buffer.borrow_mut().edit(&[4..4], "\ndef");
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
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new(0))), 0, None);
        editor.buffer.borrow_mut().edit(&[0..0], "abc");
        editor.buffer.borrow_mut().edit(&[3..3], "\n");
        editor.buffer.borrow_mut().edit(&[4..4], "\ndef");

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
    fn test_backspace() {
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new(0))), 0, None);
        editor.buffer.borrow_mut().edit(&[0..0], "abcdefghi");
        editor.add_selection(Point::new(0, 3), Point::new(0, 4));
        editor.add_selection(Point::new(0, 9), Point::new(0, 9));
        editor.backspace();
        assert_eq!(editor.buffer.borrow().to_string(), "abcefghi");
        editor.backspace();
        assert_eq!(editor.buffer.borrow().to_string(), "abefgh");
    }

    #[test]
    fn test_delete() {
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new(0))), 0, None);
        editor.buffer.borrow_mut().edit(&[0..0], "abcdefghi");
        editor.add_selection(Point::new(0, 3), Point::new(0, 4));
        editor.add_selection(Point::new(0, 9), Point::new(0, 9));
        editor.delete();
        assert_eq!(editor.buffer.borrow().to_string(), "abcefghi");
        editor.delete();
        assert_eq!(editor.buffer.borrow().to_string(), "bcfghi");
    }

    #[test]
    fn test_add_selection() {
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new(0))), 0, None);
        editor
            .buffer
            .borrow_mut()
            .edit(&[0..0], "abcd\nefgh\nijkl\nmnop");
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
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new(0))), 0, None);
        editor.buffer.borrow_mut().edit(
            &[0..0],
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
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new(0))), 0, None);
        editor.buffer.borrow_mut().edit(
            &[0..0],
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
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new(0))), 0, None);

        editor
            .buffer
            .borrow_mut()
            .edit(&[0..0], "abcdefgh\nhijklmno");

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
    fn test_autoscroll() {
        let mut buffer = Buffer::new(0);
        buffer.edit(&[0..0], "abc\ndef\nghi\njkl\nmno\npqr\nstu\nvwx\nyz");
        let start = buffer.anchor_before_offset(0).unwrap();
        let end = buffer.anchor_before_offset(buffer.len()).unwrap();
        let max_point = buffer.max_point();
        let mut editor = BufferView::new(buffer.into_shared(), 0, None);
        let line_height = 5.0;
        let height = 3.0 * line_height;
        editor
            .set_height(height)
            .set_line_height(line_height)
            .set_scroll_top(2.5 * line_height);
        assert_eq!(editor.scroll_top(), 2.5 * line_height);

        editor
            .autoscroll_to_range(start.clone()..start.clone(), true)
            .unwrap();
        assert_eq!(editor.scroll_top(), 0.0);
        editor
            .autoscroll_to_range(end.clone()..end.clone(), true)
            .unwrap();
        assert_eq!(
            editor.scroll_top(),
            (max_point.row as f64 * line_height) - (height / 2.0)
        );
    }

    #[test]
    fn test_render() {
        let buffer = Rc::new(RefCell::new(Buffer::new(0)));
        buffer
            .borrow_mut()
            .edit(&[0..0], "abc\ndef\nghi\njkl\nmno\npqr\nstu\nvwx\nyz");
        let line_height = 6.0;

        {
            let mut editor = BufferView::new(buffer.clone(), 0, None);
            // Selections starting or ending outside viewport
            editor.add_selection(Point::new(1, 2), Point::new(3, 1));
            editor.add_selection(Point::new(5, 2), Point::new(6, 0));
            // Selection fully inside viewport
            editor.add_selection(Point::new(3, 2), Point::new(4, 1));
            // Selection fully outside viewport
            editor.add_selection(Point::new(6, 3), Point::new(7, 2));
            editor
                .set_height(3.0 * line_height)
                .set_line_height(line_height)
                .set_scroll_top(2.5 * line_height);

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
                    selection((5, 2), (6, 0)),
                ])
            );
        }

        // Selection starting at the end of buffer
        {
            let mut editor = BufferView::new(buffer.clone(), 0, None);
            editor.add_selection(Point::new(8, 2), Point::new(8, 2));
            editor
                .set_height(8.0 * line_height)
                .set_line_height(line_height)
                .set_scroll_top(1.0 * line_height);

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
            let mut editor = BufferView::new(buffer.clone(), 0, None);
            editor.add_selection(Point::new(0, 2), Point::new(1, 0));
            editor
                .set_height(3.0 * line_height)
                .set_line_height(line_height)
                .set_scroll_top(1.0 * line_height);

            let frame = editor.render();
            assert_eq!(frame["first_visible_row"], 1);
            assert_eq!(stringify_lines(&frame["lines"]), vec!["def", "ghi", "jkl"]);
            assert_eq!(frame["selections"], json!([]));
        }
    }

    #[test]
    fn test_render_past_last_line() {
        let line_height = 4.0;
        let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new(0))), 0, None);
        editor.buffer.borrow_mut().edit(&[0..0], "abc\ndef\nghi");
        editor.add_selection(Point::new(2, 3), Point::new(2, 3));
        editor
            .set_height(3.0 * line_height)
            .set_line_height(line_height)
            .set_scroll_top(2.0 * line_height);

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

    #[test]
    fn test_dropping_view_removes_selection_set() {
        let buffer = Buffer::new(0).into_shared();
        let editor = BufferView::new(buffer.clone(), 0, None);
        let selection_set_id = editor.selection_set_id;
        assert!(buffer.borrow_mut().selections(selection_set_id).is_ok());

        drop(editor);
        assert!(buffer.borrow_mut().selections(selection_set_id).is_err());
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
        let buffer = editor.buffer.borrow();
        editor
            .selections()
            .iter()
            .map(|s| SelectionProps {
                user_id: 0,
                start: buffer.point_for_anchor(&s.start).unwrap(),
                end: buffer.point_for_anchor(&s.end).unwrap(),
                reversed: s.reversed,
                remote: false,
            })
            .collect()
    }

    fn empty_selection(row: u32, column: u32) -> SelectionProps {
        SelectionProps {
            user_id: 0,
            start: Point::new(row, column),
            end: Point::new(row, column),
            reversed: false,
            remote: false,
        }
    }

    fn selection(start: (u32, u32), end: (u32, u32)) -> SelectionProps {
        SelectionProps {
            user_id: 0,
            start: Point::new(start.0, start.1),
            end: Point::new(end.0, end.1),
            reversed: false,
            remote: false,
        }
    }

    fn rev_selection(start: (u32, u32), end: (u32, u32)) -> SelectionProps {
        SelectionProps {
            user_id: 0,
            start: Point::new(start.0, start.1),
            end: Point::new(end.0, end.1),
            reversed: true,
            remote: false,
        }
    }
}
