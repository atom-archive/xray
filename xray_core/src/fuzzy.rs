use std::fmt;
use std::ops::{Index, IndexMut};
use std::f64;

struct Matrix<T> {
    rows: usize,
    cols: usize,
    buffer: Vec<T>,
}

pub struct Matcher {
    needle: Vec<u8>,
    d: Matrix<f64>,
    m: Matrix<f64>,
    bonus_cache: Vec<f64>,
    stack: Vec<usize>
}

impl Matcher {
    pub fn new(needle: String) -> Self {
        let needle = needle.as_bytes().to_vec();
        Self {
            d: Matrix::new(needle.len(), 0),
            m: Matrix::new(needle.len(), 0),
            needle,
            bonus_cache: Vec::new(),
            stack: Vec::new()
        }
    }

    pub fn push(&mut self, component: &[u8], positions: Option<&mut [usize]>) -> Option<f64> {
        let component_len = component.len();
        let haystack_start = self.m.cols;
        let haystack_len = haystack_start + component_len;
        let needle_len = self.needle.len();

        self.stack.push(component_len);
        self.precompute_bonus(component);
        self.d.add_columns(component_len);
        self.m.add_columns(component_len);

        for i in 0..needle_len {
            let mut prev_score = SCORE_MIN;
            let gap_score = if i == needle_len - 1 {
                SCORE_GAP_TRAILING
            } else {
                SCORE_GAP_INNER
            };

            for j in haystack_start..haystack_len {
                let needle_ch = self.needle[i];
                let haystack_ch = component[j - haystack_start];

                if needle_ch.eq_ignore_ascii_case(&haystack_ch) {
                    let score;
                    if i == 0 {
                        score = (j as f64 * SCORE_GAP_LEADING) + self.bonus_cache[j - haystack_start];
                    } else if j > 0 {
                        let score_1 = self.m[(i - 1, j - 1)] + self.bonus_cache[j - haystack_start];
                        let score_2 = self.d[(i - 1, j - 1)] + SCORE_MATCH_CONSECUTIVE;
                        score = score_1.max(score_2);
                    } else {
                        score = SCORE_MIN;
                    }

                    self.d[(i, j)] = score;
                    let best_score = score.max(prev_score + gap_score);
                    self.m[(i, j)] = best_score;
                    prev_score = best_score;
                } else {
                    self.d[(i, j)] = SCORE_MIN;
                    let best_score = prev_score + gap_score;
                    self.m[(i, j)] = best_score;
                    prev_score = best_score;
                }
            }
        }

        positions.map(|positions| {
            let mut match_required = false;
            let mut j = haystack_len - 1;
            for i in (0..needle_len).rev() {
                while j != 0  {
                    if self.d[(i, j)] != SCORE_MIN && (match_required || self.d[(i, j)] == self.m[(i, j)]) {
                        match_required =
                            i > 0 && j > 0 &&
                            self.m[(i, j)] == self.d[(i - 1, j - 1)] + SCORE_MATCH_CONSECUTIVE;
                        positions[i] = j;
                        j -= 1;
                        break;
                    }

                    j -= 1;
                }
            }

            self.m[(needle_len - 1, haystack_len - 1)]
        })
    }

    pub fn pop(&mut self) {
        let component_len = self.stack.pop().unwrap();
        self.d.remove_columns(component_len);
        self.m.remove_columns(component_len);
    }

    fn precompute_bonus(&mut self, component: &[u8]) {
        self.bonus_cache.truncate(0);
        let mut last_ch = b'/';
        for ch in component {
            self.bonus_cache.push(compute_bonus(last_ch, *ch));
            last_ch = *ch;
        }
    }
}

impl<T: Clone + Default> Matrix<T> {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            buffer: Vec::with_capacity(rows * cols)
        }
    }

    fn add_columns(&mut self, additional: usize) {
        let prev_len = self.buffer.len();
        self.buffer.resize(prev_len + (self.rows * additional), T::default());
        self.cols += additional;
    }

    fn remove_columns(&mut self, exceeding: usize) {
        let prev_len = self.buffer.len();
        self.buffer.truncate(prev_len - (self.rows * exceeding));
        self.cols -= exceeding;
    }
}

impl<T> Index<(usize, usize)> for Matrix<T> {
    type Output = T;

    fn index(&self, (row, col): (usize, usize)) -> &Self::Output {
        &self.buffer[(col * self.rows) + row]
    }
}

impl<T: Default> IndexMut<(usize, usize)> for Matrix<T> {
    fn index_mut(&mut self, (row, col): (usize, usize)) -> &mut Self::Output {
        &mut self.buffer[(col * self.rows) + row]
    }
}

impl<T: fmt::Debug> fmt::Debug for Matrix<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for row in 0..self.rows {
            for col in 0..self.cols {
                write!(f, "{:?} ", self[(row, col)])?;
            }
            writeln!(f, "")?;
        }

        Ok(())
    }
}

#[inline(always)]
fn compute_bonus(last_ch: u8, ch: u8) -> f64 {
    BONUS_STATES[BONUS_INDEX[ch as usize] * 256 + last_ch as usize]
}

const SCORE_MIN: f64 = f64::NEG_INFINITY;
const SCORE_GAP_LEADING: f64 = -0.005;
const SCORE_GAP_TRAILING: f64 = -0.005;
const SCORE_GAP_INNER: f64 = -0.01;
const SCORE_MATCH_CONSECUTIVE: f64 = 1.0;
const SCORE_MATCH_SLASH: f64 = 0.9;
const SCORE_MATCH_WORD: f64 = 0.8;
const SCORE_MATCH_CAPITAL: f64 = 0.7;
const SCORE_MATCH_DOT: f64 = 0.6;

lazy_static! {
    static ref BONUS_INDEX: [usize; 256] = {
        let mut table = [0; 256];

        for ch in b'A'..b'Z' {
            table[ch as usize] = 2;
        }

        for ch in b'a'..b'z' {
            table[ch as usize] = 1;
        }

        for ch in b'0'..b'9' {
            table[ch as usize] = 1;
        }

        table
    };

    static ref BONUS_STATES: [f64; 3 * 256] = {
        let mut table = [0_f64; 3 * 256];

        table[1 * 256 + b'/' as usize] = SCORE_MATCH_SLASH;
        table[1 * 256 + b'-' as usize] = SCORE_MATCH_WORD;
        table[1 * 256 + b'_' as usize] = SCORE_MATCH_WORD;
        table[1 * 256 + b' ' as usize] = SCORE_MATCH_WORD;
        table[1 * 256 + b'.' as usize] = SCORE_MATCH_DOT;

        table[2 * 256 + b'/' as usize] = SCORE_MATCH_SLASH;
        table[2 * 256 + b'-' as usize] = SCORE_MATCH_WORD;
        table[2 * 256 + b'_' as usize] = SCORE_MATCH_WORD;
        table[2 * 256 + b' ' as usize] = SCORE_MATCH_WORD;
        table[2 * 256 + b'.' as usize] = SCORE_MATCH_DOT;
        for ch in b'a'..b'z' {
            table[2 * 256 + ch as usize] = SCORE_MATCH_CAPITAL;
        }

        table
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic() {
        let mut positions = [0; 3].to_vec();
        let mut search = Matcher::new("abc".to_owned());
        search.push(b"abc/", None);
        search.push(b"abc", Some(&mut positions));
        assert_eq!(positions, &[4, 5, 6]);
    }

    #[test]
    fn test_push_pop() {
        let mut positions = [0; 3].to_vec();
        let mut search = Matcher::new("bna".to_owned());
        search.push(b"abc/", None);
        search.push(b"bandana/", None);
        search.push(b"banana/", None);
        search.push(b"foo", Some(&mut positions));
        assert_eq!(positions, &[12, 14, 15]);

        search.pop();
        search.pop();
        search.push(b"bar", Some(&mut positions));
        assert_eq!(positions, &[4, 9, 10]);
    }
}
