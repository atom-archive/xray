use std::cmp;
use buffer::{Buffer, Point, Anchor};

pub fn left(buffer: &Buffer, cursor: &Anchor) -> Anchor {
    let mut point = buffer.point_for_anchor(cursor).unwrap();
    if point.column > 0 {
        point.column -= 1;
    } else if point.row > 0 {
        point.row -= 1;
        point.column = buffer.len_for_row(point.row).unwrap();
    }
    buffer.anchor_before_point(point).unwrap()
}

pub fn right(buffer: &Buffer, cursor: &Anchor) -> Anchor {
    let mut point = buffer.point_for_anchor(cursor).unwrap();
    let max_column = buffer.len_for_row(point.row).unwrap();
    if point.column < max_column {
        point.column += 1;
    } else if point.row < buffer.max_point().row {
        point.row += 1;
        point.column = 0;
    }
    buffer.anchor_before_point(point).unwrap()
}

pub fn up(buffer: &Buffer, cursor: &Anchor, goal_column: Option<u32>) -> (Anchor, Option<u32>) {
    let mut point = buffer.point_for_anchor(cursor).unwrap();
    let goal_column = goal_column.or(Some(point.column));
    if point.row > 0 {
        point.row -= 1;
        point.column = cmp::min(
            goal_column.unwrap(),
            buffer.len_for_row(point.row).unwrap()
        );
    } else {
        point = Point::new(0, 0);
    }

    (buffer.anchor_before_point(point).unwrap(), goal_column)
}

pub fn down(buffer: &Buffer, cursor: &Anchor, goal_column: Option<u32>) -> (Anchor, Option<u32>) {
    let mut point = buffer.point_for_anchor(cursor).unwrap();
    let goal_column = goal_column.or(Some(point.column));
    let max_point = buffer.max_point();
    if point.row < max_point.row {
        point.row += 1;
        point.column = cmp::min(
            goal_column.unwrap(),
            buffer.len_for_row(point.row).unwrap()
        )
    } else {
        point = max_point;
    }

    (buffer.anchor_before_point(point).unwrap(), goal_column)
}
