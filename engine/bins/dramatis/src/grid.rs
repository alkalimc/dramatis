//! A group's picture for a notification, when the user set none: the same grid the app
//! draws for it (`GroupAvatar` in the UI), composed as a PNG because a banner takes a file.
//! The user's own picture comes first, then up to three members in the order they joined,
//! in a 2x2 grid; with one member the two pictures are halves side by side, with two the
//! first spans both rows. A tile with no cached picture stays the app's field colour, as
//! the grid does while a portrait is not there.
//!
//! The composite is written to the media cache under a name made from what went into it,
//! so it is made once and a change of members or pictures makes a new one.

use std::path::PathBuf;

use api::Engine;
use api::engine::{Face, GroupFaces};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage, imageops};

/// Side of the composed picture; cached portraits are 180 pixels square.
const SIZE: u32 = 180;
/// The gap between tiles, as in the UI's grid. It stays transparent.
const GAP: u32 = 2;
/// The UI's `--field` in the default (dark) theme: what an empty tile shows.
const FIELD: Rgba<u8> = Rgba([0x36, 0x3e, 0x44, 0xff]);
/// Bumped when the drawing changes, so older composites are not reused.
const VERSION: &str = "1";

/// Where one tile sits in the picture, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The tiles of a grid of `count` pictures (the user's and the members', 1 to 4), in the
/// order the UI places them.
pub fn layout(count: usize) -> Vec<Cell> {
    let half = (SIZE - GAP) / 2;
    let far = half + GAP;
    let cell = |x, y, w, h| Cell { x, y, w, h };
    match count {
        0 => Vec::new(),
        1 => vec![cell(0, 0, SIZE, SIZE)],
        2 => vec![cell(0, 0, half, SIZE), cell(far, 0, half, SIZE)],
        3 => vec![
            cell(0, 0, half, SIZE),
            cell(far, 0, half, half),
            cell(far, far, half, half),
        ],
        _ => vec![
            cell(0, 0, half, half),
            cell(far, 0, half, half),
            cell(0, far, half, half),
            cell(far, far, half, half),
        ],
    }
}

/// The cache name for a composite of tiles whose pictures are identified by `tiles`
/// (`None`: the tile is empty). The order, the count and every picture change it.
pub fn key(tiles: &[Option<String>]) -> String {
    let count = tiles.len().to_string();
    let mut parts = vec!["notice-group", VERSION, &count];
    parts.extend(tiles.iter().map(|t| t.as_deref().unwrap_or("-")));
    api::media::stem(&parts)
}

/// Draw the tiles; `None` when none of them has a decodable picture (an all-empty square
/// says nothing, so the banner keeps the app's icon).
pub fn compose(tiles: &[Option<Vec<u8>>]) -> Option<Vec<u8>> {
    let faces: Vec<Option<RgbaImage>> = tiles
        .iter()
        .map(|t| {
            let bytes = t.as_deref()?;
            image::load_from_memory_with_format(bytes, ImageFormat::Png)
                .ok()
                .map(|i| i.into_rgba8())
        })
        .collect();
    if faces.iter().all(Option::is_none) {
        return None;
    }
    let mut canvas = RgbaImage::new(SIZE, SIZE);
    for (cell, face) in layout(faces.len()).into_iter().zip(&faces) {
        for y in cell.y..cell.y + cell.h {
            for x in cell.x..cell.x + cell.w {
                canvas.put_pixel(x, y, FIELD);
            }
        }
        if let Some(face) = face {
            let fitted = DynamicImage::ImageRgba8(face.clone())
                .resize_to_fill(cell.w, cell.h, imageops::FilterType::Triangle)
                .into_rgba8();
            imageops::overlay(&mut canvas, &fitted, i64::from(cell.x), i64::from(cell.y));
        }
    }
    let mut out = Vec::new();
    canvas
        .write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// The picture a notification for a group shows: the one the user set, else the grid from
/// pictures already cached (made and stored once). `None` when no face is cached at all.
/// Reads and writes files: call it off the UI thread.
pub fn group_image(engine: &Engine, faces: GroupFaces) -> Option<PathBuf> {
    if let Some(custom) = faces.custom {
        return Some(custom);
    }
    if faces.members.is_empty() {
        return None;
    }
    let tiles: Vec<Option<Face>> = std::iter::once(faces.user)
        .chain(faces.members.into_iter().map(|m| m.map(Face::File)))
        .collect();
    let identities: Vec<Option<String>> = tiles
        .iter()
        .map(|t| t.as_ref().map(Face::identity))
        .collect();
    if identities.iter().all(Option::is_none) {
        return None;
    }
    let stem = key(&identities);
    if let Some(path) = engine.cached_notice_image(&stem) {
        return Some(path);
    }
    let bytes: Vec<Option<Vec<u8>>> = tiles.iter().map(|t| t.as_ref()?.read()).collect();
    let png = compose(&bytes)?;
    engine.store_notice_image(&stem, &png)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(colour: [u8; 4], side: u32) -> Vec<u8> {
        let img = RgbaImage::from_pixel(side, side, Rgba(colour));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    fn pixel(png: &[u8], x: u32, y: u32) -> [u8; 4] {
        let img = image::load_from_memory(png).unwrap().into_rgba8();
        img.get_pixel(x, y).0
    }

    fn cell(x: u32, y: u32, w: u32, h: u32) -> Cell {
        Cell { x, y, w, h }
    }

    #[test]
    fn layout_follows_the_ui_grid() {
        // One member: the user's and the member's pictures as two halves.
        assert_eq!(layout(2), vec![cell(0, 0, 89, 180), cell(91, 0, 89, 180)]);
        // Two members: the user's tile spans both rows, the members stack at the right.
        assert_eq!(
            layout(3),
            vec![
                cell(0, 0, 89, 180),
                cell(91, 0, 89, 89),
                cell(91, 91, 89, 89)
            ]
        );
        // Three members: 2x2.
        let four = layout(4);
        assert_eq!(four.len(), 4);
        assert_eq!(four[3], cell(91, 91, 89, 89));
        // More pictures than tiles do not add any.
        assert_eq!(layout(7), four);
    }

    #[test]
    fn tiles_stay_inside_the_picture_and_apart() {
        for n in 1..=4 {
            let cells = layout(n);
            for (i, a) in cells.iter().enumerate() {
                assert!(a.x + a.w <= SIZE && a.y + a.h <= SIZE);
                for b in &cells[i + 1..] {
                    let apart = a.x + a.w + GAP <= b.x
                        || b.x + b.w + GAP <= a.x
                        || a.y + a.h + GAP <= b.y
                        || b.y + b.h + GAP <= a.y;
                    assert!(apart, "{a:?} touches {b:?}");
                }
            }
        }
    }

    #[test]
    fn the_key_follows_its_inputs() {
        let t = |a: &str, b: Option<&str>| vec![Some(a.to_owned()), b.map(str::to_owned)];
        let base = key(&t("me", Some("amy")));
        assert_eq!(base, key(&t("me", Some("amy"))));
        // A picture changed, a member changed, one went missing, the order changed.
        assert_ne!(base, key(&t("me2", Some("amy"))));
        assert_ne!(base, key(&t("me", Some("bob"))));
        assert_ne!(base, key(&t("me", None)));
        assert_ne!(base, key(&[Some("amy".to_owned()), Some("me".to_owned())]));
        // A member joining makes a new picture.
        let mut more = t("me", Some("amy"));
        more.push(Some("bob".to_owned()));
        assert_ne!(base, key(&more));
    }

    #[test]
    fn compose_fills_tiles_and_leaves_gaps_clear() {
        let red = png([255, 0, 0, 255], 180);
        let blue = png([0, 0, 255, 255], 180);
        let out = compose(&[Some(red), Some(blue), None]).unwrap();
        // The first tile spans both rows.
        assert_eq!(pixel(&out, 10, 10), [255, 0, 0, 255]);
        assert_eq!(pixel(&out, 10, 170), [255, 0, 0, 255]);
        // The second is the member; the third has no picture and shows the field colour.
        assert_eq!(pixel(&out, 120, 20), [0, 0, 255, 255]);
        assert_eq!(pixel(&out, 120, 150), [0x36, 0x3e, 0x44, 0xff]);
        // The gap between the columns is transparent.
        assert_eq!(pixel(&out, 90, 20)[3], 0);
    }

    #[test]
    fn compose_without_a_face_makes_nothing() {
        assert_eq!(compose(&[None, None]), None);
        // Bytes that are not a picture count as no face.
        assert_eq!(compose(&[Some(vec![1, 2, 3]), None]), None);
        assert!(compose(&[Some(vec![1, 2, 3]), Some(png([0, 255, 0, 255], 64))]).is_some());
    }
}
