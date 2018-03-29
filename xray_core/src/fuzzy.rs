use std::f64;
use std::fmt;
use std::ops::{Index, IndexMut};

pub type Score = f64;

pub const SCORE_MIN: Score = f64::NEG_INFINITY;
const SCORE_GAP_LEADING: Score = -0.005;
const SCORE_GAP_TRAILING: Score = -0.005;
const SCORE_GAP_INNER: Score = -0.01;
const SCORE_MATCH_CONSECUTIVE: Score = 1.0;
const SCORE_MATCH_SLASH: Score = 0.9;
const SCORE_MATCH_WORD: Score = 0.8;
const SCORE_MATCH_CAPITAL: Score = 0.7;
const SCORE_MATCH_DOT: Score = 0.6;

pub struct Matcher<'a> {
    needle: &'a [char],
    stack: Vec<usize>,
}

pub struct Scorer<'a> {
    needle: &'a [char],
    d: Matrix<Score>,
    m: Matrix<Score>,
    bonus_cache: Vec<Score>,
    stack: Vec<usize>,
}

struct Matrix<T> {
    rows: usize,
    cols: usize,
    buffer: Vec<T>,
}

impl<'a> Matcher<'a> {
    pub fn new(needle: &'a [char]) -> Self {
        Self {
            needle,
            stack: Vec::new(),
        }
    }

    pub fn push(&mut self, component: &[char]) -> bool {
        if self.needle.is_empty() {
            true
        } else {
            let mut needle_index = self.stack.last().cloned().unwrap_or(0);
            for ch in component {
                if self.needle[needle_index].eq_ignore_ascii_case(ch) {
                    needle_index += 1;
                    if needle_index == self.needle.len() {
                        self.stack.push(needle_index);
                        return true;
                    }
                }
            }
            self.stack.push(needle_index);
            false
        }
    }

    pub fn pop(&mut self) {
        self.stack.pop();
    }
}

impl<'a> Scorer<'a> {
    pub fn new(needle: &'a [char]) -> Self {
        Self {
            d: Matrix::new(needle.len(), 0),
            m: Matrix::new(needle.len(), 0),
            needle,
            bonus_cache: Vec::new(),
            stack: Vec::new(),
        }
    }

    pub fn push(&mut self, component: &[char], positions: Option<&mut [usize]>) -> Score {
        let component_len = component.len();
        let haystack_start = self.m.cols;
        let haystack_len = haystack_start + component_len;
        let needle_len = self.needle.len();

        self.stack.push(component_len);
        self.precompute_bonus(component);
        self.d.add_columns(component_len);
        self.m.add_columns(component_len);

        for i in 0..needle_len {
            let mut prev_score = if haystack_start > 0 {
                self.m[(i, haystack_start - 1)]
            } else {
                SCORE_MIN
            };
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
                        score =
                            (j as Score * SCORE_GAP_LEADING) + self.bonus_cache[j - haystack_start];
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
                while j != 0 {
                    if self.d[(i, j)] != SCORE_MIN
                        && (match_required || self.d[(i, j)] == self.m[(i, j)])
                    {
                        match_required = i > 0 && j > 0
                            && self.m[(i, j)] == self.d[(i - 1, j - 1)] + SCORE_MATCH_CONSECUTIVE;
                        positions[i] = j;
                        j -= 1;
                        break;
                    }

                    j -= 1;
                }
            }
        });

        self.m[(needle_len - 1, haystack_len - 1)]
    }

    pub fn pop(&mut self) {
        let component_len = self.stack.pop().unwrap();
        self.d.remove_columns(component_len);
        self.m.remove_columns(component_len);
    }

    fn precompute_bonus(&mut self, component: &[char]) {
        self.bonus_cache.truncate(0);
        let mut last_ch = '/';
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
            buffer: Vec::with_capacity(rows * cols),
        }
    }

    fn add_columns(&mut self, additional: usize) {
        let prev_len = self.buffer.len();
        self.buffer
            .resize(prev_len + (self.rows * additional), T::default());
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

impl fmt::Debug for Matrix<f64> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for row in 0..self.rows {
            for col in 0..self.cols {
                write!(f, "{:.2} ", self[(row, col)])?;
            }
            writeln!(f, "")?;
        }

        Ok(())
    }
}

#[inline(always)]
fn compute_bonus(last_ch: char, ch: char) -> Score {
    if last_ch as usize > 255 || ch as usize > 255 {
        0_f64
    } else {
        BONUS_STATES[BONUS_INDEX[ch as usize] * 256 + last_ch as usize]
    }
}

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
    static ref BONUS_STATES: [Score; 3 * 256] = {
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
    fn test_matcher() {
        let needle = to_chars("abc");
        let mut matcher = Matcher::new(&needle);
        assert_eq!(matcher.push(&to_chars("abra/")), false);
        assert_eq!(matcher.push(&to_chars("cadabra")), true);
        matcher.pop();
        assert_eq!(matcher.push(&to_chars("ham/")), false);
        assert_eq!(matcher.push(&to_chars("lincoln")), true);
    }

    #[test]
    fn test_scorer() {
        let mut positions = [0; 3].to_vec();
        let needle = to_chars("bna");
        let mut scorer = Scorer::new(&needle);
        scorer.push(&to_chars("abc/"), None);
        scorer.push(&to_chars("bandana/"), None);
        scorer.push(&to_chars("banana/"), None);
        scorer.push(&to_chars("foo"), Some(&mut positions));
        assert_eq!(positions, &[12, 14, 15]);

        scorer.pop();
        scorer.pop();
        scorer.push(&to_chars("bar"), Some(&mut positions));
        assert_eq!(positions, &[4, 9, 10]);

        scorer.pop();
        scorer.pop();
        scorer.push(&to_chars("ban/"), None);
        scorer.push(&to_chars("dana"), Some(&mut positions));
        assert_eq!(positions, &[4, 10, 11]);
    }

    fn to_chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }
}
