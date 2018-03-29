use smallvec::SmallVec;
use std::u16;

type MatchIndices = SmallVec<[u16; 12]>;

pub struct Search {
    query: Vec<char>,
    variants: Vec<MatchVariant>,
    char_count: u16,
    subword_start_bonus: usize,
    consecutive_bonus: usize,
}

#[derive(Clone)]
pub struct Checkpoint {
    variants_len: usize,
    char_count: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchResult {
    pub score: usize,
    pub match_indices: MatchIndices,
}

#[derive(Debug, Clone)]
struct MatchVariant {
    score: usize,
    query_index: u16,
    match_indices: MatchIndices
}

impl Search {
    pub fn new(query: &str) -> Self {
        Search {
            query: query.chars().map(|c| c.to_ascii_lowercase()).collect(),
            char_count: 0,
            variants: vec![
                MatchVariant {
                    score: 0,
                    query_index: 0,
                    match_indices: MatchIndices::new()
                }
            ],
            subword_start_bonus: 0,
            consecutive_bonus: 0,
        }
    }

    pub fn set_subword_start_bonus(&mut self, bonus: usize) -> &mut Self {
        self.subword_start_bonus = bonus;
        self
    }

    pub fn set_consecutive_bonus(&mut self, bonus: usize) -> &mut Self {
        self.consecutive_bonus = bonus;
        self
    }

    pub fn get_checkpoint(&self) -> Checkpoint {
        Checkpoint{
            variants_len: self.variants.len(),
            char_count: self.char_count,
        }
    }

    pub fn restore_checkpoint(&mut self, checkpoint: Checkpoint) {
        self.variants.truncate(checkpoint.variants_len);
        self.char_count = checkpoint.char_count;
    }

    pub fn process<T: IntoIterator<Item = char>>(&mut self, characters: T, match_bonus: usize) -> &mut Self {
        let mut new_variants = Vec::new();
        let previous_variants_len = self.variants.len();

        let mut last_character_is_alphanumeric = false;
        for character in characters {
            for (variant_index, variant) in self.variants.iter_mut().enumerate() {
                if let Some(query_character) = self.query.get(variant.query_index as usize) {
                    if character == *query_character {
                        let mut new_variant = if variant_index >= previous_variants_len {
                            variant
                        } else {
                            new_variants.push(variant.clone());
                            new_variants.last_mut().unwrap()
                        };

                        let match_index = self.char_count;
                        new_variant.query_index += 1;
                        new_variant.score += match_bonus;

                        // Apply a bonus if the current character is the start of a word.
                        if last_character_is_alphanumeric {
                            new_variant.score += self.subword_start_bonus;
                        }

                        // Apply a bonus if the last character of the path also matched.
                        if new_variant.match_indices.last().map_or(false, |index| *index == match_index - 1) {
                            new_variant.score += self.consecutive_bonus;
                        }

                        new_variant.match_indices.push(match_index);
                    }
                }
            }

            for new_variant in new_variants.drain(..) {
                if self.variants.iter().all(|v|
                    v.query_index != new_variant.query_index || v.score <= new_variant.score
                ) {
                    self.variants.push(new_variant);
                }
            }

            last_character_is_alphanumeric = character.is_alphanumeric();
            self.char_count += 1;
        }

        self
    }

    pub fn finish(&self) -> Option<SearchResult> {
        let query_len = self.query.len() as u16;
        self.variants.iter()
            .filter(|v| v.query_index == query_len)
            .max_by_key(|v| v.score)
            .map(|v| SearchResult {
                match_indices: v.match_indices.clone(),
                score: v.score,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_search() {
        let mut search = Search::new("ace");
        let result = search.process("abcde".chars(), 1).finish();
        assert_eq!(result, Some(SearchResult {
            match_indices: MatchIndices::from_vec(vec![0, 2, 4]),
            score: 3,
        }));
    }

    #[test]
    fn test_search_with_checkpoints() {
        let mut search = Search::new("bg");
        search.set_consecutive_bonus(10);

        search.process("abc".chars(), 1);
        assert_eq!(search.finish(), None);

        // "abc/defg"
        let checkpoint = search.get_checkpoint();
        search.process("/defg".chars(), 1);
        assert_eq!(search.finish(), Some(SearchResult {
            match_indices: MatchIndices::from_vec(vec![1, 7]),
            score: 2,
        }));

        // "abc/debg"
        search.restore_checkpoint(checkpoint);
        search.process("/debg".chars(), 1);
        assert_eq!(search.finish(), Some(SearchResult {
            match_indices: MatchIndices::from_vec(vec![6, 7]),
            score: 12,
        }));
    }
}
