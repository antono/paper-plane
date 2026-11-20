/// A media item size, in pixels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PhysicalSize {
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// The position and size of a media item within the album grid, in pixels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// The width of a tile relative to its height. Individual ratios are clamped
/// between these bounds, so that extremely wide or tall media cannot distort
/// the whole grid.
const MIN_RATIO: f64 = 0.5;
const MAX_RATIO: f64 = 2.0;

/// Computes the grid of an album from the sizes of its items.
///
/// The items are distributed over rows according to the number of items and
/// their aspect ratios (see `row_splits`), and every row is laid out so that
/// its tiles are as tall as the row is high: the row height is the width
/// available after the spacing divided by the sum of the tile ratios. The
/// returned rects start at (0, 0) and span at most `max_width` horizontally.
pub(crate) fn calculate_album_grid(
    sizes: &[PhysicalSize],
    max_width: f64,
    spacing: f64,
) -> Vec<Rect> {
    let n = sizes.len();
    if n == 0 {
        return Vec::new();
    }

    let ratios: Vec<f64> = sizes
        .iter()
        .map(|size| (size.width / size.height).clamp(MIN_RATIO, MAX_RATIO))
        .collect();

    let splits = row_splits(n, &ratios);

    let mut rects = Vec::with_capacity(n);
    let mut item_idx = 0;
    let mut current_y = 0.0_f64;

    for row_size in splits {
        // Guard against a split asking for more items than are left, so that
        // the slicing below can never panic.
        let row_size = row_size.min(n - item_idx);
        if row_size == 0 {
            break;
        }

        let row_ratios = &ratios[item_idx..item_idx + row_size];
        let ratio_sum: f64 = row_ratios.iter().sum();

        let available_w = (max_width - (row_size as f64 - 1.0) * spacing).max(0.0);
        let row_height = available_w / ratio_sum;

        let mut current_x = 0.0_f64;
        for &r in row_ratios {
            let item_w = r * row_height;
            rects.push(Rect {
                x: current_x.round(),
                y: current_y.round(),
                width: item_w.round(),
                height: row_height.round(),
            });
            current_x += item_w + spacing;
        }

        current_y += row_height + spacing;
        item_idx += row_size;

        if item_idx >= n {
            break;
        }
    }

    rects
}

/// Selects the row split pattern of an album, i.e. the number of items of
/// every row, based on the item count and their aspect ratios.
///
/// Two portrait items share a single row, while other pairs are stacked.
/// Three items are laid out with the oldest one as a full-width tile on top
/// of a row of two. Larger albums are grouped in rows of three, with the
/// remainder distributed over the last rows.
fn row_splits(n: usize, ratios: &[f64]) -> Vec<usize> {
    match n {
        1 => vec![1],
        2 => {
            if ratios[0] < 0.9 && ratios[1] < 0.9 {
                vec![2]
            } else {
                vec![1, 1]
            }
        }
        3 => vec![1, 2],
        4 => vec![2, 2],
        5 => vec![2, 3],
        6 => vec![3, 3],
        7 => vec![3, 2, 2],
        8 => vec![3, 3, 2],
        9 => vec![3, 3, 3],
        10 => vec![4, 3, 3],
        _ => {
            let mut splits = vec![3; n / 3];
            match n % 3 {
                0 => {}
                1 => {
                    if let Some(last) = splits.last_mut() {
                        *last += 1;
                    } else {
                        splits.push(1);
                    }
                }
                _ => splits.push(2),
            }
            splits
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: f64, height: f64) -> PhysicalSize {
        PhysicalSize { width, height }
    }

    fn rows(rects: &[Rect]) -> Vec<usize> {
        let mut rows: Vec<(f64, usize)> = vec![];
        for rect in rects {
            match rows.last_mut() {
                Some((y, count)) if *y == rect.y => *count += 1,
                _ => rows.push((rect.y, 1)),
            }
        }
        rows.into_iter().map(|(_, count)| count).collect()
    }

    #[test]
    fn empty_album_has_no_tiles() {
        assert!(calculate_album_grid(&[], 400.0, 2.0).is_empty());
    }

    #[test]
    fn row_patterns_follow_the_item_count() {
        let square = size(100.0, 100.0);
        for (n, expected) in [
            (1, vec![1]),
            (2, vec![1, 1]),
            (3, vec![1, 2]),
            (4, vec![2, 2]),
            (5, vec![2, 3]),
            (6, vec![3, 3]),
            (7, vec![3, 2, 2]),
            (8, vec![3, 3, 2]),
            (9, vec![3, 3, 3]),
            (10, vec![4, 3, 3]),
        ] {
            let rects = calculate_album_grid(&vec![square; n], 400.0, 2.0);
            assert_eq!(rects.len(), n);
            assert_eq!(rows(&rects), expected, "{n} items");
        }
    }

    #[test]
    fn two_portrait_items_share_a_row() {
        let portrait = size(60.0, 100.0);
        let rects = calculate_album_grid(&[portrait, portrait], 400.0, 2.0);
        assert_eq!(rows(&rects), [2]);
    }

    #[test]
    fn large_albums_are_grouped_in_threes() {
        assert_eq!(row_splits(11, &[1.0; 11]), [3, 3, 3, 2]);
        assert_eq!(row_splits(13, &[1.0; 13]), [3, 3, 3, 4]);
        assert_eq!(row_splits(12, &[1.0; 12]), [3, 3, 3, 3]);
    }

    #[test]
    fn rows_fill_the_width_and_do_not_overlap() {
        let sizes = [
            size(1920.0, 1080.0),
            size(1080.0, 1920.0),
            size(100.0, 100.0),
            size(5000.0, 100.0), // clamped to MAX_RATIO
            size(100.0, 5000.0), // clamped to MIN_RATIO
        ];
        let (max_width, spacing) = (400.0, 2.0);
        let rects = calculate_album_grid(&sizes, max_width, spacing);

        for rect in &rects {
            assert!(rect.x >= 0.0 && rect.y >= 0.0);
            assert!(rect.x + rect.width <= max_width + 1.0, "{rect:?} too wide");
            let ratio = rect.width / rect.height;
            assert!(
                (MIN_RATIO - 0.05..=MAX_RATIO + 0.05).contains(&ratio),
                "{rect:?}"
            );
        }

        // The last tile of every row reaches the right edge (within rounding).
        for pair in rects.windows(2) {
            if pair[1].y != pair[0].y {
                let right = pair[0].x + pair[0].width;
                assert!((right - max_width).abs() <= 1.0, "{:?}", pair[0]);
                assert!(pair[1].y >= pair[0].y + pair[0].height, "rows overlap");
            }
        }
    }
}
