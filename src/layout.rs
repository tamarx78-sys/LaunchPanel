//! 複数列レイアウトの計算。列数は「幅 / 列幅」を四捨五入して決める。
//! スナップ後の幅は列幅のちょうど整数倍になり、列数が切り替わる閾値 (x.5 倍) から
//! 最も遠いので、DPI や小数座標の丸め誤差で列数が増減しない。

use crate::config::MIN_INNER_SIZE;

/// 現在のウィンドウ幅から表示列数を決める。`item_count` が 1 以上なら列数の上限になる。
pub fn column_count(window_width: f64, column_width: f64, item_count: usize) -> usize {
    if !window_width.is_finite() || !column_width.is_finite() || column_width <= 0.0 {
        return 1;
    }
    let columns = ((window_width / column_width + 0.5).floor() as usize).max(1);
    if item_count >= 1 { columns.min(item_count) } else { columns }
}

/// 列数に対応するウィンドウ幅 (スナップ先)。
pub fn snapped_width(columns: usize, column_width: f64) -> f64 {
    (columns.max(1) as f64 * column_width).max(MIN_INNER_SIZE)
}

/// 1列の幅を変更した時の新しいウィンドウ幅。現在の列数を維持する。
pub fn width_for_column_width_change(current: f64, old_column: f64, new_column: f64, item_count: usize) -> f64 {
    snapped_width(column_count(current, old_column, item_count), new_column)
}

/// 各列に割り当てる件数。割り切れない分は左の列から1件ずつ多く割り当てる。
pub fn column_sizes(item_count: usize, columns: usize) -> Vec<usize> {
    let columns = columns.max(1);
    (0..columns).map(|c| item_count / columns + usize::from(c < item_count % columns)).collect()
}

/// 配列上の位置から (列, 行) を求める。左列の上から下へ、次に右の列へ並べる。
pub fn cells(item_count: usize, columns: usize) -> Vec<(usize, usize)> {
    column_sizes(item_count, columns)
        .into_iter()
        .enumerate()
        .flat_map(|(c, n)| (0..n).map(move |r| (c, r)))
        .collect()
}

/// 上部表示用の文字列 `幅×高さ N列`。
pub fn status_text(width: f64, height: f64, columns: usize) -> String {
    format!("{}×{} {}列", width.round(), height.round(), columns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_count_follows_width() {
        assert_eq!(column_count(280.0, 280.0, 10), 1);
        assert_eq!(column_count(560.0, 280.0, 10), 2);
        assert_eq!(column_count(419.0, 280.0, 10), 1);
        assert_eq!(column_count(421.0, 280.0, 10), 2);
        assert_eq!(column_count(200.0, 280.0, 10), 1);
        assert_eq!(column_count(2000.0, 280.0, 3), 3);
        assert_eq!(column_count(2000.0, 280.0, 0), 7);
        assert_eq!(column_count(f64::NAN, 280.0, 5), 1);
    }

    #[test]
    fn stable_around_snapped_width() {
        for w in [559.4, 560.6, 559.999999] {
            assert_eq!(column_count(w, 280.0, 10), 2);
        }
    }

    #[test]
    fn sizes_and_cells() {
        assert_eq!(column_sizes(5, 3), vec![2, 2, 1]);
        assert_eq!(column_sizes(2, 3), vec![1, 1, 0]);
        assert_eq!(cells(5, 3), vec![(0, 0), (0, 1), (1, 0), (1, 1), (2, 0)]);
    }

    #[test]
    fn width_changes() {
        assert_eq!(snapped_width(3, 280.0), 840.0);
        assert_eq!(width_for_column_width_change(840.0, 280.0, 400.0, 10), 1200.0);
        assert_eq!(status_text(560.2, 599.6, 2), "560×600 2列");
    }
}
