//! アイテム配列への操作。配列順が表示順の正。

use crate::config::Item;

/// 既定の表示名。HTTP/HTTPS URL はホスト部分、ローカルパスは末尾のファイル名またはフォルダー名、
/// いずれも取れなければ入力値そのもの。
pub fn default_name(path: &str) -> String {
    let trimmed = path.trim();
    let lower = trimmed.to_ascii_lowercase();
    for scheme in ["https://", "http://"] {
        if lower.starts_with(scheme) {
            let rest = &trimmed[scheme.len()..];
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            let host = host.rsplit('@').next().unwrap_or(host); // user:pass@ を除く
            let host = host.split(':').next().unwrap_or(host); // ポートを除く
            return if host.is_empty() { trimmed.to_owned() } else { host.to_owned() };
        }
    }
    let without_trailing = trimmed.trim_end_matches(['\\', '/']);
    match without_trailing.rsplit(['\\', '/']).next() {
        Some(name) if !name.is_empty() && !name.ends_with(':') => name.to_owned(),
        _ => trimmed.to_owned(),
    }
}

/// Windows のパスは大文字小文字を区別しない。
pub fn contains(items: &[Item], path: &str) -> bool {
    items.iter().any(|i| lower(&i.path) == lower(path))
}

fn lower(s: &str) -> String {
    s.trim().to_lowercase()
}

/// 未登録のパスだけを末尾へ追加し、追加した件数を返す。
pub fn add_paths(items: &mut Vec<Item>, paths: impl IntoIterator<Item = String>) -> usize {
    let mut added = 0;
    for raw in paths {
        let path = raw.trim();
        if path.is_empty() || contains(items, path) {
            continue;
        }
        items.push(Item { name: default_name(path), path: path.to_owned() });
        added += 1;
    }
    added
}

/// 隣接項目と入れ替える。端で移動できない場合は false。
pub fn move_by<T>(items: &mut [T], index: usize, offset: isize) -> bool {
    let Some(target) = index.checked_add_signed(offset) else { return false };
    if index >= items.len() || target >= items.len() {
        return false;
    }
    items.swap(index, target);
    true
}

/// ドラッグ並び替えの挿入結果。`dragged` を取り除いた配列で、`target` の前 (`after` = false)
/// または後へ挿入した新しい順序を返す。対象が自分自身・範囲外なら元の順序のまま。
pub fn reorder<T: PartialEq + Copy>(order: &[T], dragged: T, target: T, after: bool) -> Vec<T> {
    let mut result = order.to_vec();
    if dragged == target || !result.contains(&dragged) || !result.contains(&target) {
        return result;
    }
    result.retain(|&x| x != dragged);
    let index = result.iter().position(|&x| x == target).unwrap() + after as usize;
    result.insert(index, dragged);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(default_name(r"C:\Tools\app.exe"), "app.exe");
        assert_eq!(default_name(r"C:\Users\me\Documents\"), "Documents");
        assert_eq!(default_name(r"C:\"), r"C:\");
        assert_eq!(default_name("https://www.example.com/path?q=1"), "www.example.com");
        assert_eq!(default_name("http://example.org"), "example.org");
        assert_eq!(default_name("https://example.org:8080/x"), "example.org");
        assert_eq!(default_name("notepad"), "notepad");
    }

    #[test]
    fn add_skips_duplicates_ignoring_case() {
        let mut items = vec![Item { name: "a".into(), path: r"C:\A.txt".into() }];
        let added = add_paths(&mut items, [r"c:\a.txt", r"C:\B.txt", r"C:\B.txt"].map(String::from));
        assert_eq!(added, 1);
        assert_eq!(items[1].name, "B.txt");
    }

    #[test]
    fn move_ignores_ends() {
        let mut items: Vec<Item> = "ABC".chars().map(|c| Item { name: c.into(), path: c.into() }).collect();
        assert!(!move_by(&mut items, 0, -1));
        assert!(!move_by(&mut items, 2, 1));
        assert!(move_by(&mut items, 0, 1));
        assert_eq!(items.iter().map(|i| i.name.as_str()).collect::<String>(), "BAC");
    }

    #[test]
    fn reorder_inserts_before_or_after() {
        let run = |order: &str, d: char, t: char, after: bool| {
            reorder(&order.chars().collect::<Vec<_>>(), d, t, after).into_iter().collect::<String>()
        };
        assert_eq!(run("ABCDE", 'A', 'C', false), "BACDE");
        assert_eq!(run("ABCDE", 'A', 'C', true), "BCADE");
        assert_eq!(run("ABCDE", 'E', 'B', false), "AEBCD");
        assert_eq!(run("ABCDE", 'E', 'B', true), "ABECD");
        assert_eq!(run("ABCDE", 'C', 'E', true), "ABDEC");
        assert_eq!(run("ABCDE", 'C', 'A', false), "CABDE");
        assert_eq!(run("ABCDE", 'C', 'C', true), "ABCDE");
    }
}
