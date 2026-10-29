/// Returns input indices whose labels contain `query` as a case-insensitive subsequence.
///
/// Results are ordered by match tightness, then by their original input order.
pub fn fuzzy_filter(query: &str, labels: &[String]) -> Vec<usize> {
  let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
  let mut matches: Vec<(usize, usize)> = labels
    .iter()
    .enumerate()
    .filter_map(|(index, label)| fuzzy_score(&query, label).map(|score| (index, score)))
    .collect();
  matches.sort_by_key(|(index, score)| (*score, *index));
  matches.into_iter().map(|(index, _)| index).collect()
}

fn fuzzy_score(query: &[char], candidate: &str) -> Option<usize> {
  if query.is_empty() {
    return Some(0);
  }
  let candidate: Vec<char> = candidate.chars().flat_map(char::to_lowercase).collect();
  let mut position = 0;
  let mut previous = None;
  let mut score = 0;
  for target in query {
    let offset = candidate[position..]
      .iter()
      .position(|character| character == target)?;
    position += offset;
    if let Some(previous) = previous {
      score += position.saturating_sub(previous + 1);
    } else {
      score += position;
    }
    previous = Some(position);
    position += 1;
  }
  Some(score)
}

#[cfg(test)]
mod tests {
  use super::fuzzy_filter;

  // Developer approved: do not remove.
  #[test]
  fn fuzzy_filter_matches_case_insensitive_subsequences_and_prefers_tight_matches() {
    let labels = ["foo-bar".to_owned(), "fuzzy picker".to_owned(), "far-away".to_owned()];

    assert_eq!(fuzzy_filter("FP", &labels), vec![1]);
    assert_eq!(fuzzy_filter("fa", &labels), vec![2, 0]);
  }

  // Developer approved: do not remove.
  #[test]
  fn empty_query_preserves_the_input_order() {
    let labels = ["third".to_owned(), "first".to_owned()];

    assert_eq!(fuzzy_filter("", &labels), vec![0, 1]);
  }
}
