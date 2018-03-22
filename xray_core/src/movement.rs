use std::cmp;
use buffer::{Buffer, Point};

pub fn left(buffer: &Buffer, mut point: Point) -> Point {
    if point.column > 0 {
        point.column -= 1;
    } else if point.row > 0 {
        point.row -= 1;
        point.column = buffer.len_for_row(point.row).unwrap();
    }
    point
}

pub fn right(buffer: &Buffer, mut point: Point) -> Point {
    let max_column = buffer.len_for_row(point.row).unwrap();
    if point.column < max_column {
        point.column += 1;
    } else if point.row < buffer.max_point().row {
        point.row += 1;
        point.column = 0;
    }
    point
}

pub fn up(buffer: &Buffer, mut point: Point, goal_column: Option<u32>) -> (Point, Option<u32>) {
    let goal_column = goal_column.or(Some(point.column));
    if point.row > 0 {
        point.row -= 1;
        point.column = cmp::min(goal_column.unwrap(), buffer.len_for_row(point.row).unwrap());
    } else {
        point = Point::new(0, 0);
    }

    (point, goal_column)
}

pub fn down(buffer: &Buffer, mut point: Point, goal_column: Option<u32>) -> (Point, Option<u32>) {
    let goal_column = goal_column.or(Some(point.column));
    let max_point = buffer.max_point();
    if point.row < max_point.row {
        point.row += 1;
        point.column = cmp::min(goal_column.unwrap(), buffer.len_for_row(point.row).unwrap())
    } else {
        point = max_point;
    }

    (point, goal_column)
}
