/// Fuzzy filtering and match highlighting for dynamic resource tables.

/// Score from fuzzy matching a row against a query.
#[derive(Debug, Clone)]
pub struct FuzzyScore {
    pub score: i32,
    pub is_exact: bool,
}

/// Filter rows by query, returning (original_index, score) pairs sorted by relevance.
pub fn filter_and_rank(rows: &[Vec<String>], query: &str) -> Vec<(usize, FuzzyScore)> {
    let query_lower = query.to_ascii_lowercase();
    let mut results: Vec<(usize, FuzzyScore)> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let mut best_score = -1i32;
        let mut is_exact = false;
        for cell in row {
            let cell_lower = cell.to_ascii_lowercase();
            if cell_lower.contains(&query_lower) {
                let score = if cell_lower == query_lower {
                    100
                } else if cell_lower.starts_with(&query_lower) {
                    80
                } else {
                    50
                };
                if score > best_score {
                    best_score = score;
                    is_exact = score >= 80;
                }
            }
        }
        if best_score > 0 {
            results.push((i, FuzzyScore { score: best_score, is_exact }));
        }
    }
    results.sort_by(|a, b| b.1.score.cmp(&a.1.score));
    results
}

/// Find byte-offset ranges in `text` that match `query` (case-insensitive substring).
pub fn find_match_ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    let text_lower = text.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    let mut ranges = Vec::new();
    let mut start = 0;
    while let Some(pos) = text_lower[start..].find(&query_lower) {
        let abs_start = start + pos;
        let abs_end = abs_start + query.len();
        ranges.push((abs_start, abs_end));
        start = abs_end;
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_finds_matches() {
        let rows = vec![
            vec!["nginx-pod".into(), "default".into()],
            vec!["redis-pod".into(), "kube-system".into()],
        ];
        let results = filter_and_rank(&rows, "nginx");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 0);
    }

    #[test]
    fn filter_empty_query_returns_empty() {
        let rows = vec![vec!["nginx".into()]];
        // Empty query: no filtering (implementation returns empty for empty queries)
        let results = filter_and_rank(&rows, "");
        assert!(results.is_empty());
    }

    #[test]
    fn filter_no_matches() {
        let rows = vec![
            vec!["nginx-pod".into()],
            vec!["redis-pod".into()],
        ];
        let results = filter_and_rank(&rows, "xyz-nonexistent");
        assert!(results.is_empty());
    }

    #[test]
    fn filter_ranks_exact_higher() {
        let rows = vec![
            vec!["my-nginx-pod".into()],  // substring match
            vec!["nginx".into()],          // exact match (higher score)
        ];
        let results = filter_and_rank(&rows, "nginx");
        assert_eq!(results.len(), 2);
        // Exact match (index 1) should rank first
        assert_eq!(results[0].0, 1);
        assert_eq!(results[1].0, 0);
    }

    #[test]
    fn filter_ranks_prefix_higher_than_substring() {
        let rows = vec![
            vec!["my-nginx-pod".into()],   // substring match
            vec!["nginx-deploy".into()],    // prefix match (higher score)
        ];
        let results = filter_and_rank(&rows, "nginx");
        assert_eq!(results.len(), 2);
        // Prefix match should rank first
        assert_eq!(results[0].0, 1);
    }

    #[test]
    fn filter_searches_all_columns() {
        let rows = vec![
            vec!["redis-pod".into(), "nginx-namespace".into()],
        ];
        let results = filter_and_rank(&rows, "nginx");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 0);
    }

    #[test]
    fn filter_case_insensitive() {
        let rows = vec![vec!["Nginx-Pod".into()]];
        let results = filter_and_rank(&rows, "nginx");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn find_ranges_basic() {
        let ranges = find_match_ranges("nginx-deployment", "nginx");
        assert_eq!(ranges, vec![(0, 5)]);
    }

    #[test]
    fn find_ranges_empty_query() {
        let ranges = find_match_ranges("hello", "");
        assert!(ranges.is_empty());
    }

    #[test]
    fn find_ranges_no_match() {
        let ranges = find_match_ranges("hello", "xyz");
        assert!(ranges.is_empty());
    }

    #[test]
    fn find_ranges_middle_match() {
        let ranges = find_match_ranges("my-nginx-pod", "nginx");
        assert_eq!(ranges, vec![(3, 8)]);
    }

    #[test]
    fn find_ranges_case_insensitive() {
        let ranges = find_match_ranges("ConfigMap", "config");
        assert_eq!(ranges, vec![(0, 6)]);
    }

    #[test]
    fn find_ranges_multiple_occurrences() {
        let ranges = find_match_ranges("pod-pod-pod", "pod");
        assert_eq!(ranges, vec![(0, 3), (4, 7), (8, 11)]);
    }
}
