//! Fuzzy matching for the command palette.

/// Score a query against a candidate. Higher is better; None = no match.
/// Subsequence match with bonuses for word starts and adjacency.
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let c: Vec<char> = candidate.to_lowercase().chars().collect();
    // Try each query word independently; all must match.
    let mut total = 0;
    for word in q.split(|ch| *ch == ' ').filter(|w| !w.is_empty()) {
        total += score_word(word, &c)?;
    }
    Some(total)
}

/// How many times the query's words occur in the candidate beyond the first
/// hit each. A command whose aliases repeat the word ("Align Right … flush
/// right") is about that word; one that merely contains it ("Word Right") is
/// not.
pub fn extra_occurrences(query: &str, candidate: &str) -> i32 {
    let c = candidate.to_lowercase();
    query
        .to_lowercase()
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(|w| (c.matches(w).count() as i32 - 1).max(0))
        .sum()
}

fn score_word(q: &[char], c: &[char]) -> Option<i32> {
    // Exact substring first.
    if let Some(pos) = find_sub(q, c) {
        let word_start = pos == 0 || !c[pos - 1].is_alphanumeric();
        return Some(100 + if word_start { 40 } else { 0 } - pos as i32 / 2 - (c.len() as i32 - q.len() as i32) / 10);
    }
    // Subsequence.
    let mut qi = 0;
    let mut s = 0;
    let mut last: Option<usize> = None;
    for (i, ch) in c.iter().enumerate() {
        if qi < q.len() && *ch == q[qi] {
            let word_start = i == 0 || !c[i - 1].is_alphanumeric();
            s += 10;
            if word_start {
                s += 15;
            }
            if let Some(l) = last {
                if l + 1 == i {
                    s += 8;
                }
            }
            last = Some(i);
            qi += 1;
        }
    }
    if qi == q.len() {
        Some(s - (c.len() as i32) / 8)
    } else {
        // Tolerate one typo (skip one query char) for short-ish queries.
        if q.len() >= 4 {
            for skip in 0..q.len() {
                let mut q2 = q.to_vec();
                q2.remove(skip);
                if let Some(s2) = score_word_strict(&q2, c) {
                    return Some(s2 - 30);
                }
            }
        }
        None
    }
}

fn score_word_strict(q: &[char], c: &[char]) -> Option<i32> {
    let mut qi = 0;
    let mut s = 0;
    for (i, ch) in c.iter().enumerate() {
        if qi < q.len() && *ch == q[qi] {
            s += 10;
            if i == 0 || !c[i - 1].is_alphanumeric() {
                s += 15;
            }
            qi += 1;
        }
    }
    (qi == q.len()).then_some(s)
}

fn find_sub(q: &[char], c: &[char]) -> Option<usize> {
    if q.len() > c.len() {
        return None;
    }
    (0..=c.len() - q.len()).find(|&i| c[i..i + q.len()] == *q)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuzzy_finds_footnote_with_typo() {
        assert!(score("fooot", "Insert Footnote").is_some());
        assert!(score("fooot", "Format ▸ Footer ▸ Edit Footer").is_some());
        assert!(score("xyzzy", "Insert Footnote").is_none());
        assert!(score("bold", "Bold").unwrap() > score("bold", "Bold Off Something").unwrap());
    }
}
