use buffer::{Buffer, Point};
use std::char::decode_utf16;
use std::cmp;

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

pub fn beginning_of_word(buffer: &Buffer, mut point: Point) -> Point {
    // TODO: remove this once the iterator returns char instances.
    let mut iter = decode_utf16(buffer.backward_iter_starting_at_point(point)).map(|c| c.unwrap());
    let skip_alphanumeric = iter.next().map_or(false, |c| c.is_alphanumeric());
    point = left(buffer, point);
    for character in iter {
        if skip_alphanumeric == character.is_alphanumeric() {
            point = left(buffer, point);
        } else {
            break;
        }
    }
    point
}

pub fn end_of_word(buffer: &Buffer, mut point: Point) -> Point {
    // TODO: remove this once the iterator returns char instances.
    let mut iter = decode_utf16(buffer.iter_starting_at_point(point)).map(|c| c.unwrap());
    let skip_alphanumeric = iter.next().map_or(false, |c| c.is_alphanumeric());
    point = right(buffer, point);
    for character in iter {
        if skip_alphanumeric == character.is_alphanumeric() {
            point = right(buffer, point);
        } else {
            break;
        }
    }
    point
}
