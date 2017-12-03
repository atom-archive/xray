use std::sync::Arc;

pub trait Summarize {
    type Summary: Accumulate;

    fn summarize(&self) -> Self::Summary;
}

pub trait Accumulate {
    fn accumulate<T: IntoIterator<Item=Self>>() -> Self where Self: Sized;
}

pub enum Tree<T: Summarize> {
    Empty,
    Leaf {
        summary: T::Summary,
        value: T
    },
    Internal {
        summary: T::Summary,
        children: Vec<Arc<Tree<T>>>,
        height: u16
    }
}

impl<T: Summarize> From<T> for Tree<T> {
    fn from(value: T) -> Self {
        Tree::Leaf {
            summary: value.summarize(),
            value: value
        }
    }
}

impl<T: Summarize> Tree<T> {
    pub fn new() -> Self {
        Tree::Empty
    }

    pub fn concat(&self, other: Self) -> Self {
        Tree::Empty
    }
}
