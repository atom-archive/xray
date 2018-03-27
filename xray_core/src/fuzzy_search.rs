use smallvec::SmallVec;
use std::u16;

type MatchIndices = SmallVec<[u16; 12]>;

pub struct Search {
    query: Vec<char>,
    variants: Vec<MatchVariant>,
    characters: Vec<char>,
    subword_start_bonus: usize,
    consecutive_bonus: usize,
}

#[derive(Clone)]
pub struct Checkpoint {
    variants_len: usize,
    characters_len: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SearchResult {
    pub score: usize,
    pub match_indices: Vec<u16>,
    pub string: String,
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
            characters: Vec::new(),
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
            characters_len: self.characters.len(),
        }
    }

    pub fn restore_checkpoint(&mut self, checkpoint: Checkpoint) {
        self.variants.truncate(checkpoint.variants_len);
        self.characters.truncate(checkpoint.characters_len);
    }

    pub fn process<T: IntoIterator<Item = char>>(&mut self, characters: T, match_bonus: usize) -> &mut Self {
        let mut new_variants = Vec::new();

        for character in characters {
            for variant in &self.variants {
                if let Some(query_character) = self.query.get(variant.query_index as usize) {
                    if character == *query_character {
                        let mut new_variant = variant.clone();
                        let match_index = self.characters.len() as u16;
                        new_variant.query_index += 1;
                        new_variant.score += match_bonus;

                        // Apply a bonus if the current character is the start of a word.
                        if self.characters.last().map_or(true, |c| !c.is_alphanumeric()) {
                            new_variant.score += self.subword_start_bonus;
                        }

                        // Apply a bonus if the last character of the path also matched.
                        if new_variant.match_indices.last().map_or(false, |index| *index == match_index - 1) {
                            new_variant.score += self.consecutive_bonus;
                        }

                        new_variant.match_indices.push(match_index);
                        new_variants.push(new_variant);
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

            self.characters.push(character);
        }

        self
    }

    pub fn finish(&self) -> Option<SearchResult> {
        let query_len = self.query.len() as u16;
        self.variants.iter()
            .filter(|v| v.query_index == query_len)
            .max_by_key(|v| v.score)
            .map(|v| SearchResult {
                match_indices: v.match_indices.to_vec(),
                score: v.score,
                string: self.characters.iter().collect(),
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
            string: String::from("abcde"),
            match_indices: vec![0, 2, 4],
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
            string: String::from("abc/defg"),
            match_indices: vec![1, 7],
            score: 2,
        }));

        // "abc/debg"
        search.restore_checkpoint(checkpoint);
        search.process("/debg".chars(), 1);
        assert_eq!(search.finish(), Some(SearchResult {
            string: String::from("abc/debg"),
            match_indices: vec![6, 7],
            score: 12,
        }));
    }
}
